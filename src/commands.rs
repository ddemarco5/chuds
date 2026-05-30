use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use poise::Modal;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, CreateActionRow, CreateAttachment,
    CreateButton, CreateInteractionResponse, CreateInteractionResponseFollowup, CreateMessage,
    EditInteractionResponse, EditMessage, GetMessages, MessageId,
};

use crate::board::{Board, BoardQuest};
use crate::chud_msg;
use crate::hospital::Hospital;
use crate::message_cache::{JobSlot, MessageCache};
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

fn chudmaster_check(data: &Data, user_id: u64, channel_id: u64) -> bool {
    let is_admin = user_id == data.admin_user_id;
    let is_chudmaster = storage::is_chudmaster(user_id).unwrap_or(false);
    (is_admin || is_chudmaster) && channel_id == data.channel_id
}

fn parse_difficulty(raw: &str) -> Result<u8, &'static str> {
    let difficulty: u8 = raw.trim().parse().map_err(|_| "difficulty must be a number")?;
    if !(1..=10).contains(&difficulty) {
        return Err("difficulty must be between 1 and 10");
    }
    Ok(difficulty)
}

async fn say_ephemeral(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

#[derive(Debug, poise::Modal)]
#[name = "Generate Job"]
struct GenerateJobModal {
    #[name = "Description"]
    #[paragraph]
    description: String,
    #[name = "Goal (optional)"]
    #[paragraph]
    goal: Option<String>,
    #[name = "Difficulty (1-10)"]
    #[placeholder = "e.g. 5"]
    difficulty: String,
}

#[derive(Debug, poise::Modal)]
#[name = "Write Job"]
struct WriteJobModal {
    #[name = "Title"]
    title: String,
    #[name = "Giver"]
    giver: String,
    #[name = "Description"]
    #[paragraph]
    description: String,
    #[name = "Goal"]
    #[paragraph]
    goal: String,
    #[name = "Difficulty (1-10)"]
    #[placeholder = "e.g. 5"]
    difficulty: String,
}

// ---------------------------------------------------------------------------
// Channel cleanup
// ---------------------------------------------------------------------------

/// Delete all messages in the channel (both bot and non-bot messages).
pub async fn delete_all_messages_in_channel(
    http: &serenity::Http,
    channel_id: u64,
) {
    let ch = serenity::ChannelId::new(channel_id);
    let messages = match ch.messages(http, GetMessages::new().limit(100)).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(err = %e, "failed to fetch channel messages for purge");
            return;
        }
    };
    if messages.is_empty() {
        return;
    }
    tracing::info!(count = messages.len(), "purging all messages in channel");
    for msg in &messages {
        if let Err(e) = http.delete_message(ch, msg.id, None).await {
            tracing::warn!(msg_id = msg.id.get(), err = %e, "failed to delete message during purge");
        }
    }
}

/// Validate that all cached message IDs still exist in Discord.
/// Returns true if all exist, false if any are missing.
pub async fn validate_cached_messages_exist(
    http: &serenity::Http,
    channel_id: u64,
    board: &Board,
    cache: &MessageCache,
) -> bool {
    let ch = serenity::ChannelId::new(channel_id);

    // Collect all message IDs to check
    let mut message_ids = Vec::new();

    // Chudlerboard message ID (stored in Board)
    if let Some(id) = board.chudlerboard_message_id {
        message_ids.push(("chudlerboard", id));
    }

    // Header message ID (stored in MessageCache)
    if let Some(id) = cache.header_message_id {
        message_ids.push(("header", id));
    }

    // Divider message ID (stored in MessageCache)
    if let Some(id) = cache.divider_message_id {
        message_ids.push(("divider", id));
    }

    // Job slot message IDs (stored in MessageCache)
    for slot in &cache.slots {
        if let Some(id) = slot.message_id {
            message_ids.push(("slot", id));
        }
    }

    if message_ids.is_empty() {
        return true; // No cached messages to validate
    }

    // Check each message by attempting to fetch it
    for (kind, id) in message_ids {
        let msg_id = serenity::MessageId::new(id);
        match http.get_message(ch, msg_id).await {
            Ok(_) => continue, // Message exists
            Err(e) => {
                tracing::warn!(kind, msg_id = id, err = %e, "cached message not found");
                return false; // At least one message is missing
            }
        }
    }

    true
}

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
            if q.has_active() {
                // Taken: collapse to a struck-through title to save channel space.
                format!("~~{}~~", title)
            } else {
                let giver = &q.generated.quest_giver;
                let desc = &q.generated.description;
                let reward = q.generated.reward;
                format!("**{}** - *{}* | ${}\n{}", title, giver, reward, desc)
            }
        }
        None => "*Nothing posted here*".to_string(),
    }
}

/// Components for a job slot message: a "Take" button on open quests plus a
/// "Scout" button on any posted quest (open or already taken). Empty slots get
/// no components; passing an empty vec on edit clears stale buttons.
fn job_slot_components(quest: Option<&BoardQuest>) -> Vec<CreateActionRow> {
    match quest {
        Some(q) => {
            let mut buttons = Vec::new();
            if !q.has_active() {
                buttons.push(
                    CreateButton::new(format!("take:{}", q.id))
                        .label("Take")
                        .style(ButtonStyle::Primary),
                );
            }
            buttons.push(
                CreateButton::new(format!("scout:{}", q.id))
                    .label("Scout")
                    .style(ButtonStyle::Secondary),
            );
            vec![CreateActionRow::Buttons(buttons)]
        }
        None => vec![],
    }
}

/// Post-assignment tail shared by `/take` and the Take button: queue the LLM
/// job, announce the take in-channel, and refresh the board messages.
async fn announce_taken_quest(
    http: &serenity::Http,
    board: &mut Board,
    data: &Data,
    info: engine::AssignInfo,
    quest_id: u32,
) -> anyhow::Result<()> {
    let board_quest = board
        .quests
        .iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();

    let first_name = info.player.name.split_whitespace().next().unwrap_or(&info.player.name).to_string();
    let content = chud_msg!("chud_takes_job", first_name, info.quest_title);

    data.generation_queue
        .send(engine::GenerationJob::QuestResult { board_quest, player: info.player })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;

    post_buffered_message(http, data.channel_id, data.max_buffer_messages, &content).await;
    update_board_message(http, data.channel_id, board, data.max_jobs).await?;
    Ok(())
}

/// Send an ephemeral followup to a component interaction (visible only to the clicker).
async fn ephemeral_followup(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    content: &str,
) -> anyhow::Result<()> {
    interaction
        .create_followup(
            &ctx.http,
            CreateInteractionResponseFollowup::new().ephemeral(true).content(content),
        )
        .await?;
    Ok(())
}

/// Send an ephemeral followup with a heal button.
async fn ephemeral_with_heal_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    content: &str,
    user_id: u64,
    heal_price: u32,
) -> anyhow::Result<()> {
    let heal_label = chud_msg!("heal_button_label", heal_price);
    let button = CreateButton::new(format!("heal:{}", user_id))
        .label(heal_label)
        .style(ButtonStyle::Primary);
    let row = CreateActionRow::Buttons(vec![button]);

    interaction
        .create_followup(
            &ctx.http,
            CreateInteractionResponseFollowup::new()
                .ephemeral(true)
                .content(content)
                .components(vec![row]),
        )
        .await?;
    Ok(())
}

/// Ephemeral response when a hospitalized player tries to take or scout a job.
async fn ephemeral_hospitalized_response(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    player: &crate::player::Player,
    user_id: u64,
) -> anyhow::Result<()> {
    let hospital = Hospital::load()?;
    let days = hospital.get_ticks_remaining(user_id).unwrap_or(0);
    let days_remaining = if days == 1 {
        "1 day".to_string()
    } else {
        format!("{days} days")
    };

    if let Some(heal_price) = hospital.get_heal_price(user_id) {
        if player.cash >= heal_price {
            let msg = chud_msg!("busy_hospitalized", player.name, &days_remaining);
            ephemeral_with_heal_button(ctx, interaction, &msg, user_id, heal_price).await?;
        } else {
            let msg = chud_msg!("heal_insufficient_funds", player.name);
            ephemeral_followup(ctx, interaction, &msg).await?;
        }
    } else {
        let msg = chud_msg!("busy_hospitalized", player.name, &days_remaining);
        ephemeral_followup(ctx, interaction, &msg).await?;
    }
    Ok(())
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
                .edit_message(
                    ch,
                    MessageId::new(msg_id),
                    &EditMessage::new().content(&expected).components(job_slot_components(quest)),
                    vec![],
                )
                .await
                .is_ok();
            if !ok {
                tracing::warn!(msg_id, slot = i, "failed to edit job slot message, posting new one");
                match http.send_message(ch, vec![], &CreateMessage::new().content(&expected).components(job_slot_components(quest))).await {
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
            match http.send_message(ch, vec![], &CreateMessage::new().content(&expected).components(job_slot_components(quest))).await {
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

    {
        let board_guard = board.lock().await;
        if board_guard.active_quest_count() > 0 {
            let new_day_msg = chud_msg!("new_day");
            post_buffered_message(http, channel_id, max_buffer, &new_day_msg).await;
        }
    }

    // Process hospital tick first
    let mut hospital = crate::hospital::Hospital::load()?;
    let released_messages = hospital.tick();
    for msg in &released_messages {
        post_buffered_message(http, channel_id, max_buffer, msg).await;
    }
    hospital.save()?;

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

        let first_name = qr.player_name.split_whitespace().next().unwrap_or(&qr.player_name).to_string();
        let return_key = if qr.hospitalized {
            "return_hospitalized"
        } else if qr.result.passed {
            "return_passed"
        } else {
            "return_failed"
        };
        let return_msg = chud_msg!(return_key, first_name);
        post_buffered_message(http, channel_id, max_buffer, &return_msg).await;

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

                // Send hospital notification DM if player was hospitalized
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

    for sr in &scouted {
        let first_name = sr.player_name.split_whitespace().next().unwrap_or(&sr.player_name).to_string();
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
pub async fn generate_job(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get();
    let channel_id = ctx.channel_id().get();
    if !chudmaster_check(ctx.data(), user_id, channel_id) {
        tracing::info!("Non-CM tried to submit a generate job");
        tracing::warn!(user = user_id, channel = channel_id, "unauthorized or off-channel command ignored");
        say_ephemeral(ctx, "you don't have permission for this command (sorry bud)").await?;
        return Ok(());
    }

    let Context::Application(app_ctx) = ctx else {
        return Ok(());
    };
    let Some(data) = GenerateJobModal::execute(app_ctx).await? else {
        return Ok(());
    };

    let difficulty = match parse_difficulty(&data.difficulty) {
        Ok(d) => d,
        Err(msg) => {
            say_ephemeral(ctx, msg).await?;
            return Ok(());
        }
    };
    {
        let board = ctx.data().board.lock().await;
        let pending = ctx.data().pending_quests.load(Ordering::SeqCst);
        if board.quests.len() + pending >= ctx.data().max_jobs {
            say_ephemeral(ctx, format!("Board is full ({} jobs max).", ctx.data().max_jobs)).await?;
            return Ok(());
        }
        ctx.data().pending_quests.fetch_add(1, Ordering::SeqCst);
    }
    let goal = data.goal.filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string());
    tracing::info!(
        author = %ctx.author().name,
        description = %data.description,
        has_goal = goal.is_some(),
        "submitted generate_job"
    );
    ctx.data().generation_queue.send(engine::make_quest_creation_job(data.description, difficulty, goal))
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;
    say_ephemeral(ctx, "ok").await?;
    Ok(())
}

/// Post a user-authored quest; only trial situations are LLM-generated.
#[poise::command(slash_command)]
pub async fn write_job(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get();
    let channel_id = ctx.channel_id().get();
    if !chudmaster_check(ctx.data(), user_id, channel_id) {
        tracing::info!("Non-CM tried to submit a write job");
        tracing::warn!(user = user_id, channel = channel_id, "unauthorized or off-channel command ignored");
        say_ephemeral(ctx, "you don't have permission for this command (sorry bud)").await?;
        return Ok(());
    }

    let Context::Application(app_ctx) = ctx else {
        return Ok(());
    };
    let Some(data) = WriteJobModal::execute(app_ctx).await? else {
        return Ok(());
    };

    let difficulty = match parse_difficulty(&data.difficulty) {
        Ok(d) => d,
        Err(msg) => {
            say_ephemeral(ctx, msg).await?;
            return Ok(());
        }
    };
    let mut board = ctx.data().board.lock().await;
    if board.quests.len() >= ctx.data().max_jobs {
        say_ephemeral(ctx, format!("Board is full ({} jobs max).", ctx.data().max_jobs)).await?;
        return Ok(());
    }
    tracing::info!("{} submitted write_job with description '{}'", ctx.author().name, data.description);
    engine::write_job(
        &ctx.data().generator,
        &mut *board,
        data.title,
        data.giver,
        data.description,
        data.goal,
        difficulty,
    ).await?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board, ctx.data().max_jobs).await?;
    say_ephemeral(ctx, "ok").await?;
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

/// Handle a click on a job slot's "Take" button.
///
/// Acks immediately (silent) so the slow board refresh that follows can't trip
/// Discord's 3-second response deadline; all user feedback uses ephemeral
/// followups visible only to the clicker.
pub async fn handle_take_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    let quest_id: u32 = match interaction.data.custom_id.strip_prefix("take:").and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return Ok(()),
    };
    let user_id = interaction.user.id.get();

    let player = match storage::load_player(user_id)? {
        Some(p) => p,
        None => {
            ephemeral_followup(ctx, interaction, "You don't have a chud.").await?;
            return Ok(());
        }
    };

    let mut board = data.board.lock().await;
    if let Some(reason) = board.is_player_busy(user_id)? {
        match reason {
            engine::BusyReason::ActiveQuest { quest_title } => {
                let msg = chud_msg!("busy_active_quest", player.name, quest_title);
                ephemeral_followup(ctx, interaction, &msg).await?;
            }
            engine::BusyReason::Scouting => {
                let msg = chud_msg!("busy_scouting", player.name);
                ephemeral_followup(ctx, interaction, &msg).await?;
            }
            engine::BusyReason::Hospitalized => {
                ephemeral_hospitalized_response(ctx, interaction, &player, user_id).await?;
            }
        }
        return Ok(());
    }

    // assign_chud_to_quest guards quest existence/availability internally.
    let info = match engine::assign_chud_to_quest(&mut *board, user_id, quest_id) {
        Ok(info) => info,
        Err(_) => {
            ephemeral_followup(ctx, interaction, "That job is no longer available.").await?;
            return Ok(());
        }
    };

    announce_taken_quest(&ctx.http, &mut *board, data, info, quest_id).await?;
    Ok(())
}

/// Handle a click on a job slot's "Scout" button.
///
/// Mirrors [`handle_take_button`]: acks immediately, then validates and applies
/// the scout, surfacing any problem as an ephemeral followup. Scouting doesn't
/// change a slot's appearance, so no board refresh is needed.
pub async fn handle_scout_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    let quest_id: u32 = match interaction.data.custom_id.strip_prefix("scout:").and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return Ok(()),
    };
    let user_id = interaction.user.id.get();

    let player = match storage::load_player(user_id)? {
        Some(p) => p,
        None => {
            ephemeral_followup(ctx, interaction, "You don't have a chud.").await?;
            return Ok(());
        }
    };

    let mut board = data.board.lock().await;
    if let Some(reason) = board.is_player_busy(user_id)? {
        match reason {
            engine::BusyReason::ActiveQuest { quest_title } => {
                let msg = chud_msg!("busy_active_quest", player.name, quest_title);
                ephemeral_followup(ctx, interaction, &msg).await?;
            }
            engine::BusyReason::Scouting => {
                let msg = chud_msg!("busy_scouting", player.name);
                ephemeral_followup(ctx, interaction, &msg).await?;
            }
            engine::BusyReason::Hospitalized => {
                ephemeral_hospitalized_response(ctx, interaction, &player, user_id).await?;
            }
        }
        return Ok(());
    }

    if !board.scout(quest_id, user_id) {
        ephemeral_followup(ctx, interaction, "That job is no longer available.").await?;
        return Ok(());
    }
    storage::save_board(&*board)?;
    drop(board);

    let first_name = player.name.split_whitespace().next().unwrap_or(&player.name).to_string();
    let content = format!("**{}** stumbled out the door", first_name);
    post_buffered_message(&ctx.http, data.channel_id, data.max_buffer_messages, &content).await;
    Ok(())
}

/// Handle a click on the "Heal chud" button.
///
/// The button is only shown when the player owns the chud, the chud is hospitalized,
/// and the player has sufficient funds. Any violation of these is a bug.
pub async fn handle_heal_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    let user_id: u64 = interaction
        .data
        .custom_id
        .strip_prefix("heal:")
        .and_then(|s| s.parse().ok())
        .expect("valid heal button ID");

    // Button is only shown to the chud owner
    assert_eq!(
        interaction.user.id.get(),
        user_id,
        "heal button clicked by non-owner"
    );

    let mut player = storage::load_player(user_id)?
        .expect("player with heal button exists");

    let mut hospital = Hospital::load()?;
    let heal_price = hospital
        .get_heal_price(user_id)
        .expect("chud with heal button is hospitalized");

    // Button is only shown when player can afford it
    assert!(
        player.cash >= heal_price,
        "player cannot afford heal they were offered"
    );

    // Deduct cash and save
    player.cash -= heal_price;
    storage::save_player(&player)?;

    // Release from hospital, announce in main channel, and save
    let chud_name = hospital
        .get_chud_name(user_id)
        .expect("hospitalized chud has name")
        .to_string();
    let release_msg = hospital
        .release(user_id)
        .expect("hospitalized chud exists");
    hospital.save()?;
    post_buffered_message(
        &ctx.http,
        data.channel_id,
        data.max_buffer_messages,
        &release_msg,
    )
    .await;

    let msg = chud_msg!("heal_success", chud_name, heal_price);
    interaction
        .edit_response(
            &ctx.http,
            EditInteractionResponse::new().content(msg).components(vec![]),
        )
        .await?;

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

