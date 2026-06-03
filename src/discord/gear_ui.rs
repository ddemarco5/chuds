use poise::serenity_prelude::{self as serenity, ComponentInteraction, Http};

use crate::game::busy::{self, BusyReason};
use crate::game::domain::item::{EquipmentSlot, Item};
use crate::game::domain::player::Player;
use crate::game::domain::stash::STASH_CAPACITY;
use crate::game::engine;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;

use super::components_v2::{
    components_v2_flags, ActionRow, Button, Component, ComponentsV2Message, InteractionUpdateResponse,
    Separator, TextDisplay,
};
use super::context::Data;

fn format_item_block(item: &Item) -> String {
    let subtype = if item.subtype.is_empty() {
        String::new()
    } else {
        format!(" ({})", item.subtype)
    };
    format!(
        "**{}**{} [{}] - ${}\n{}",
        item.name,
        subtype,
        item.stats.format_triplet(),
        item.value,
        item.description,
    )
}

fn format_equipped_block(slot: EquipmentSlot, item: Option<&Item>) -> String {
    match item {
        Some(item) => format!("**{}**\n{}", slot.label(), format_item_block(item)),
        None => format!("**{}**\n*(empty)*", slot.label()),
    }
}

fn sell_button(item_id: u32, value: u32, confirm_pending: bool) -> Button {
    if confirm_pending {
        Button::danger(format!("g_sell_confirm:{item_id}"), "Are you sure?")
    } else {
        Button::danger(format!("g_sell:{item_id}"), format!("Sell ${value}"))
    }
}

fn item_action_buttons(
    item_id: u32,
    value: u32,
    sell_confirm_item_id: Option<u32>,
    primary: Button,
) -> Vec<Button> {
    vec![
        primary,
        sell_button(item_id, value, sell_confirm_item_id == Some(item_id)),
    ]
}

fn push_entry(components: &mut Vec<Component>, text: impl Into<String>, buttons: Vec<Button>) {
    components.push(Component::Text(TextDisplay::new(text)));
    if !buttons.is_empty() {
        components.push(Component::ActionRow(ActionRow::buttons(buttons)));
    }
}

pub fn build_gear_message(
    player: &Player,
    registry: &ItemRegistry,
    notice: Option<&str>,
    sell_confirm_item_id: Option<u32>,
) -> ComponentsV2Message {
    let mut components = Vec::new();

    if let Some(notice) = notice {
        components.push(Component::Text(TextDisplay::new(notice)));
    }

    components.push(Component::Text(TextDisplay::new(format!(
        "**Equipped** — ${} on hand",
        player.cash
    ))));

    for slot in EquipmentSlot::ALL {
        let item = player
            .chud
            .equipment
            .item_id_in_slot(slot)
            .and_then(|id| registry.get(id));
        let buttons = item.map(|item| {
            item_action_buttons(
                item.id,
                item.value,
                sell_confirm_item_id,
                Button::secondary(
                    format!("g_unequip:{}", slot.custom_id_suffix()),
                    "Unequip",
                ),
            )
        }).unwrap_or_default();
        push_entry(
            &mut components,
            format_equipped_block(slot, item),
            buttons,
        );
    }

    components.push(Component::Separator(Separator::section()));
    components.push(Component::Text(TextDisplay::new(format!(
        "**Stash** ({}/{})",
        player.stash.len(),
        STASH_CAPACITY
    ))));

    if player.stash.is_empty() {
        components.push(Component::Text(TextDisplay::new("*(empty)*")));
    } else {
        for &item_id in player.stash.items() {
            match registry.get(item_id) {
                Some(item) => {
                    push_entry(
                        &mut components,
                        format_item_block(item),
                        item_action_buttons(
                            item_id,
                            item.value,
                            sell_confirm_item_id,
                            Button::secondary(format!("g_equip:{item_id}"), "Equip"),
                        ),
                    );
                }
                None => {
                    tracing::warn!(item_id, "stash item missing from registry");
                }
            }
        }
    }

    ComponentsV2Message {
        flags: components_v2_flags(),
        components,
    }
}

fn busy_notice(reason: BusyReason, player: &Player) -> String {
    match reason {
        BusyReason::ActiveQuest { quest_title } => {
            format!("_**{}** is on the job \"{}\"._", player.name, quest_title)
        }
        BusyReason::Scouting => format!("_**{}** is out scouting._", player.name),
        BusyReason::Hospitalized => format!("_**{}** is in the hospital._", player.name),
    }
}

/// `(notice, item_id awaiting sell confirmation)`
fn apply_gear_action(
    player: &mut Player,
    custom_id: &str,
    registry: &mut ItemRegistry,
) -> (Option<String>, Option<u32>) {
    if let Some(suffix) = custom_id.strip_prefix("g_unequip:") {
        let slot = match EquipmentSlot::parse_suffix(suffix) {
            Some(slot) => slot,
            None => return (Some("_Unknown action._".into()), None),
        };
        let item_name = player
            .chud
            .equipment
            .item_id_in_slot(slot)
            .and_then(|id| registry.get(id))
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "item".into());
        let notice = match engine::unequip_slot(player, slot) {
            Ok(()) => Some(format!("_Unequipped **{item_name}**._")),
            Err(e) if e.to_string().contains("stash is full") => {
                Some(format!("_Stash is full ({STASH_CAPACITY}/{STASH_CAPACITY})._"))
            }
            Err(e) if e.to_string().contains("slot is empty") => {
                Some("_That slot is already empty._".into())
            }
            Err(e) => {
                tracing::warn!(err = %e, "unequip failed");
                Some("_Could not unequip that item._".into())
            }
        };
        return (notice, None);
    }

    if let Some(id_str) = custom_id.strip_prefix("g_equip:") {
        let item_id: u32 = match id_str.parse() {
            Ok(id) => id,
            Err(_) => return (Some("_Unknown action._".into()), None),
        };
        let item_name = registry
            .get(item_id)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "item".into());
        let notice = match engine::equip_from_stash(player, item_id, registry) {
            Ok(()) => Some(format!("_Equipped **{item_name}**._")),
            Err(e) if e.to_string().contains("not in stash") => {
                Some("_That item is no longer in your stash._".into())
            }
            Err(e) if e.to_string().contains("stash is full") => {
                Some(format!("_Stash is full ({STASH_CAPACITY}/{STASH_CAPACITY})._"))
            }
            Err(e) => {
                tracing::warn!(err = %e, "equip failed");
                Some("_Could not equip that item._".into())
            }
        };
        return (notice, None);
    }

    if let Some(id_str) = custom_id.strip_prefix("g_sell:") {
        let item_id: u32 = match id_str.parse() {
            Ok(id) => id,
            Err(_) => return (Some("_Unknown action._".into()), None),
        };
        if registry.get(item_id).is_none() {
            return (Some("_That item no longer exists._".into()), None);
        }
        if !player.stash.contains(item_id)
            && !player
                .chud
                .equipment
                .all_ids()
                .any(|id| id == item_id)
        {
            return (Some("_You don't have that item._".into()), None);
        }
        return (None, Some(item_id));
    }

    if let Some(id_str) = custom_id.strip_prefix("g_sell_confirm:") {
        let item_id: u32 = match id_str.parse() {
            Ok(id) => id,
            Err(_) => return (Some("_Unknown action._".into()), None),
        };
        let item_name = registry
            .get(item_id)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "item".into());
        let notice = match engine::sell_item(player, registry, item_id) {
            Ok(gold) => Some(format!("_Sold **{item_name}** for ${gold}._")),
            Err(e) if e.to_string().contains("not owned") => {
                Some("_You don't have that item._".into())
            }
            Err(e) if e.to_string().contains("not found") => {
                Some("_That item no longer exists._".into())
            }
            Err(e) => {
                tracing::warn!(err = %e, "sell failed");
                Some("_Could not sell that item._".into())
            }
        };
        return (notice, None);
    }

    (None, None)
}

/// Build the updated gear UI under board/registry locks, then drop them before any HTTP I/O.
async fn prepare_gear_update(
    data: &Data,
    player: &mut Player,
    custom_id: &str,
) -> anyhow::Result<ComponentsV2Message> {
    let hospital = storage::load_hospital()?;
    // Match lock order used elsewhere (board before item_registry) to avoid deadlocks.
    let board = data.board.lock().await;
    let mut registry = data.item_registry.lock().await;

    let (notice, sell_confirm) =
        if let Some(reason) = busy::is_player_busy(&*board, &hospital, player.discord_user_id) {
            (Some(busy_notice(reason, player)), None)
        } else {
            apply_gear_action(player, custom_id, &mut registry)
        };

    Ok(build_gear_message(
        player,
        &registry,
        notice.as_deref(),
        sell_confirm,
    ))
}

pub async fn edit_gear_message(
    http: &Http,
    interaction_token: &str,
    message: &ComponentsV2Message,
) -> anyhow::Result<()> {
    http.edit_original_interaction_response(interaction_token, message, vec![])
        .await?;
    Ok(())
}

pub async fn handle_gear_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let custom_id = interaction.data.custom_id.clone();
    let http = &ctx.http;

    let mut player = match storage::load_player(user_id)? {
        Some(p) => p,
        None => {
            let payload = InteractionUpdateResponse::update(ComponentsV2Message {
                flags: components_v2_flags(),
                components: vec![Component::Text(TextDisplay::new(
                    "You don't have a chud.",
                ))],
            });
            if let Err(e) = http
                .create_interaction_response(interaction.id, &interaction.token, &payload, vec![])
                .await
            {
                tracing::debug!(err = %e, "gear button response failed (ephemeral may be dismissed)");
            }
            return Ok(());
        }
    };

    let message = prepare_gear_update(data, &mut player, &custom_id).await?;

    if let Err(e) = http
        .create_interaction_response(
            interaction.id,
            &interaction.token,
            &InteractionUpdateResponse::update(message),
            vec![],
        )
        .await
    {
        // Expected when the user dismissed the ephemeral or the interaction token expired.
        tracing::debug!(err = %e, user_id, "gear button update failed (ephemeral may be dismissed)");
    }

    Ok(())
}
