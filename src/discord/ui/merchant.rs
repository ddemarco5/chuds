use poise::serenity_prelude::{
    self as serenity, ComponentInteraction, ComponentInteractionDataKind, Http, MessageId,
};

use crate::chud_msg;
use crate::discord::channel::append_activity_log;
use crate::discord::formatting::format_item_block;
use crate::game::domain::player::Player;
use crate::game::merchant::{BuyError, MerchantState, MerchantVisit};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::message_cache::MessageCache;
use crate::game::persistence::storage;

use super::super::components_v2::{
    components_v2_flags, ActionRow, Button, Component, ComponentsV2Message, ContainerChild,
    SelectOption, StringSelect, TextDisplay,
};
use super::super::context::Data;
use super::{no_chud_message, push_status_notice, respond_ephemeral_create, respond_ephemeral_update};

fn merchant_visit_identity(visit: &MerchantVisit) -> String {
    format!("{}:{}", visit.merchant_index, visit.merchant_name)
}

/// Shop title with merchant theme as Discord subtext (`-#`).
fn format_shop_header(name: &str, theme: &str) -> String {
    let theme_line = theme.split_whitespace().collect::<Vec<_>>().join(" ");
    if theme_line.is_empty() {
        format!("**{name}**")
    } else {
        format!("**{name}**\n-# {theme_line}")
    }
}

/// Pick or reuse a random visiting announcement. A new line is chosen only when a visit
/// starts or a different merchant replaces the current one.
fn resolve_merchant_visit_text(
    merchant: &MerchantState,
    cache: &mut MessageCache,
) -> (Option<String>, bool) {
    let mut dirty = false;
    match &merchant.visit {
        None => {
            if cache.merchant_visit_text.is_some() || cache.merchant_visit_identity.is_some() {
                cache.merchant_visit_text = None;
                cache.merchant_visit_identity = None;
                dirty = true;
            }
            (None, dirty)
        }
        Some(visit) => {
            let identity = merchant_visit_identity(visit);
            if cache.merchant_visit_identity.as_deref() != Some(identity.as_str()) {
                cache.merchant_visit_text =
                    Some(chud_msg!("merchant_visiting", &visit.merchant_name));
                cache.merchant_visit_identity = Some(identity);
                dirty = true;
            }
            (cache.merchant_visit_text.clone(), dirty)
        }
    }
}

fn build_merchant_channel_message(visit_text: Option<&str>) -> ComponentsV2Message {
    match visit_text {
        None => ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(
            "\u{200B}\n\u{200B}",
        ))]),
        Some(text) => {
            let mut inner = vec![ContainerChild::Text(TextDisplay::new(text))];
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

    let (visit_text, header_dirty) = resolve_merchant_visit_text(merchant, &mut cache);
    let message = build_merchant_channel_message(visit_text.as_deref());
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

    if cache_dirty || header_dirty {
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache");
        }
    }
    Ok(())
}

pub fn build_shop_message(
    visit: &MerchantVisit,
    merchant_theme: &str,
    registry: &ItemRegistry,
    player: &Player,
    selected_slot: usize,
    notice: Option<&str>,
) -> ComponentsV2Message {
    let mut components = Vec::new();

    components.push(Component::Text(TextDisplay::new(format_shop_header(
        &visit.merchant_name,
        merchant_theme,
    ))));
    components.push(Component::Text(TextDisplay::new(format!(
        "You have ${} on hand",
        player.cash
    ))));

    let available: Vec<usize> = (0..visit.stock.len())
        .filter(|&slot| registry.get(visit.stock[slot]).is_some())
        .collect();

    if available.is_empty() {
        components.push(Component::Text(TextDisplay::new("_The merchant is sold out._")));
        push_status_notice(&mut components, notice);
        return ComponentsV2Message {
            flags: components_v2_flags(),
            components,
        };
    }

    let selected = if available.contains(&selected_slot) {
        selected_slot
    } else {
        available[0]
    };

    for &slot in &available {
        if let Some(item) = registry.get(visit.stock[slot]) {
            components.push(Component::Text(TextDisplay::new(format_item_block(item))));
        }
    }

    let select_options: Vec<SelectOption> = available
        .iter()
        .map(|&slot| {
            let name = registry
                .get(visit.stock[slot])
                .map(|item| item.name.clone())
                .unwrap_or_else(|| "Unknown item".into());
            let base = SelectOption::new(name, slot.to_string());
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

    let price = registry
        .get(visit.stock[selected])
        .map(|item| item.value)
        .unwrap_or(0);
    components.push(Component::ActionRow(ActionRow::one_button(
        Button::primary(format!("shop_buy:{selected}"), format!("Buy ${price}")),
    )));

    push_status_notice(&mut components, notice);

    ComponentsV2Message {
        flags: components_v2_flags(),
        components,
    }
}

fn buy_notice(err: BuyError) -> String {
    match err {
        BuyError::SoldOut => "Sorry, someone snagged that.".into(),
        BuyError::InsufficientFunds => "You don't have enough cash.".into(),
        BuyError::StashFull => "Your stash is full.".into(),
        BuyError::InvalidSlot => "That item is no longer available.".into(),
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

    let registry = data.runtime.item_registry.lock().await;
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
        match merchant.buy(slot, &mut player, &registry) {
            Ok(item) => {
                notice = Some(format!("Bought {} for ${}.", item.name, item.value));
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
        let stock_len = merchant
            .visit
            .as_ref()
            .map(|v| v.stock.len())
            .unwrap_or(0);
        if stock_len == 0 {
            selected_slot
        } else {
            (0..stock_len)
                .find(|&slot| slot > bought_slot)
                .unwrap_or(0)
        }
    } else {
        selected_slot
    };

    let visit = merchant
        .visit
        .as_ref()
        .cloned()
        .unwrap_or(visit);

    let merchant_theme = merchant
        .catalog
        .as_ref()
        .and_then(|c| c.merchants.get(visit.merchant_index))
        .map(|m| m.theme.as_str())
        .unwrap_or("");

    Ok(build_shop_message(
        &visit,
        merchant_theme,
        &registry,
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
    let selected = merchant.first_stock_slot().unwrap_or(0);
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
