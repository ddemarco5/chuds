use poise::serenity_prelude::{self as serenity, ComponentInteraction};

use crate::chud_msg;
use crate::discord::channel::append_activity_log;
use crate::game::domain::player::Player;
use crate::game::persistence::storage;
use crate::game::slots::{self, SlotsError, SlotsSpin};

use super::super::components_v2::{
    components_v2_flags, ActionRow, Button, Component, ComponentsV2Message, TextDisplay,
};
use super::super::context::Data;
use super::{no_chud_message, push_status_notice, respond_ephemeral_update};

pub const PLAYER_SPIN_PREFIX: &str = "slots:spin";
pub const ADMIN_SPIN_PREFIX: &str = "admin_slots:spin";

const MAX_PAY_IN: u32 = 1000;

pub fn validate_pay_in(pay_in: u32) -> Result<(), &'static str> {
    if pay_in < 1 {
        Err("pay-in must be at least $1")
    } else if pay_in > MAX_PAY_IN {
        Err("pay-in must be at most $1000")
    } else {
        Ok(())
    }
}

fn format_symbol_values(pay_in: u32) -> String {
    let values = slots::pay_table_5x_row(pay_in)
        .iter()
        .map(|(symbol, value)| format!("{symbol} ${value}"))
        .collect::<Vec<_>>()
        .join("  ");
    format!("-# {values}")
}

fn format_reels(reels: Option<&[char]>) -> String {
    match reels {
        None => "| ? ? ? ? ? |".into(),
        Some(symbols) => {
            let inner = symbols
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            format!("| {inner} |")
        }
    }
}

fn format_result(spin: &SlotsSpin, cash: Option<u32>) -> String {
    let net = spin.payout as i64 - spin.pay_in as i64;
    let mut headline = if net > 0 {
        format!("**+${net}** (won ${})", spin.payout)
    } else if net < 0 {
        format!("**-${}** (no win)", spin.pay_in)
    } else {
        "**break even**".into()
    };

    if let Some(cash) = cash {
        headline = format!("{headline} · ${cash} on hand");
    }

    if spin.wins.is_empty() {
        return headline;
    }

    let lines: Vec<String> = spin
        .wins
        .iter()
        .map(|(symbol, count, payout)| format!("{count}x {symbol} → ${payout}"))
        .collect();
    format!("{headline}\n{}", lines.join("\n"))
}

pub fn build_slots_message(
    pay_in: u32,
    cash: Option<u32>,
    last_spin: Option<&SlotsSpin>,
    notice: Option<&str>,
    spin_button_prefix: &str,
) -> ComponentsV2Message {
    let mut components = vec![
        Component::Text(TextDisplay::new("**Slots**")),
        Component::Text(TextDisplay::new(format_symbol_values(pay_in))),
        Component::Text(TextDisplay::new(format_reels(
            last_spin.map(|s| s.reels.as_slice()),
        ))),
    ];

    if let Some(spin) = last_spin {
        components.push(Component::Text(TextDisplay::new(format_result(
            spin,
            cash,
        ))));
    } else if let Some(cash) = cash {
        components.push(Component::Text(TextDisplay::new(format!(
            "You have ${cash} on hand"
        ))));
    }

    components.push(Component::ActionRow(ActionRow::one_button(Button::primary(
        format!("{spin_button_prefix}:{pay_in}"),
        format!("Spin ${pay_in}"),
    ))));

    push_status_notice(&mut components, notice);

    ComponentsV2Message {
        flags: components_v2_flags(),
        components,
    }
}

fn parse_spin_custom_id(custom_id: &str) -> Option<(bool, u32)> {
    if let Some(rest) = custom_id.strip_prefix(ADMIN_SPIN_PREFIX) {
        let pay_in = rest.strip_prefix(':')?.parse().ok()?;
        return Some((true, pay_in));
    }
    if let Some(rest) = custom_id.strip_prefix(PLAYER_SPIN_PREFIX) {
        let pay_in = rest.strip_prefix(':')?.parse().ok()?;
        return Some((false, pay_in));
    }
    None
}

fn insufficient_funds_notice() -> &'static str {
    "You don't have enough cash."
}

pub async fn handle_slots_spin(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let custom_id = &interaction.data.custom_id;
    let Some((admin, pay_in)) = parse_spin_custom_id(custom_id) else {
        return Ok(());
    };

    if admin {
        let message = match validate_pay_in(pay_in) {
            Err(msg) => build_slots_message(pay_in.max(1), None, None, Some(msg), ADMIN_SPIN_PREFIX),
            Ok(()) => {
                let spin = slots::spin(pay_in);
                build_slots_message(pay_in, None, Some(&spin), None, ADMIN_SPIN_PREFIX)
            }
        };
        respond_ephemeral_update(&ctx.http, interaction, &message).await;
        return Ok(());
    }

    let mut player = match storage::load_player(user_id)? {
        Some(p) if p.has_chud() => p,
        _ => {
            respond_ephemeral_update(&ctx.http, interaction, &no_chud_message()).await;
            return Ok(());
        }
    };

    let mut episode_stats = storage::load_episode_stats()?;

    let message = match validate_pay_in(pay_in) {
        Err(msg) => build_slots_message(
            pay_in.max(1),
            Some(player.cash),
            None,
            Some(msg),
            PLAYER_SPIN_PREFIX,
        ),
        Ok(()) => match slots::spin_for_player(&mut player, pay_in) {
            Ok(spin) => {
                let chud_name = player.chud_ref().name.clone();
                episode_stats.record_gambling_spin(
                    user_id,
                    &chud_name,
                    spin.pay_in,
                    spin.payout,
                );
                storage::save_episode_stats(&episode_stats)?;

                if spin.payout > 0 {
                    let first_name = chud_name
                        .split_whitespace()
                        .next()
                        .unwrap_or(&chud_name)
                        .to_string();
                    let trace_msg = chud_msg!("chud_wins_slots", first_name, spin.payout);
                    tracing::info!(
                        user_id,
                        pay_in,
                        payout = spin.payout,
                        mini_jackpot = ?spin.mini_jackpot_symbol(),
                        "{trace_msg}",
                    );
                    if spin.mini_jackpot_symbol().is_some() {
                        let log_msg = chud_msg!("chud_jackpot_slots", first_name, spin.payout);
                        append_activity_log(&data.runtime.activity_log, &log_msg).await;
                    }
                }
                build_slots_message(
                    pay_in,
                    Some(player.cash),
                    Some(&spin),
                    None,
                    PLAYER_SPIN_PREFIX,
                )
            }
            Err(SlotsError::InsufficientFunds) => build_slots_message(
                pay_in,
                Some(player.cash),
                None,
                Some(insufficient_funds_notice()),
                PLAYER_SPIN_PREFIX,
            ),
        },
    };

    respond_ephemeral_update(&ctx.http, interaction, &message).await;
    Ok(())
}

pub fn build_player_slots_open_message(player: &Player, pay_in: u32) -> ComponentsV2Message {
    build_slots_message(
        pay_in,
        Some(player.cash),
        None,
        None,
        PLAYER_SPIN_PREFIX,
    )
}
