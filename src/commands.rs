use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use poise::serenity_prelude::{self as serenity, CreateAttachment, CreateMessage, EditMessage, GetMessages, MessageId};

use crate::board::{Board, BoardQuest};
use crate::chud_msg;
use crate::message_cache::JobSlot;
use crate::engine;
use crate::quest_generator::QuestGenerator;
use crate::simulation;
use crate::storage;

pub struct Data {
    pub generator: Arc<QuestGenerator>,
    pub board: Arc<tokio::sync::Mutex<Board>>,
    pub admin_user_id: u64,
    pub bot_user_id: u64,
    pub channel_id: u64,
    pub max_buffer_messages: usize,
    pub max_jobs: usize,
    pub max_non_bot_messages: usize,
    /// Sender half of the generation worker channel (see main.rs).
    /// Commands drop a GenerationJob here after assigning a quest; the worker
    /// task runs the LLM call in the background and writes the result to
    /// board.completed_results. The tick loop picks it up later.
    pub generation_queue: tokio::sync::mpsc::UnboundedSender<engine::GenerationJob>,
    /// Number of QuestCreation jobs currently in-flight (queued but not yet
    /// written to the board). Checked alongside board.quests.len() under the
    /// board lock in /generate_job to prevent concurrent requests from
    /// overflowing MAX_JOBS.
    pub pending_quests: Arc<AtomicUsize>,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// Returns true if the invoking user is the admin and is in the configured channel.
/// On failure logs a warning and sends an ephemeral ack to resolve the deferred interaction.
async fn admin_guard(ctx: Context<'_>) -> bool {
    let data = ctx.data();
    let ok = ctx.author().id.get() == data.admin_user_id
        && ctx.channel_id().get() == data.channel_id;
    if !ok {
        tracing::warn!(
            user = ctx.author().id.get(),
            channel = ctx.channel_id().get(),
            "unauthorized or off-channel command ignored"
        );
        ctx.say("you don't have permission for this command (sorry bud)").await.ok();
    }
    ok
}

/// Returns true if the invoking user is admin or a Chudmaster and is in the configured channel.
async fn chudmaster_guard(ctx: Context<'_>) -> bool {
    let data = ctx.data();
    let user_id = ctx.author().id.get();
    let is_admin = user_id == data.admin_user_id;
    let is_chudmaster = storage::is_chudmaster(user_id).unwrap_or(false);
    let ok = (is_admin || is_chudmaster) && ctx.channel_id().get() == data.channel_id;
    if !ok {
        tracing::warn!(
            user = user_id,
            channel = ctx.channel_id().get(),
            "unauthorized or off-channel command ignored"
        );
        ctx.say("you don't have permission for this command (sorry bud)").await.ok();
    }
    ok
}

// ---------------------------------------------------------------------------
// Channel cleanup
// ---------------------------------------------------------------------------

/// Delete all non-bot messages in the channel when their count exceeds `threshold`.
///
/// Runs before any message edits on each tick so the board channel stays tidy.
pub async fn cleanup_non_bot_messages(
    http: &serenity::Http,
    channel_id: u64,
    bot_user_id: u64,
    threshold: usize,
) {
    let ch = serenity::ChannelId::new(channel_id);
    let messages = match ch.messages(http, GetMessages::new().limit(100)).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(err = %e, "failed to fetch channel messages for cleanup");
            return;
        }
    };
    let non_bot: Vec<_> = messages.iter()
        .filter(|m| m.author.id.get() != bot_user_id)
        .collect();
    if non_bot.len() > threshold {
        tracing::info!(count = non_bot.len(), threshold, "cleaning up non-bot messages in channel");
        for msg in &non_bot {
            if let Err(e) = http.delete_message(ch, msg.id, None).await {
                tracing::warn!(msg_id = msg.id.get(), err = %e, "failed to delete non-bot message during cleanup");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Board message helpers
// ---------------------------------------------------------------------------

fn format_job_slot(quest: Option<&BoardQuest>) -> String {
    match quest {
        Some(q) => {
            let title = &q.generated.quest_title;
            let giver = &q.generated.quest_giver;
            let desc = &q.generated.description;
            let reward = q.generated.reward;
            if q.has_active() {
                format!("~~**{}** - *{}* | ${}~~\n~~{}~~\n─────────────────", title, giver, reward, desc)
            } else {
                format!("**{}** - *{}* | ${}\n{}\n─────────────────", title, giver, reward, desc)
            }
        }
        None => "*Nothing posted here*".to_string(),
    }
}

async fn post_buffered_message(
    http: &serenity::Http,
    channel_id: u64,
    max_buffer: usize,
    content: &str,
) {
    let mut cache = storage::load_message_cache().unwrap_or_default();
    let ch = serenity::ChannelId::new(channel_id);
    if cache.pending_deletes.len() >= max_buffer {
        let old_id = cache.pending_deletes.remove(0);
        if let Err(e) = http.delete_message(ch, MessageId::new(old_id), None).await {
            tracing::warn!(msg_id = old_id, err = %e, "failed to evict oldest buffered message");
        }
    }
    match http.send_message(ch, vec![], &CreateMessage::new().content(content)).await {
        Ok(msg) => {
            cache.pending_deletes.push(msg.id.get());
            if let Err(e) = storage::save_message_cache(&cache) {
                tracing::warn!(err = %e, "failed to save message cache after buffered post");
            }
        }
        Err(e) => tracing::warn!(err = %e, "failed to post buffered message"),
    }
}

async fn format_chudlerboard(http: &serenity::Http) -> String {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for chudlerboard");
            return String::new();
        }
    };
    if ids.is_empty() {
        return String::new();
    }

    let players: Vec<crate::player::Player> = ids
        .iter()
        .filter_map(|&id| match storage::load_player(id) {
            Ok(Some(p)) => Some(p),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(discord_user_id = id, err = %e, "failed to load player for chudlerboard");
                None
            }
        })
        .collect();

    if players.is_empty() {
        return String::new();
    }

    fn sole_leader(
        players: &[crate::player::Player],
        accessor: fn(&crate::player::Player) -> u8,
    ) -> Option<String> {
        let max_val = players.iter().map(|p| accessor(p)).max().unwrap_or(0);
        let mut leaders = players.iter().filter(|p| accessor(p) == max_val);
        let first = leaders.next()?;
        if leaders.next().is_some() { return None; }
        Some(first.name.clone())
    }

    let mut lines = Vec::new();

    if let Some(richest) = players.iter().max_by_key(|p| p.cash).filter(|_| {
        let max_cash = players.iter().map(|p| p.cash).max().unwrap_or(0);
        players.iter().filter(|p| p.cash == max_cash).count() == 1
    }) {
        let discord_name = match http.get_user(serenity::UserId::new(richest.discord_user_id)).await {
            Ok(user) => user.global_name.unwrap_or(user.name),
            Err(_) => richest.discord_user_id.to_string(),
        };
        lines.push(format!("{} is the richest chudlord", discord_name));
    }

    let stat_defs: [(&str, fn(&crate::player::Player) -> u8); 4] = [
        ("strongest", |p| p.strength),
        ("smartest",  |p| p.smarts),
        ("sneakiest", |p| p.stealth),
        ("expert",       |p| p.experience),
    ];

    for (adjective, accessor) in &stat_defs {
        if let Some(name) = sole_leader(&players, *accessor) {
            lines.push(format!("{} is the {}", name, adjective));
        }
    }

    if lines.is_empty() {
        return String::new();
    }

    format!("```\n----- Chudlerboard -----\n{}\n```", lines.join("\n"))
}

pub(crate) async fn update_board_message(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    max_jobs: usize,
) -> anyhow::Result<()> {
    let ch = serenity::ChannelId::new(channel_id);
    let mut board_dirty = false;

    let mut cache = storage::load_message_cache().unwrap_or_default();
    let mut cache_dirty = false;

    // --- Chudlerboard message ---
    let cb_content = {
        let cb = format_chudlerboard(http).await;
        if cb.is_empty() { "*No chuds yet.*".to_string() } else { cb }
    };
    if cb_content != cache.chudlerboard {
        if let Some(msg_id) = board.chudlerboard_message_id {
            let ok = http
                .edit_message(ch, MessageId::new(msg_id), &EditMessage::new().content(&cb_content), vec![])
                .await
                .is_ok();
            if !ok {
                tracing::warn!(msg_id, "failed to edit chudlerboard message, posting new one");
                match http.send_message(ch, vec![], &CreateMessage::new().content(&cb_content)).await {
                    Ok(msg) => {
                        board.chudlerboard_message_id = Some(msg.id.get());
                        cache.chudlerboard = cb_content.clone();
                        board_dirty = true;
                        cache_dirty = true;
                    }
                    Err(e) => tracing::warn!(err = %e, "failed to post chudlerboard message"),
                }
            } else {
                cache.chudlerboard = cb_content.clone();
                cache_dirty = true;
                tracing::debug!(msg_id, "chudlerboard message edited");
            }
        } else {
            match http.send_message(ch, vec![], &CreateMessage::new().content(&cb_content)).await {
                Ok(msg) => {
                    tracing::info!(msg_id = msg.id.get(), "chudlerboard message posted");
                    board.chudlerboard_message_id = Some(msg.id.get());
                    cache.chudlerboard = cb_content.clone();
                    board_dirty = true;
                    cache_dirty = true;
                }
                Err(e) => tracing::warn!(err = %e, "failed to post chudlerboard message"),
            }
        }
    }

    // --- Job board header message ---
    let filled = board.quests.len();
    let header_content = format!("\u{200B}\n\u{200B}\t\u{200B}\t**CHUD GUILD JOB BOARD** *{}/{} posted*\n\u{200B}", filled, max_jobs);
    if cache.header_message_id.is_none() {
        match http.send_message(ch, vec![], &CreateMessage::new().content(&header_content)).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "job board header message posted");
                cache.header_message_id = Some(msg.id.get());
                cache.job_board_header = header_content;
                cache_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post job board header message"),
        }
    } else if header_content != cache.job_board_header {
        let msg_id = cache.header_message_id.unwrap();
        let ok = http
            .edit_message(ch, MessageId::new(msg_id), &EditMessage::new().content(&header_content), vec![])
            .await
            .is_ok();
        if ok {
            cache.job_board_header = header_content;
            cache_dirty = true;
            tracing::debug!(msg_id, "job board header message edited");
        } else {
            tracing::warn!(msg_id, "failed to edit job board header message");
        }
    }

    // --- Job slot messages ---
    // Grow the slot list to max_jobs if needed (new slots start empty).
    while cache.slots.len() < max_jobs {
        cache.slots.push(JobSlot::default());
        cache_dirty = true;
    }

    // Tracks which slot indices had a job added or removed this tick (not strike-through/restore).
    let mut clear_reactions_slots = vec![false; cache.slots.len()];

    // Step 1: Clear slots whose quest is no longer on the board.
    for (i, slot) in cache.slots.iter_mut().enumerate() {
        if let Some(jid) = slot.job_id {
            if !board.quests.iter().any(|q| q.id == jid) {
                slot.job_id = None;
                cache_dirty = true;
                clear_reactions_slots[i] = true;
            }
        }
    }

    // Step 2: Assign board quests that don't yet have a slot to the first empty slot.
    for quest in &board.quests {
        let already_slotted = cache.slots.iter().any(|s| s.job_id == Some(quest.id));
        if !already_slotted {
            if let Some(i) = cache.slots.iter().position(|s| s.job_id.is_none()) {
                cache.slots[i].job_id = Some(quest.id);
                cache_dirty = true;
                clear_reactions_slots[i] = true;
            }
        }
    }

    // Step 3: Edit or post each slot's Discord message when content has changed.
    for (i, slot) in cache.slots.iter_mut().enumerate() {
        let quest = slot.job_id.and_then(|jid| board.quests.iter().find(|q| q.id == jid));
        let expected = format_job_slot(quest);
        if expected == slot.content && slot.message_id.is_some() {
            continue;
        }
        if let Some(msg_id) = slot.message_id {
            let ok = http
                .edit_message(ch, MessageId::new(msg_id), &EditMessage::new().content(&expected), vec![])
                .await
                .is_ok();
            if !ok {
                tracing::warn!(msg_id, slot = i, "failed to edit job slot message, posting new one");
                match http.send_message(ch, vec![], &CreateMessage::new().content(&expected)).await {
                    Ok(msg) => {
                        slot.message_id = Some(msg.id.get());
                        slot.content = expected;
                        cache_dirty = true;
                        if clear_reactions_slots[i] {
                            if let Err(e) = http.delete_message_reactions(ch, msg.id).await {
                                tracing::warn!(msg_id = msg.id.get(), slot = i, err = %e, "failed to clear reactions on job slot");
                            }
                        }
                    }
                    Err(e) => tracing::warn!(err = %e, slot = i, "failed to post replacement job slot message"),
                }
            } else {
                slot.content = expected;
                cache_dirty = true;
                tracing::debug!(msg_id, slot = i, "job slot message edited");
                if clear_reactions_slots[i] {
                    if let Err(e) = http.delete_message_reactions(ch, MessageId::new(msg_id)).await {
                        tracing::warn!(msg_id, slot = i, err = %e, "failed to clear reactions on job slot");
                    }
                }
            }
        } else {
            match http.send_message(ch, vec![], &CreateMessage::new().content(&expected)).await {
                Ok(msg) => {
                    tracing::info!(msg_id = msg.id.get(), slot = i, "job slot message posted");
                    slot.message_id = Some(msg.id.get());
                    slot.content = expected;
                    cache_dirty = true;
                    if clear_reactions_slots[i] {
                        if let Err(e) = http.delete_message_reactions(ch, msg.id).await {
                            tracing::warn!(msg_id = msg.id.get(), slot = i, err = %e, "failed to clear reactions on job slot");
                        }
                    }
                }
                Err(e) => tracing::warn!(err = %e, slot = i, "failed to post job slot message"),
            }
        }
    }

    // --- Divider message ---
    // Content is a compile-time constant; only post it once — never re-edit.
    const DIVIDER: &str = "\u{200B}\n\u{200B}";
    if cache.divider_message_id.is_none() {
        match http.send_message(ch, vec![], &CreateMessage::new().content(DIVIDER)).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "divider message posted");
                cache.divider_message_id = Some(msg.id.get());
                cache_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post divider message"),
        }
    }

    if cache_dirty {
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache");
        }
    }
    if board_dirty {
        storage::save_board(board)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Apply one game tick and post results to Discord.
///
/// This function does **not** call the LLM. Generation is handled by the
/// background worker task (see main.rs). By the time a tick fires, the worker
/// has (usually) already written each quest's result into
/// `board.completed_results`. `engine::run_tick` reads those cached results,
/// applies stat changes, and returns the resolved quests for us to announce.
/// Any quest whose result isn't ready yet is silently skipped and rechecked
/// on the next tick.
pub async fn execute_tick(
    http: &serenity::Http,
    board: &tokio::sync::Mutex<Board>,
    channel_id: u64,
    max_buffer: usize,
    max_jobs: usize,
    bot_user_id: u64,
    max_non_bot_messages: usize,
) -> anyhow::Result<()> {
    cleanup_non_bot_messages(http, channel_id, bot_user_id, max_non_bot_messages).await;

    let mut board = board.lock().await;

    let (resolved, scouted) = simulation::run_tick(&mut *board).await?;

    for qr in &resolved {
        tracing::info!(quest = %qr.quest_title, passed = qr.result.passed, player = %qr.player_name, "quest resolved");
        let content = if qr.level_up.any() {
            let mut adjs: Vec<&str> = Vec::new();
            if qr.level_up.str_up { adjs.push("stronger"); }
            if qr.level_up.smt_up { adjs.push("smarter"); }
            if qr.level_up.sth_up { adjs.push("sneakier"); }
            if qr.level_up.exp_up { adjs.push("more experienced"); }
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

        let dm_content = engine::format_dm_completion_report(&qr.player_name, &qr.result, &qr.player, &qr.level_up, qr.reward);
        let dm_map = serde_json::json!({ "recipient_id": qr.discord_user_id.to_string() });
        match http.create_private_channel(&dm_map).await {
            Ok(dm) => {
                const DM_LIMIT: usize = 2000;
                let (attachments, msg) = if dm_content.len() > DM_LIMIT {
                    let attachment = CreateAttachment::bytes(dm_content.into_bytes(), "job_report.txt");
                    let msg = CreateMessage::new()
                        .content(format!("{}'s attempt at {} was too epic for discords character limit", qr.player_name, qr.quest_title));
                    (vec![attachment], msg)
                } else {
                    (vec![], CreateMessage::new().content(dm_content))
                };
                if let Err(e) = http.send_message(dm.id, attachments, &msg).await {
                    tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to DM quest report");
                }
            }
            Err(e) => tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to open DM channel"),
        }
    }

    for sr in &scouted {
        let first_name = sr.player_name.split_whitespace().next().unwrap_or(&sr.player_name).to_string();
        let flavour = if sr.chance == 0.0 {
            " with poop in their pants"
        } else if sr.chance <= 0.20 {
            " white as a ghost"
        } else if sr.chance >= 0.80 {
            " with a shit-eating grin"
        } else {
            ""
        };
        post_buffered_message(http, channel_id, max_buffer, &format!("{} saunters back in{}.", first_name, flavour)).await;

        let active_player_name: Option<String> = sr.active_discord_user_id
            .and_then(|id| storage::load_player(id).ok().flatten())
            .map(|p| p.chud.name);
        let scouting_player_names: Vec<String> = sr.other_scouting_discord_user_ids.iter()
            .filter_map(|&id| storage::load_player(id).ok().flatten())
            .map(|p| p.chud.name)
            .collect();
        let scouting_name_refs: Vec<&str> = scouting_player_names.iter().map(String::as_str).collect();
        let dm_content = engine::format_dm_scouting_report(
            &sr.player_name,
            &sr.quest_title,
            sr.chance,
            active_player_name.as_deref(),
            &scouting_name_refs,
        );
        let dm_map = serde_json::json!({ "recipient_id": sr.discord_user_id.to_string() });
        match http.create_private_channel(&dm_map).await {
            Ok(dm) => {
                if let Err(e) = http.send_message(dm.id, vec![], &CreateMessage::new().content(&dm_content)).await {
                    tracing::warn!(discord_user_id = sr.discord_user_id, err = %e, "failed to DM scouting report");
                }
            }
            Err(e) => tracing::warn!(discord_user_id = sr.discord_user_id, err = %e, "failed to open DM channel for scouting report"),
        }
    }

    if !resolved.is_empty() || !scouted.is_empty() {
        storage::save_board(&*board)?;
        update_board_message(http, channel_id, &mut *board, max_jobs).await?;
    }
    Ok(())
}

/// Grant a Discord user Chudmaster status, allowing them to post and generate jobs.
#[poise::command(slash_command)]
pub async fn add_cm(ctx: Context<'_>, discord_user_id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let discord_user_id: u64 = match discord_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => { ctx.say("invalid Discord user ID").await?; return Ok(()); }
    };
    let added = storage::add_chudmaster(discord_user_id)?;
    if !added {
        ctx.say("that user is already a Chudmaster™").await?;
        return Ok(());
    }
    let http = &ctx.serenity_context().http;
    let display_name = match http.get_user(serenity::UserId::new(discord_user_id)).await {
        Ok(user) => user.global_name.unwrap_or(user.name),
        Err(_) => discord_user_id.to_string(),
    };
    let content = format!("{} is now a Chudmaster™", display_name);
    post_buffered_message(http, ctx.data().channel_id, ctx.data().max_buffer_messages, &content).await;
    ctx.say("ok").await?;
    Ok(())
}

/// Revoke a Discord user's Chudmaster status.
#[poise::command(slash_command)]
pub async fn delete_cm(ctx: Context<'_>, discord_user_id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let discord_user_id: u64 = match discord_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => { ctx.say("invalid Discord user ID").await?; return Ok(()); }
    };
    let removed = storage::remove_chudmaster(discord_user_id)?;
    if !removed {
        ctx.say("that user is not a Chudmaster™").await?;
        return Ok(());
    }
    ctx.say("ok").await?;
    Ok(())
}

/// Advance the board by one tick, resolving any due quests via the LLM.
#[poise::command(slash_command)]
pub async fn tick(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    execute_tick(
        &ctx.serenity_context().http,
        &ctx.data().board,
        ctx.data().channel_id,
        ctx.data().max_buffer_messages,
        ctx.data().max_jobs,
        ctx.data().bot_user_id,
        ctx.data().max_non_bot_messages,
    ).await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Fully generate a new quest via the LLM and post it to the board.
#[poise::command(slash_command)]
pub async fn generate_job(
    ctx: Context<'_>,
    description: String,
    difficulty: u8,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !chudmaster_guard(ctx).await {
        tracing::info!("Non-CM tried to submit a generate job");
        return Ok(());
    }
    {
        let board = ctx.data().board.lock().await;
        let pending = ctx.data().pending_quests.load(Ordering::SeqCst);
        if board.quests.len() + pending >= ctx.data().max_jobs {
            ctx.say(format!("Board is full ({} jobs max).", ctx.data().max_jobs)).await?;
            return Ok(());
        }
        ctx.data().pending_quests.fetch_add(1, Ordering::SeqCst);
    }
    tracing::info!("{} submitted generate_job with description '{}'", ctx.author().name, description);
    ctx.data().generation_queue.send(engine::make_quest_creation_job(description, difficulty))
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;
    ctx.say("ok").await?;
    Ok(())
}

/// Post a user-authored quest; only trial situations are LLM-generated.
#[poise::command(slash_command)]
pub async fn write_job(
    ctx: Context<'_>,
    title: String,
    giver: String,
    description: String,
    goal: String,
    difficulty: u8,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !chudmaster_guard(ctx).await {
        tracing::info!("Non-CM tried to submit a write job");
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    if board.quests.len() >= ctx.data().max_jobs {
        ctx.say(format!("Board is full ({} jobs max).", ctx.data().max_jobs)).await?;
        return Ok(());
    }
    tracing::info!("{} submitted write_job with description '{}'", ctx.author().name, description);
    engine::write_job(&ctx.data().generator, &mut *board, title, giver, description, goal, difficulty).await?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board, ctx.data().max_jobs).await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Remove a quest from the board by its ID.
#[poise::command(slash_command)]
pub async fn delete_job(ctx: Context<'_>, quest_id: u32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    engine::delete_quest(&mut *board, quest_id)?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board, ctx.data().max_jobs).await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Admin: create a chud for a specific Discord user.
#[poise::command(slash_command)]
pub async fn add_chud(ctx: Context<'_>, target_user_id: String, name: String, description: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let target_user_id: u64 = match target_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => { ctx.say("invalid Discord user ID").await?; return Ok(()); }
    };
    engine::add_chud(target_user_id, name, description)?;
    ctx.say("ok").await?;
    Ok(())
}

/// Create your chud and join the game.
#[poise::command(slash_command)]
pub async fn chud(ctx: Context<'_>, name: String, description: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if storage::load_player(ctx.author().id.get())?.is_some() {
        ctx.say("you've already got a chud").await?;
        return Ok(());
    }
    let player = engine::add_chud(ctx.author().id.get(), name, description)?;
    let content = format!(
        "A chudly **{}** saunters through the door.\n{}",
        player.name, player.description
    );
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let max_buffer = ctx.data().max_buffer_messages;
    post_buffered_message(http, channel_id, max_buffer, &content).await;
    ctx.say("ok").await?;
    Ok(())
}

/// Delete the calling user's chud.
#[poise::command(slash_command)]
pub async fn delete_chud(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    engine::delete_chud(ctx.author().id.get())?;
    ctx.say("ok").await?;
    Ok(())
}

/// Force-assign any user's chud to a quest by Discord user ID and quest ID.
#[poise::command(slash_command)]
pub async fn assign(ctx: Context<'_>, target_user_id: String, quest_id: u32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let target_user_id: u64 = match target_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => { ctx.say("invalid Discord user ID").await?; return Ok(()); }
    };
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    // let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;
    let info = engine::assign_chud_to_quest(&mut *board, target_user_id, quest_id)?;

    // Clone the now-Active quest so we can send it to the worker while the
    // board lock is still held. The worker will do the slow LLM call; the
    // board lock is released before any of that work begins.
    let board_quest = board.quests.iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();

    // Fire-and-forget: the worker task owns the receiver and will process this
    // job as soon as it finishes any preceding one.
    ctx.data().generation_queue.send(engine::GenerationJob::QuestResult { board_quest, player: info.player })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;

    storage::save_board(&*board)?;

    update_board_message(http, channel_id, &mut *board, ctx.data().max_jobs).await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Take an open quest off the board by title.
#[poise::command(slash_command)]
pub async fn take(ctx: Context<'_>, title: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    if storage::load_player(user_id)?.is_none() {
        ctx.say("you don't have a chud").await?;
        return Ok(());
    }
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;
    if board.is_player_busy(user_id) {
        ctx.say("your chud is busy").await?;
        return Ok(());
    }

    let quest_id = board
        .quests
        .iter()
        .filter(|q| !q.has_active())
        .find(|q| q.generated.quest_title.to_lowercase() == title.to_lowercase())
        .map(|q| q.id)
        .ok_or_else(|| anyhow::anyhow!("No open quest found with that title"))?;

    let info = engine::assign_chud_to_quest(&mut *board, user_id, quest_id)?;

    // Clone the now-Active quest and hand it off to the generation worker.
    // This command returns immediately; the LLM call happens in the background.
    let board_quest = board.quests.iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();

    let first_name = info.player.name.split_whitespace().next().unwrap_or(&info.player.name).to_string();
    let content = chud_msg!("chud_takes_job", first_name, info.quest_title);

    ctx.data().generation_queue.send(engine::GenerationJob::QuestResult { board_quest, player: info.player })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;

    ctx.say("ok").await?;

    post_buffered_message(http, channel_id, max_buffer, &content).await;

    update_board_message(http, channel_id, &mut *board, ctx.data().max_jobs).await?;
    Ok(())
}

/// Scout an open quest to assess your chances without committing to it.
#[poise::command(slash_command)]
pub async fn scout(ctx: Context<'_>, title: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    let player = match storage::load_player(user_id)? {
        Some(p) => p,
        None => { ctx.say("you don't have a chud").await?; return Ok(()); }
    };
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;
    if board.is_player_busy(user_id) {
        ctx.say("your chud is busy").await?;
        return Ok(());
    }

    let quest_id = board
        .quests
        .iter()
        .find(|q| q.generated.quest_title.to_lowercase() == title.to_lowercase())
        .map(|q| q.id)
        .ok_or_else(|| anyhow::anyhow!("No quest found with that title"))?;

    if !board.scout(quest_id, user_id) {
        ctx.say("that quest is not available to scout").await?;
        return Ok(());
    }

    storage::save_board(&*board)?;

    let first_name = player.name.split_whitespace().next().unwrap_or(&player.name).to_string();
    let content = format!("**{}** stumbled out the door", first_name);

    ctx.say("ok").await?;

    post_buffered_message(http, channel_id, max_buffer, &content).await;
    Ok(())
}

/// Display your chud's name, description, and current stats.
#[poise::command(slash_command)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    let player = storage::load_player(user_id)?;
    match player {
        None => ctx.say("You don't have a chud.").await?,
        Some(p) => {
            let msg = format!("**{}**\n{}\n{}\n\nYou've got ${} worth of loose change.",
                p.name, p.description,
                p.format_stats(),
                p.cash,
            );
            ctx.say(msg).await?
        }
    };
    Ok(())
}

/// Show how much money you've made from your lucrative chud delegation career.
#[poise::command(slash_command)]
pub async fn cash(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    match storage::load_player(user_id)? {
        None => ctx.say("You don't have a chud.").await?,
        Some(p) if p.cash == 0 => ctx.say("https://tenor.com/view/poor-no-money-gif-24226168").await?,
        Some(p) => ctx.say(format!("You've got {} buckeroos", p.cash)).await?,
    };
    Ok(())
}

/// Check the current status of your chud's active quest.
#[poise::command(slash_command)]
pub async fn job(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    let player = storage::load_player(user_id)?;
    let player = match player {
        None => { ctx.say("You don't have a chud.").await?; return Ok(()); }
        Some(p) => p,
    };
    let board = ctx.data().board.lock().await;
    match board.active_quest_for(user_id) {
        None => ctx.say(format!("**{}** is not on a job.", player.name)).await?,
        Some(q) => {
            let mut msg = format!(
                "**{}** - *{}*\n{}",
                q.generated.quest_title,
                q.generated.quest_giver,
                q.generated.description,
            );
            if q.quest_data.trials.len() > 1 {
                msg.push_str(&format!("\n\n{} is overcoming trials and tribulations.", player.name));
            }
            ctx.say(msg).await?
        }
    };
    Ok(())
}

/// Force-save the current board state to disk.
#[poise::command(slash_command)]
pub async fn save(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let board = ctx.data().board.lock().await;
    engine::save_all(&*board)?;
    ctx.say("ok").await?;
    Ok(())
}

/// Reload the board state from disk, replacing the in-memory state.
#[poise::command(slash_command)]
pub async fn load(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let new_board = engine::load_all()?;
    let mut board = ctx.data().board.lock().await;
    *board = new_board;
    tracing::info!("board reloaded from disk");
    ctx.say("ok").await?;
    Ok(())
}

