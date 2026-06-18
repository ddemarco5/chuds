use poise::serenity_prelude::{self as serenity, MessageId};

use crate::chud_msg;
use crate::discord::components_v2::{Component, ComponentsV2Message, TextDisplay};
use crate::discord::context::GameRuntime;
use crate::discord::formatting::{format_player_stats_block, PlayerStatsBlockOptions};
use crate::discord::guild_hall::{edit_cv2, reset_channel_cache, send_cv2};
use crate::game::domain::player::Player;
use crate::game::engine;
use crate::game::persistence::item_registry::ItemRegistry;
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
    let title = story_jobs::story_line_name();
    let when = {
        let session = runtime.session.lock().await;
        match session.game_start_at {
            Some(ts) => format!("<t:{ts}:R>"),
            None => "`starting soon`".to_string(),
        }
    };
    let names = living_chud_names();
    let roster = if names.is_empty() {
        "-# *nobody yet*".to_string()
    } else {
        format!("-# \u{2043} {}", names.join(", "))
    };
    let list_header = chud_msg!("attract_chudlist");
    let body = format!(
        "# {title}\n{when}\nType `/chud` to make your chud and join\n{list_header}\n{roster}"
    );
    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(body))])
}

/// Build the game-complete screen (story recap, per-chud stats, post-game records).
async fn build_complete_message(runtime: &GameRuntime) -> ComponentsV2Message {
    let title = story_jobs::story_line_name();
    let story_count = story_jobs::story_count();

    let total_ticks = {
        let session = runtime.session.lock().await;
        session.total_ticks
    };

    let episode_stats = storage::load_episode_stats().unwrap_or_default();

    let mut body = format!("# {title} Complete!\n");
    let story_recap = episode_stats.format_story_recap(story_count);
    if !story_recap.is_empty() {
        body.push_str(&story_recap);
        body.push('\n');
    }
    body.push_str(&format!("Days taken: {total_ticks}"));

    let registry = runtime.item_registry.lock().await;
    let chud_blocks = per_chud_complete_blocks(&registry);
    if !chud_blocks.is_empty() {
        body.push_str("\n\n**The chuds**\n");
        body.push_str(&chud_blocks.join("\n\n"));
    }

    let post_game = episode_stats.format_post_game_lines();
    if !post_game.is_empty() {
        body.push_str("\n\n");
        body.push_str(&post_game.join("\n"));
    }

    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(body))])
}

fn indent_lines(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn per_chud_complete_blocks(registry: &ItemRegistry) -> Vec<String> {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for complete screen");
            return Vec::new();
        }
    };

    let mut players: Vec<Player> = Vec::new();
    for id in ids {
        if let Ok(Some(player)) = storage::load_player(id) {
            if player.has_chud() {
                players.push(player);
            }
        }
    }
    players.sort_by(|a, b| a.chud_ref().name.cmp(&b.chud_ref().name));

    players
        .iter()
        .map(|player| {
            let header = format!("<@{}>'s chud:", player.discord_user_id);
            let stats = format_player_stats_block(
                player,
                registry,
                PlayerStatsBlockOptions {
                    include_stash: false,
                    include_cash: true,
                },
            );
            format!("{header}\n{}", indent_lines(&stats, "    "))
        })
        .collect()
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

/// Enqueue background pre-game generation (story job + merchant catalog). Idempotent.
pub async fn kickoff_attract_generation(runtime: &GameRuntime) {
    {
        let board = runtime.board.lock().await;
        engine::ensure_story_generation(
            &board,
            Some(&runtime.generation_queue),
            Some(&runtime.pending_quests),
        );
    }
    {
        let merchant = runtime.merchant.lock().await;
        engine::ensure_merchant_catalog_generation(
            &merchant,
            Some(&runtime.generation_queue),
            Some(&runtime.pending_merchant_catalog),
        );
    }
}

/// Purge the channel and post a fresh attract screen (used on transition into Attract).
pub async fn post_attract_screen(runtime: &GameRuntime) -> anyhow::Result<()> {
    reset_channel_cache(&runtime.http, runtime.channel_id).await;

    let message = build_attract_message(runtime).await;
    render_phase_screen(runtime, &message).await?;

    // Draw the attract UI before kicking off slow LLM work so the channel isn't idle
    // while generation runs.
    kickoff_attract_generation(runtime).await;
    Ok(())
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
