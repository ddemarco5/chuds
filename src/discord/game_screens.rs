use poise::serenity_prelude::{self as serenity, MessageId};

use crate::discord::components_v2::{Component, ComponentsV2Message, TextDisplay};
use crate::discord::context::GameRuntime;
use crate::discord::guild_hall::{edit_cv2, format_chudlerboard, reset_channel_cache, send_cv2};
use crate::game::engine;
use crate::game::persistence::storage;
use crate::story_jobs;

/// Collect the names of every chud currently in the game (one per player save with a chud).
fn living_chud_names() -> Vec<String> {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for attract screen");
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for id in ids {
        if let Ok(Some(player)) = storage::load_player(id) {
            if player.has_chud() {
                names.push(player.chud_ref().name.clone());
            }
        }
    }
    names.sort();
    names
}

/// Build the attract-screen message (story title, countdown, join prompt, chud roster).
async fn build_attract_message(runtime: &GameRuntime) -> ComponentsV2Message {
    let title = &story_jobs::catalog().story_line_name;
    let when = {
        let session = runtime.session.lock().await;
        match session.game_start_at {
            Some(ts) => format!("<t:{ts}:R>"),
            None => "`starting soon`".to_string(),
        }
    };
    let names = living_chud_names();
    let roster = if names.is_empty() {
        "*nobody yet*".to_string()
    } else {
        names.join(", ")
    };
    let body = format!(
        "# {title}\n{when}\ntype `/chud` to join\nCurrent vict... uh... chuds:\n{roster}"
    );
    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(body))])
}

/// Build the game-complete screen (final mission, game stats, chudlerboard, per-chud stats).
async fn build_complete_message(runtime: &GameRuntime) -> ComponentsV2Message {
    let title = &story_jobs::catalog().story_line_name;

    let (completer, total_ticks) = {
        let session = runtime.session.lock().await;
        (
            session
                .final_completer_chud_name
                .clone()
                .unwrap_or_else(|| "TBD".to_string()),
            session.total_ticks,
        )
    };

    let mut body = format!(
        "# {title}\n## GAME COMPLETE\nFinal mission completed by: **{completer}**\nDays taken: {total_ticks}\n"
    );

    let chudlerboard = format_chudlerboard(&runtime.http).await;
    if !chudlerboard.is_empty() {
        body.push('\n');
        body.push_str(&chudlerboard);
    }

    let stats = per_chud_stat_lines();
    if !stats.is_empty() {
        body.push_str("\n**The chuds**\n");
        body.push_str(&stats.join("\n"));
    }

    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(body))])
}

/// One line per living chud: missions passed/failed and current cash.
fn per_chud_stat_lines() -> Vec<String> {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for complete screen");
            return Vec::new();
        }
    };
    let mut lines = Vec::new();
    for id in ids {
        if let Ok(Some(player)) = storage::load_player(id) {
            if player.has_chud() {
                let chud = player.chud_ref();
                lines.push(format!(
                    "**{}** — {} passed, {} failed | ${}",
                    chud.name, chud.total_job_successes, chud.total_job_failures, player.cash
                ));
            }
        }
    }
    lines
}

/// Post or edit the single attract/complete phase-screen message (edit-in-place via cache key).
async fn render_phase_screen(
    runtime: &GameRuntime,
    message: &ComponentsV2Message,
) -> anyhow::Result<()> {
    let _cache_guard = storage::message_cache_lock().await;
    let ch = serenity::ChannelId::new(runtime.channel_id);
    let mut cache = storage::load_message_cache().unwrap_or_default();
    let key = message.cache_key();
    let mut dirty = false;

    match cache.phase_screen_message_id {
        Some(id) if key == cache.phase_screen_content => {
            // Unchanged; nothing to do.
            let _ = id;
        }
        Some(id) => {
            if edit_cv2(&runtime.http, ch, MessageId::new(id), message).await {
                cache.phase_screen_content = key;
                dirty = true;
            } else {
                tracing::warn!(msg_id = id, "failed to edit phase screen, posting new one");
                match send_cv2(&runtime.http, ch, message).await {
                    Ok(msg) => {
                        cache.phase_screen_message_id = Some(msg.id.get());
                        cache.phase_screen_content = key;
                        dirty = true;
                    }
                    Err(e) => tracing::warn!(err = %e, "failed to post phase screen"),
                }
            }
        }
        None => match send_cv2(&runtime.http, ch, message).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "phase screen posted");
                cache.phase_screen_message_id = Some(msg.id.get());
                cache.phase_screen_content = key;
                dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post phase screen"),
        },
    }

    if dirty {
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache after phase screen");
        }
    }
    Ok(())
}

/// Purge the channel and post a fresh attract screen (used on transition into Attract).
pub async fn post_attract_screen(runtime: &GameRuntime) -> anyhow::Result<()> {
    reset_channel_cache(&runtime.http, runtime.channel_id).await;

    // Kick off the first story job now (idempotent) so it's generated and waiting on the
    // board the moment the game switches to Playing. The worker persists it but skips the
    // board UI update while we're in attract.
    {
        let board = runtime.board.lock().await;
        engine::ensure_story_generation(
            &board,
            Some(&runtime.generation_queue),
            Some(&runtime.pending_quests),
        );
    }

    let message = build_attract_message(runtime).await;
    render_phase_screen(runtime, &message).await
}

/// Refresh the attract screen in place (used when a chud joins during Attract).
pub async fn update_attract_screen(runtime: &GameRuntime) -> anyhow::Result<()> {
    let message = build_attract_message(runtime).await;
    render_phase_screen(runtime, &message).await
}

/// Purge the channel and post the game-complete screen (used on transition into Complete).
pub async fn post_complete_screen(runtime: &GameRuntime) -> anyhow::Result<()> {
    reset_channel_cache(&runtime.http, runtime.channel_id).await;
    let message = build_complete_message(runtime).await;
    render_phase_screen(runtime, &message).await
}
