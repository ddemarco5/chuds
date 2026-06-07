use poise::serenity_prelude::{
    self as serenity, ComponentInteraction, ComponentInteractionDataKind, Http, MessageId,
};

use crate::chud_msg;
use crate::discord::channel::append_activity_log;
use crate::discord::formatting::format_item_block;
use crate::game::domain::player::Player;
use crate::game::merchant::{BuyError, MerchantState, MerchantVisit};
use crate::game::persistence::storage;

use super::super::components_v2::{
    components_v2_flags, ActionRow, Button, Component, ComponentsV2Message, ContainerChild,
    SelectOption, StringSelect, TextDisplay,
};
use super::super::context::Data;
use super::{no_chud_message, push_notice, respond_ephemeral_create, respond_ephemeral_update};

fn build_merchant_channel_message(merchant: &MerchantState) -> ComponentsV2Message {
    match &merchant.visit {
        None => ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(
            "\u{200B}\n\u{200B}",
        ))]),
        Some(visit) => {
            let mut inner = vec![ContainerChild::Text(TextDisplay::new(format!(
                "**{}** is visiting the guild hall.",
                visit.merchant_name
            )))];
            inner.push(ContainerChild::ActionRow(ActionRow::one_button(
                Button::primary("merchant:shop", "Shop"),
            )));
            ComponentsV2Message::channel_box(inner, None)
        }
    }
}

pub async fn update_merchant_message(
    http: &Http,
    channel_id: u64,
    merchant: &MerchantState,
) -> anyhow::Result<()> {
    let _cache_guard = storage::message_cache_lock().await;
    let ch = serenity::ChannelId::new(channel_id);
    let mut cache = storage::load_message_cache().unwrap_or_default();
    let mut cache_dirty = false;

    let message = build_merchant_channel_message(merchant);
    let key = message.cache_key();

    if cache.merchant_message_id.is_none() {
        match http.send_message(ch, vec![], &message).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "merchant message posted");
                cache.merchant_message_id = Some(msg.id.get());
                cache.merchant_content = key;
                cache_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post merchant message"),
        }
    } else if key != cache.merchant_content {
        let msg_id = cache.merchant_message_id.unwrap();
        if http
            .edit_message(ch, MessageId::new(msg_id), &message, vec![])
            .await
            .is_ok()
        {
            cache.merchant_content = key;
            cache_dirty = true;
            tracing::debug!(msg_id, "merchant message edited");
        } else {
            tracing::warn!(msg_id, "failed to edit merchant message");
        }
    }

    if cache_dirty {
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache");
        }
    }
    Ok(())
}

pub fn build_shop_message(
    visit: &MerchantVisit,
    player: &Player,
    selected_slot: usize,
    notice: Option<&str>,
) -> ComponentsV2Message {
    let mut components = Vec::new();

    push_notice(&mut components, notice);

    components.push(Component::Text(TextDisplay::new(format!(
        "You have ${} on hand",
        player.cash
    ))));

    let unsold: Vec<usize> = visit
        .stock
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.sold)
        .map(|(i, _)| i)
        .collect();

    if unsold.is_empty() {
        components.push(Component::Text(TextDisplay::new("_The merchant is sold out._")));
        return ComponentsV2Message {
            flags: components_v2_flags(),
            components,
        };
    }

    let selected = if unsold.contains(&selected_slot) {
        selected_slot
    } else {
        unsold[0]
    };

    for &slot in &unsold {
        components.push(Component::Text(TextDisplay::new(format_item_block(
            &visit.stock[slot].item,
        ))));
    }

    let select_options: Vec<SelectOption> = unsold
        .iter()
        .map(|&slot| {
            let item = &visit.stock[slot].item;
            let base = SelectOption::new(item.name.clone(), slot.to_string());
            if slot == selected {
                base.with_default(true)
            } else {
                base
            }
        })
        .collect();

    components.push(Component::ActionRow(ActionRow::string_select(
        StringSelect::new("shop_select", "Choose an item", select_options),
    )));

    let price = visit.stock[selected].item.value;
    components.push(Component::ActionRow(ActionRow::one_button(
        Button::primary(format!("shop_buy:{selected}"), format!("Buy ${price}")),
    )));

    ComponentsV2Message {
        flags: components_v2_flags(),
        components,
    }
}

fn buy_notice(err: BuyError) -> String {
    match err {
        BuyError::SoldOut => "_Sorry, someone snagged that._".into(),
        BuyError::InsufficientFunds => "_You don't have enough cash._".into(),
        BuyError::StashFull => "_Your stash is full._".into(),
        BuyError::InvalidSlot => "_That item is no longer available._".into(),
    }
}

async fn prepare_shop_response(
    data: &Data,
    user_id: u64,
    selected_slot: usize,
    buy_slot: Option<usize>,
) -> anyhow::Result<ComponentsV2Message> {
    let mut player = match storage::load_player(user_id)? {
        Some(p) if p.has_chud() => p,
        _ => return Ok(no_chud_message()),
    };

    let mut merchant = data.runtime.merchant.lock().await;
    let visit = match merchant.visit.as_ref() {
        Some(v) => v.clone(),
        None => {
            return Ok(ComponentsV2Message {
                flags: components_v2_flags(),
                components: vec![Component::Text(TextDisplay::new(
                    "The merchant has already left.",
                ))],
            });
        }
    };

    let mut notice = None;
    let mut bought = false;
    let mut bought_slot = None;
    if let Some(slot) = buy_slot {
        let mut registry = data.runtime.item_registry.lock().await;
        match merchant.buy(slot, &mut player, &mut registry) {
            Ok(item) => {
                notice = Some(format!("_Bought **{}** for ${}._", item.name, item.value));
                bought = true;
                bought_slot = Some(slot);
                let chud_name = player.chud_ref().name.clone();
                let first_name = chud_name
                    .split_whitespace()
                    .next()
                    .unwrap_or(&chud_name)
                    .to_string();
                let log_msg = chud_msg!("chud_buys_item", first_name, item.name, item.value);
                append_activity_log(&data.runtime.activity_log, &log_msg).await;
            }
            Err(e) => {
                notice = Some(buy_notice(e));
            }
        }
    }

    let selected = if bought {
        let bought_slot = bought_slot.expect("bought implies bought_slot");
        let unsold = merchant.unsold_slots();
        if unsold.is_empty() {
            selected_slot
        } else {
            unsold
                .iter()
                .copied()
                .find(|&slot| slot > bought_slot)
                .unwrap_or(unsold[0])
        }
    } else {
        selected_slot
    };

    let visit = merchant
        .visit
        .as_ref()
        .cloned()
        .unwrap_or(visit);

    Ok(build_shop_message(
        &visit,
        &player,
        selected,
        notice.as_deref(),
    ))
}

pub async fn handle_merchant_shop_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let merchant = data.runtime.merchant.lock().await;
    let selected = merchant.first_unsold_slot().unwrap_or(0);
    drop(merchant);

    let message = prepare_shop_response(data, user_id, selected, None).await?;
    respond_ephemeral_create(&ctx.http, interaction, &message).await;
    Ok(())
}

pub async fn handle_shop_select(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let selected = match &interaction.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values
            .first()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        _ => 0,
    };

    let message = prepare_shop_response(data, user_id, selected, None).await?;
    respond_ephemeral_update(&ctx.http, interaction, &message).await;
    Ok(())
}

pub async fn handle_shop_buy(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let custom_id = &interaction.data.custom_id;
    let slot: usize = custom_id
        .strip_prefix("shop_buy:")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let message = prepare_shop_response(data, user_id, slot, Some(slot)).await?;
    respond_ephemeral_update(&ctx.http, interaction, &message).await;
    Ok(())
}
