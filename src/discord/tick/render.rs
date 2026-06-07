use poise::serenity_prelude::CreateMessage;

use crate::chud_msg;
use crate::discord::channel::{append_activity_log_deferred, sync_activity_log_now};
use crate::discord::context::GameRuntime;
use crate::discord::formatting;
use crate::discord::guild_hall;
use crate::discord::report_dm;
use crate::discord::ui::update_merchant_message;
use crate::game::domain::board::Board;
use crate::game::engine;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::message_cache::ActivityLogKind;
use crate::game::persistence::storage;
use crate::game::tick::TickOutcome;

/// Render a completed [`TickOutcome`] to Discord: activity log, DMs, board, merchant.
///
/// `board` and `item_registry` are the already-locked tick guards; the merchant lock is
/// taken internally.
pub async fn render_tick_outcome(
    runtime: &GameRuntime,
    outcome: &TickOutcome,
    board: &mut Board,
    registry: &mut ItemRegistry,
) -> anyhow::Result<()> {
    let http = &runtime.http;
    let activity_log = &runtime.activity_log;
    let channel_id = runtime.channel_id;
    let max_jobs = runtime.max_jobs;

    for msg in &outcome.hospital_releases {
        append_activity_log_deferred(activity_log, ActivityLogKind::Standard, msg).await;
    }

    for qr in &outcome.quest_resolved {
        tracing::info!(quest = %qr.quest_title, passed = qr.result.passed, player = %qr.player_name, "quest resolved");
        let content = if qr.level_up.any() {
            let mut adjs: Vec<&str> = Vec::new();
            if qr.level_up.str_up {
                adjs.push("stronger");
            }
            if qr.level_up.smt_up {
                adjs.push("smarter");
            }
            if qr.level_up.sth_up {
                adjs.push("sneakier");
            }
            if qr.level_up.exp_up {
                adjs.push("more experienced");
            }
            let adj_str = match adjs.len() {
                1 => adjs[0].to_string(),
                2 => format!("{} and {}", adjs[0], adjs[1]),
                _ => {
                    let (last, rest) = adjs.split_last().unwrap();
                    format!("{}, and {}", rest.join(", "), last)
                }
            };
            format!("{}\n{} seems {}.", qr.summary, qr.player_name, adj_str)
        } else {
            qr.summary.clone()
        };
        append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &content).await;

        let first_name = qr
            .player_name
            .split_whitespace()
            .next()
            .unwrap_or(&qr.player_name)
            .to_string();
        let return_key = if qr.died {
            "return_died"
        } else if qr.hospitalized {
            "return_hospitalized"
        } else if qr.result.passed {
            "return_passed"
        } else {
            "return_failed"
        };
        let return_msg = chud_msg!(return_key, first_name);
        append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &return_msg).await;

        if qr.died {
            if let Some(ctx) = &qr.death_ctx {
                let mut graveyard = storage::load_graveyard()?;
                let mut starting_benefits = storage::load_starting_benefits()?;
                let kill = engine::kill_chud(
                    qr.discord_user_id,
                    ctx,
                    &mut graveyard,
                    &mut starting_benefits,
                    registry,
                    &runtime.gravestone_generator,
                )
                .await?;
                report_dm::send_death_dm(http, &kill, Some(&qr.summary)).await;
                continue;
            }
        }

        report_dm::send_job_completion_dm(http, registry, qr).await;
    }

    for sr in &outcome.scout_results {
        let first_name = sr
            .player_name
            .split_whitespace()
            .next()
            .unwrap_or(&sr.player_name)
            .to_string();
        let scout_return_msg =
            chud_msg!(formatting::scout_returned_message_key(sr.chance), first_name);
        append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &scout_return_msg).await;

        let active_player_name: Option<String> = sr
            .active_discord_user_id
            .and_then(|id| storage::load_player(id).ok().flatten())
            .filter(|p| p.has_chud())
            .map(|p| p.chud_ref().name.clone());
        let scouting_player_names: Vec<String> = sr
            .other_scouting_discord_user_ids
            .iter()
            .filter_map(|&id| storage::load_player(id).ok().flatten().filter(|p| p.has_chud()))
            .map(|p| p.chud_ref().name.clone())
            .collect();
        let scouting_name_refs: Vec<&str> = scouting_player_names.iter().map(String::as_str).collect();
        let dm_content = formatting::format_dm_scouting_report(
            &sr.player_name,
            &sr.quest_title,
            sr.quest_days,
            sr.chance,
            active_player_name.as_deref(),
            &scouting_name_refs,
        );
        let dm_map = serde_json::json!({ "recipient_id": sr.discord_user_id.to_string() });
        match http.create_private_channel(&dm_map).await {
            Ok(dm) => {
                if let Err(e) = http
                    .send_message(dm.id, vec![], &CreateMessage::new().content(&dm_content))
                    .await
                {
                    tracing::warn!(discord_user_id = sr.discord_user_id, err = %e, "failed to DM scouting report");
                }
            }
            Err(e) => tracing::warn!(discord_user_id = sr.discord_user_id, err = %e, "failed to open DM channel for scouting report"),
        }
    }

    sync_activity_log_now(http, channel_id).await;

    if !outcome.quest_resolved.is_empty()
        || !outcome.scout_results.is_empty()
        || outcome.slots_filled > 0
    {
        storage::save_board(board)?;
    }
    guild_hall::update_board_message(http, channel_id, board, max_jobs, None).await?;

    {
        let mut merchant_guard = runtime.merchant.lock().await;
        let force = merchant_guard.spawn_next_tick;
        merchant_guard.spawn_next_tick = false;
        merchant_guard.advance_tick(force);
        storage::save_guild_hall(&merchant_guard)?;
        update_merchant_message(http, channel_id, &merchant_guard).await?;
    }

    if outcome.story_series_complete {
        tracing::info!(
            story_line = %crate::story_jobs::catalog().story_line_name,
            "story series complete"
        );
        // Hand off to the completion supervisor: we're running on the simulation's own tick
        // task, so it (not us) must stop the simulation and paint the complete screen.
        let completion = crate::discord::context::GameCompletion {
            user_id: outcome.final_completer_user_id.unwrap_or_default(),
            chud_name: outcome
                .final_completer_chud_name
                .clone()
                .unwrap_or_else(|| "a chud".to_string()),
        };
        let _ = runtime.complete_tx.send(completion);
    }

    Ok(())
}
