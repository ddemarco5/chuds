use poise::serenity_prelude::CreateMessage;

use crate::chud_msg;
use crate::discord::channel::{append_activity_log_deferred, sync_activity_log_now};
use crate::discord::context::GameRuntime;
use crate::discord::formatting;
use crate::discord::guild_hall;
use crate::discord::report_dm;
use crate::discord::ui::update_merchant_message;
use crate::game::domain::player::first_word;
use crate::game::engine::{self, KillChudPending, KillResult};
use crate::game::persistence::message_cache::ActivityLogKind;
use crate::game::persistence::storage;
use crate::game::tick::TickOutcome;

/// Render a completed [`TickOutcome`] to Discord: activity log, DMs, board, merchant.
///
/// Clones game state under brief locks, then performs all Discord I/O without holding
/// runtime mutexes so interaction handlers stay responsive.
pub async fn render_tick_outcome(
    runtime: &GameRuntime,
    outcome: &TickOutcome,
    pending_deaths: &[KillChudPending],
) -> anyhow::Result<()> {
    let http = &runtime.http;
    let activity_log = &runtime.activity_log;
    let channel_id = runtime.channel_id;
    let max_jobs = runtime.max_jobs;

    let (board, registry, merchant) = {
        let b = runtime.board.lock().await;
        let r = runtime.item_registry.lock().await;
        let m = runtime.merchant.lock().await;
        (b.clone(), r.clone(), m.clone())
    };

    for (discord_user_id, message) in &outcome.hospital_releases {
        append_activity_log_deferred(activity_log, ActivityLogKind::Standard, message).await;
        report_dm::send_hospital_release_dm(http, *discord_user_id, message).await;
    }

    for qr in &outcome.quest_resolved {
        append_activity_log_deferred(
            activity_log,
            ActivityLogKind::QuestSummary,
            &qr.summary,
        )
        .await;
        if qr.level_up.any() {
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
            let adj_str = formatting::oxford_join(&adjs);
            let level_msg = format!("{} seems {}.", qr.player_name, adj_str);
            append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &level_msg).await;
        }

        let first_name = first_word(&qr.player_name);
        if qr.died {
            let return_msg = chud_msg!("return_died", first_name);
            append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &return_msg).await;
        } else if qr.hospitalized {
            let return_msg = chud_msg!("return_hospitalized", first_name);
            append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &return_msg).await;
        }

        let kill_result = if qr.died {
            if let Some(pending) = pending_deaths
                .iter()
                .find(|p| p.discord_user_id == qr.discord_user_id)
            {
                let epitaph =
                    engine::finish_gravestone_epitaph(&runtime.gravestone_generator, pending)
                        .await?;
                Some(KillResult {
                    discord_user_id: pending.discord_user_id,
                    chud_name: pending.chud_name.clone(),
                    benefits_awarded: pending.benefits_awarded,
                    epitaph,
                })
            } else {
                None
            }
        } else {
            None
        };

        let dm_ok = report_dm::send_job_completion_dm(http, &registry, qr, kill_result.as_ref()).await;
        if qr.awaiting_return && !dm_ok {
            match engine::complete_guild_return(qr.discord_user_id) {
                Ok(Some((_, kind))) => {
                    let return_msg = chud_msg!(kind.message_key(), first_name);
                    append_activity_log_deferred(activity_log, ActivityLogKind::Standard, &return_msg)
                        .await;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    discord_user_id = qr.discord_user_id,
                    err = %e,
                    "failed to auto-return chud after DM failure"
                ),
            }
        }
    }

    for sr in &outcome.scout_results {
        let first_name = first_word(&sr.player_name);
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

    guild_hall::update_board_message(http, channel_id, &board, max_jobs, None).await?;
    update_merchant_message(http, channel_id, &merchant).await?;

    if outcome.story_series_complete {
        tracing::info!(
            story_line = %crate::story_jobs::story_line_name(),
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
