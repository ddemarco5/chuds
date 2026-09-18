use poise::serenity_prelude::{self as serenity, MessageId};

use crate::chud_msg;
use crate::discord::components_v2::{
    Component, ComponentsV2Message, Container, ContainerChild, TextDisplay,
};
use crate::discord::context::GameRuntime;
use crate::discord::formatting::{format_player_stats_block, PlayerStatsBlockOptions};
use crate::discord::guild_hall::{edit_cv2, reset_channel_cache, send_cv2};
use crate::discord::report_dm::{pack_message_segments, DM_CHAR_LIMIT};
use crate::game::engine;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;
use crate::story_jobs;

/// Collect the names of every chud currently in the game (one per player save with a chud).
fn living_chud_names() -> Vec<String> {
    let mut names: Vec<String> = storage::load_chuds()
        .into_iter()
        .map(|player| player.chud_ref().name.clone())
        .collect();
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
        "# {title}\n{when}\nType `/new_chud` to make your chud and join\n{list_header}\n{roster}"
    );
    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(body))])
}

/// Build the game-complete screen (story recap, per-chud stats, post-game records).
/// Returns one rich CV2 message when under the limit, or multiple text messages when fragmented.
async fn build_complete_messages(runtime: &GameRuntime) -> Vec<ComponentsV2Message> {
    let header = build_complete_header(runtime).await;
    let registry = runtime.item_registry.lock().await;
    let chud_blocks = per_chud_complete_blocks(&registry);

    let mut segments = vec![header.clone()];
    if !chud_blocks.is_empty() {
        segments.push("\n\n**The chuds**\n\n".to_string());
        for block in &chud_blocks {
            segments.push(format!("{block}\n\n"));
        }
    }

    let plain_len: usize = segments.iter().map(String::len).sum();
    if plain_len <= DM_CHAR_LIMIT {
        return vec![build_rich_complete_message(&header, &chud_blocks)];
    }

    tracing::info!(
        plain_len,
        parts = pack_message_segments(&segments, DM_CHAR_LIMIT).len(),
        "fragmenting complete screen across Discord messages"
    );
    pack_message_segments(&segments, DM_CHAR_LIMIT)
        .into_iter()
        .map(|part| ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(part))]))
        .collect()
}

async fn build_complete_header(runtime: &GameRuntime) -> String {
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

    let post_game = episode_stats.format_post_game_lines();
    if !post_game.is_empty() {
        body.push_str("\n\n");
        body.push_str(&post_game.join("\n"));
    }

    body
}

fn build_rich_complete_message(header: &str, chud_blocks: &[String]) -> ComponentsV2Message {
    let mut components = vec![Component::Text(TextDisplay::new(header))];

    if !chud_blocks.is_empty() {
        components.push(Component::Text(TextDisplay::new("**The chuds**")));
        for block in chud_blocks {
            components.push(Component::Container(Container::new(vec![ContainerChild::Text(
                TextDisplay::new(block),
            )])));
        }
    }

    ComponentsV2Message::channel(components)
}

fn per_chud_complete_blocks(registry: &ItemRegistry) -> Vec<String> {
    let mut players = storage::load_chuds();
    players.sort_by(|a, b| a.chud_ref().name.cmp(&b.chud_ref().name));

    players
        .iter()
        .map(|player| {
            let stats = format_player_stats_block(
                player,
                registry,
                PlayerStatsBlockOptions {
                    include_cash: false,
                },
            );
            format!("<@{}>'s chud {stats}", player.discord_user_id)
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
            if edit_cv2(&runtime.http, ch, MessageId::new(id), message)
                .await
                .is_ok()
            {
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
    let messages = build_complete_messages(runtime).await;
    let ch = serenity::ChannelId::new(runtime.channel_id);
    for message in &messages {
        if let Err(e) = send_cv2(&runtime.http, ch, message).await {
            tracing::warn!(err = %e, "failed to post complete screen segment");
        }
    }
    Ok(())
}
