use poise::serenity_prelude::{self as serenity, ComponentInteraction};

use crate::chud_msg;
use crate::discord::channel::append_activity_log;
use crate::discord::formatting::format_item_block;
use crate::game::busy::{self, BusyReason};
use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::item::EquipmentSlot;
use crate::game::domain::session::GamePhase;
use crate::game::domain::player::Player;
use crate::game::domain::stash::STASH_CAPACITY;
use crate::game::engine;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;

use super::super::components_v2::{
    components_v2_flags, ActionRow, Button, Component, ComponentsV2Message, Separator,
    TextDisplay,
};
use super::super::context::Data;
use super::{no_chud_message, push_status_notice, respond_ephemeral_update};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GearInteractionMode {
    Full,
    EquipmentLocked,
    ReadOnly,
}

pub fn derive_gear_mode(read_only: bool, busy_reason: Option<BusyReason>) -> GearInteractionMode {
    if read_only {
        GearInteractionMode::ReadOnly
    } else if busy_reason.is_some() {
        GearInteractionMode::EquipmentLocked
    } else {
        GearInteractionMode::Full
    }
}

pub fn gear_equipment_locked_notice(reason: BusyReason, player: &Player) -> String {
    let name = &player.chud_ref().name;
    let status = match reason {
        BusyReason::ActiveQuest { quest_title } => format!("on the job \"{quest_title}\""),
        BusyReason::Scouting => "out scouting".into(),
        BusyReason::Hospitalized => "in the hospital".into(),
    };
    chud_msg!("gear_equipment_locked", name, status)
}

fn gear_persistent_notice(
    read_only: bool,
    busy_reason: Option<BusyReason>,
    player: &Player,
) -> Option<String> {
    if read_only {
        Some("The game is over — your loadout is locked.".into())
    } else if let Some(reason) = busy_reason {
        Some(gear_equipment_locked_notice(reason, player))
    } else {
        None
    }
}

/// Build the gear UI for `/gear` or other first-open paths (no button action).
pub async fn build_gear_open_message(
    data: &Data,
    player: &Player,
) -> anyhow::Result<ComponentsV2Message> {
    let hospital = storage::load_hospital()?;
    let read_only = data.runtime.session.lock().await.phase == GamePhase::Complete;
    let board = data.runtime.board.lock().await;
    let busy_reason = busy::is_player_busy(&*board, &hospital, player.discord_user_id);
    let mode = derive_gear_mode(read_only, busy_reason.clone());
    let notice = gear_persistent_notice(read_only, busy_reason, player);
    let registry = data.runtime.item_registry.lock().await;
    Ok(build_gear_message(
        player,
        &registry,
        notice.as_deref(),
        None,
        mode,
    ))
}

fn format_equipped_block(slot: EquipmentSlot, item: Option<&crate::game::domain::item::Item>) -> String {
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
    mode: GearInteractionMode,
) -> ComponentsV2Message {
    let mut components = Vec::new();

    components.push(Component::Text(TextDisplay::new(format!(
        "**Equipped** — ${} on hand",
        player.cash
    ))));

    for slot in EquipmentSlot::ALL {
        let item = player
            .chud_ref()
            .equipment
            .item_id_in_slot(slot)
            .and_then(|id| registry.get(id));
        let buttons = match mode {
            GearInteractionMode::ReadOnly | GearInteractionMode::EquipmentLocked => Vec::new(),
            GearInteractionMode::Full => item
                .map(|item| {
                    item_action_buttons(
                        item.id,
                        item.value,
                        sell_confirm_item_id,
                        Button::secondary(
                            format!("g_unequip:{}", slot.custom_id_suffix()),
                            "Unequip",
                        ),
                    )
                })
                .unwrap_or_default(),
        };
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
                    let buttons = match mode {
                        GearInteractionMode::ReadOnly => Vec::new(),
                        GearInteractionMode::EquipmentLocked => vec![sell_button(
                            item_id,
                            item.value,
                            sell_confirm_item_id == Some(item_id),
                        )],
                        GearInteractionMode::Full => item_action_buttons(
                            item_id,
                            item.value,
                            sell_confirm_item_id,
                            Button::secondary(format!("g_equip:{item_id}"), "Equip"),
                        ),
                    };
                    push_entry(&mut components, format_item_block(item), buttons);
                }
                None => {
                    tracing::warn!(item_id, "stash item missing from registry");
                }
            }
        }
    }

    push_status_notice(&mut components, notice);

    ComponentsV2Message {
        flags: components_v2_flags(),
        components,
    }
}

fn equipment_locked_action_notice(reason: BusyReason, player: &Player) -> String {
    gear_equipment_locked_notice(reason, player)
}

/// `(notice, item_id awaiting sell confirmation, activity log line)`
fn apply_gear_action(
    player: &mut Player,
    custom_id: &str,
    registry: &mut ItemRegistry,
    mode: GearInteractionMode,
    board: &Board,
    hospital: &Hospital,
    busy_reason: Option<BusyReason>,
) -> (Option<String>, Option<u32>, Option<String>) {
    if let Some(suffix) = custom_id.strip_prefix("g_unequip:") {
        if mode == GearInteractionMode::EquipmentLocked {
            return (
                busy_reason
                    .clone()
                    .map(|reason| equipment_locked_action_notice(reason, player)),
                None,
                None,
            );
        }
        let slot = match EquipmentSlot::parse_suffix(suffix) {
            Some(slot) => slot,
            None => return (Some("Unknown action.".into()), None, None),
        };
        let item_name = player
            .chud_ref()
            .equipment
            .item_id_in_slot(slot)
            .and_then(|id| registry.get(id))
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "item".into());
        let notice = match engine::unequip_slot(board, hospital, player, slot) {
            Ok(()) => Some(format!("Unequipped {item_name}.")),
            Err(e) if e.to_string().contains("equipment locked") => busy_reason
                .clone()
                .map(|reason| equipment_locked_action_notice(reason, player)),
            Err(e) if e.to_string().contains("stash is full") => {
                Some(format!("Stash is full ({STASH_CAPACITY}/{STASH_CAPACITY})."))
            }
            Err(e) if e.to_string().contains("slot is empty") => {
                Some("That slot is already empty.".into())
            }
            Err(e) => {
                tracing::warn!(err = %e, "unequip failed");
                Some("Could not unequip that item.".into())
            }
        };
        return (notice, None, None);
    }

    if let Some(id_str) = custom_id.strip_prefix("g_equip:") {
        if mode == GearInteractionMode::EquipmentLocked {
            return (
                busy_reason
                    .clone()
                    .map(|reason| equipment_locked_action_notice(reason, player)),
                None,
                None,
            );
        }
        let item_id: u32 = match id_str.parse() {
            Ok(id) => id,
            Err(_) => return (Some("Unknown action.".into()), None, None),
        };
        let item_name = registry
            .get(item_id)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "item".into());
        let notice = match engine::equip_from_stash(board, hospital, player, item_id, registry) {
            Ok(()) => Some(format!("Equipped {item_name}.")),
            Err(e) if e.to_string().contains("equipment locked") => busy_reason
                .clone()
                .map(|reason| equipment_locked_action_notice(reason, player)),
            Err(e) if e.to_string().contains("not in stash") => {
                Some("That item is no longer in your stash.".into())
            }
            Err(e) if e.to_string().contains("stash is full") => {
                Some(format!("Stash is full ({STASH_CAPACITY}/{STASH_CAPACITY})."))
            }
            Err(e) => {
                tracing::warn!(err = %e, "equip failed");
                Some("Could not equip that item.".into())
            }
        };
        return (notice, None, None);
    }

    if let Some(id_str) = custom_id.strip_prefix("g_sell:") {
        let item_id: u32 = match id_str.parse() {
            Ok(id) => id,
            Err(_) => return (Some("Unknown action.".into()), None, None),
        };
        if registry.get(item_id).is_none() {
            return (Some("That item no longer exists.".into()), None, None);
        }
        if !player.stash.contains(item_id)
            && !player
                .chud_ref()
                .equipment
                .all_ids()
                .any(|id| id == item_id)
        {
            return (Some("You don't have that item.".into()), None, None);
        }
        if mode == GearInteractionMode::EquipmentLocked && engine::is_item_equipped(player, item_id) {
            return (
                busy_reason
                    .clone()
                    .map(|reason| equipment_locked_action_notice(reason, player)),
                None,
                None,
            );
        }
        return (None, Some(item_id), None);
    }

    if let Some(id_str) = custom_id.strip_prefix("g_sell_confirm:") {
        let item_id: u32 = match id_str.parse() {
            Ok(id) => id,
            Err(_) => return (Some("Unknown action.".into()), None, None),
        };
        if mode == GearInteractionMode::EquipmentLocked && engine::is_item_equipped(player, item_id) {
            return (
                busy_reason
                    .clone()
                    .map(|reason| equipment_locked_action_notice(reason, player)),
                None,
                None,
            );
        }
        let item_name = registry
            .get(item_id)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "item".into());
        let chud_name = player.chud_ref().name.clone();
        let first_name = chud_name
            .split_whitespace()
            .next()
            .unwrap_or(&chud_name)
            .to_string();
        let (notice, activity_log) = match engine::sell_item(board, hospital, player, registry, item_id)
        {
            Ok(gold) => (
                Some(format!("Sold {item_name} for ${gold}.")),
                Some(chud_msg!("chud_sells_item", first_name, item_name)),
            ),
            Err(e) if e.to_string().contains("equipment locked") => (
                busy_reason
                    .clone()
                    .map(|reason| equipment_locked_action_notice(reason, player)),
                None,
            ),
            Err(e) if e.to_string().contains("not owned") => {
                (Some("You don't have that item.".into()), None)
            }
            Err(e) if e.to_string().contains("not found") => {
                (Some("That item no longer exists.".into()), None)
            }
            Err(e) => {
                tracing::warn!(err = %e, "sell failed");
                (Some("Could not sell that item.".into()), None)
            }
        };
        return (notice, None, activity_log);
    }

    (None, None, None)
}

/// Build the updated gear UI under board/registry locks, then drop them before any HTTP I/O.
async fn prepare_gear_update(
    data: &Data,
    player: &mut Player,
    custom_id: &str,
) -> anyhow::Result<(ComponentsV2Message, Option<String>)> {
    let hospital = storage::load_hospital()?;
    let read_only = data.runtime.session.lock().await.phase == GamePhase::Complete;
    // Match lock order used elsewhere (board before item_registry) to avoid deadlocks.
    let board = data.runtime.board.lock().await;
    let mut registry = data.runtime.item_registry.lock().await;

    let busy_reason = busy::is_player_busy(&*board, &hospital, player.discord_user_id);
    let mode = derive_gear_mode(read_only, busy_reason.clone());

    let persistent_notice = gear_persistent_notice(read_only, busy_reason.clone(), player);

    let (action_notice, sell_confirm, activity_log) = if read_only {
        (None, None, None)
    } else {
        apply_gear_action(
            player,
            custom_id,
            &mut registry,
            mode,
            &*board,
            &hospital,
            busy_reason,
        )
    };

    let notice = action_notice.or(persistent_notice);

    Ok((
        build_gear_message(
            player,
            &registry,
            notice.as_deref(),
            sell_confirm,
            mode,
        ),
        activity_log,
    ))
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
        Some(p) if p.has_chud() => p,
        _ => {
            respond_ephemeral_update(http, interaction, &no_chud_message()).await;
            return Ok(());
        }
    };

    let (message, activity_log) = prepare_gear_update(data, &mut player, &custom_id).await?;
    if let Some(log_msg) = activity_log {
        append_activity_log(&data.runtime.activity_log, &log_msg).await;
    }
    respond_ephemeral_update(http, interaction, &message).await;

    Ok(())
}
