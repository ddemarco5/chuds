use poise::serenity_prelude::{self as serenity, CreateAttachment, CreateMessage};

use crate::chud_msg;
use crate::discord::board_ui;
use crate::discord::channel::{cleanup_non_bot_messages, post_buffered_message};
use crate::discord::formatting;
use crate::game::domain::board::Board;
use crate::game::domain::job_queue::JobQueue;
use crate::game::persistence::storage;
use crate::game::tick::{run_tick, TickContext};

/// Apply one game tick and post results to Discord.
pub async fn execute_tick(
    http: &serenity::Http,
    board: &tokio::sync::Mutex<Board>,
    job_queue: &tokio::sync::Mutex<JobQueue>,
    channel_id: u64,
    max_buffer: usize,
    max_jobs: usize,
    bot_user_id: u64,
    max_non_bot_messages: usize,
) -> anyhow::Result<()> {
    cleanup_non_bot_messages(http, channel_id, bot_user_id, max_non_bot_messages).await;

    {
        let board_guard = board.lock().await;
        if board_guard.active_quest_count() > 0 {
            let new_day_msg = chud_msg!("new_day");
            post_buffered_message(http, channel_id, max_buffer, &new_day_msg).await;
        }
    }

    let mut hospital = storage::load_hospital()?;
    let mut board = board.lock().await;
    let mut queue = job_queue.lock().await;

    let outcome = run_tick(&mut TickContext {
        board: &mut *board,
        hospital: &mut hospital,
        queue: &mut *queue,
        max_jobs,
    })?;

    storage::save_hospital(&hospital)?;

    for msg in &outcome.hospital_releases {
        post_buffered_message(http, channel_id, max_buffer, msg).await;
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
        post_buffered_message(http, channel_id, max_buffer, &content).await;

        let first_name = qr
            .player_name
            .split_whitespace()
            .next()
            .unwrap_or(&qr.player_name)
            .to_string();
        let return_key = if qr.hospitalized {
            "return_hospitalized"
        } else if qr.result.passed {
            "return_passed"
        } else {
            "return_failed"
        };
        let return_msg = chud_msg!(return_key, first_name);
        post_buffered_message(http, channel_id, max_buffer, &return_msg).await;

        let dm_content = formatting::format_dm_completion_report(
            &qr.player_name,
            &qr.result,
            &qr.player,
            &qr.level_up,
            qr.reward,
        );
        let dm_map = serde_json::json!({ "recipient_id": qr.discord_user_id.to_string() });
        match http.create_private_channel(&dm_map).await {
            Ok(dm) => {
                const DM_LIMIT: usize = 2000;
                let (attachments, msg) = if dm_content.len() > DM_LIMIT {
                    let attachment =
                        CreateAttachment::bytes(dm_content.into_bytes(), "job_report.txt");
                    let msg = CreateMessage::new().content(format!(
                        "{}'s attempt at {} was too epic for discords character limit",
                        qr.player_name, qr.quest_title
                    ));
                    (vec![attachment], msg)
                } else {
                    (vec![], CreateMessage::new().content(dm_content))
                };
                if let Err(e) = http.send_message(dm.id, attachments, &msg).await {
                    tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to DM quest report");
                }

                if qr.hospitalized {
                    let hospital_msg = chud_msg!("dm_hospitalized", qr.player_name);
                    let hospital_dm = CreateMessage::new().content(hospital_msg);
                    if let Err(e) = http.send_message(dm.id, vec![], &hospital_dm).await {
                        tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to DM hospital notification");
                    }
                }
            }
            Err(e) => tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to open DM channel"),
        }
    }

    for sr in &outcome.scout_results {
        let first_name = sr
            .player_name
            .split_whitespace()
            .next()
            .unwrap_or(&sr.player_name)
            .to_string();
        let scout_key = if sr.chance == 0.0 {
            "scout_returned_impossible"
        } else if sr.chance <= 0.25 {
            "scout_returned_terrified"
        } else if sr.chance <= 0.50 {
            "scout_returned_nervous"
        } else if sr.chance <= 0.80 {
            "scout_returned_confident"
        } else {
            "scout_returned_cocky"
        };
        let scout_return_msg = chud_msg!(scout_key, first_name);
        post_buffered_message(http, channel_id, max_buffer, &scout_return_msg).await;

        let active_player_name: Option<String> = sr
            .active_discord_user_id
            .and_then(|id| storage::load_player(id).ok().flatten())
            .map(|p| p.chud.name);
        let scouting_player_names: Vec<String> = sr
            .other_scouting_discord_user_ids
            .iter()
            .filter_map(|&id| storage::load_player(id).ok().flatten())
            .map(|p| p.chud.name)
            .collect();
        let scouting_name_refs: Vec<&str> = scouting_player_names.iter().map(String::as_str).collect();
        let dm_content = formatting::format_dm_scouting_report(
            &sr.player_name,
            &sr.quest_title,
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

    if !outcome.quest_resolved.is_empty()
        || !outcome.scout_results.is_empty()
        || outcome.slots_filled > 0
    {
        storage::save_board(&*board)?;
        board_ui::update_board_message(http, channel_id, &mut *board, max_jobs).await?;
    }
    Ok(())
}
