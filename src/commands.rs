use std::sync::Arc;

use poise::serenity_prelude::{self as serenity, CreateMessage, EditMessage, MessageId};

use crate::board::{Board, QuestStatus};
use crate::engine;
use crate::quest_generator::QuestGenerator;
use crate::storage;

pub struct Data {
    pub generator: Arc<QuestGenerator>,
    pub board: Arc<tokio::sync::Mutex<Board>>,
    pub admin_user_id: u64,
    pub channel_id: u64,
    pub max_buffer_messages: usize,
    pub max_jobs: usize,
    /// Sender half of the generation worker channel (see main.rs).
    /// Commands drop a GenerationJob here after assigning a quest; the worker
    /// task runs the LLM call in the background and writes the result to
    /// board.completed_results. The tick loop picks it up later.
    pub generation_queue: tokio::sync::mpsc::UnboundedSender<engine::GenerationJob>,
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
        ctx.say("ok").await.ok();
    }
    ok
}

// ---------------------------------------------------------------------------
// Board message helpers
// ---------------------------------------------------------------------------

fn format_job_slot(board: &Board, index: usize) -> String {
    match board.quests.get(index) {
        Some(q) => {
            let title = &q.generated.quest_title;
            let giver = &q.generated.quest_giver;
            let desc = &q.generated.description;
            match &q.status {
                QuestStatus::Open => format!("**{}** - *{}*\n{}", title, giver, desc),
                QuestStatus::Active { .. } => {
                    format!("~~**{}** - *{}*~~\n~~{}~~", title, giver, desc)
                }
            }
        }
        None => "*Nothing posted here*".to_string(),
    }
}

async fn post_buffered_message(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    max_buffer: usize,
    content: &str,
) {
    let ch = serenity::ChannelId::new(channel_id);
    if board.pending_deletes.len() >= max_buffer {
        let old_id = board.pending_deletes.remove(0);
        if let Err(e) = http.delete_message(ch, MessageId::new(old_id), None).await {
            tracing::warn!(msg_id = old_id, err = %e, "failed to evict oldest buffered message");
        }
    }
    match http.send_message(ch, vec![], &CreateMessage::new().content(content)).await {
        Ok(msg) => board.pending_deletes.push(msg.id.get()),
        Err(e) => tracing::warn!(err = %e, "failed to post buffered message"),
    }
}

fn format_dm_report(
    player_name: &str,
    result: &crate::quest_result::QuestResult,
    player: &crate::player::Player,
    level_up: &crate::player::LevelUp,
) -> String {
    let mut out = String::new();
    let outcome = if result.passed { "PASSED" } else { "FAILED" };

    out.push_str(&format!("{}\n\n", player.format_stats()));

    out.push_str(&format!(
        "**{}** - {}\n{}\n",
        result.quest_title, result.quest_giver, result.quest_description
    ));

    for (i, trial) in result.trials.iter().enumerate() {
        let pass_str = if trial.passed { "\u{2705}" } else { "\u{274c}" };
        let brain = if trial.chose_optimal { " \u{1F9E0}" } else { "" };
        out.push_str(&format!(
            "\n**Trial {}** - {}\n{} {} | {} rolled {} vs {}{}\n*{}*\n",
            i + 1,
            trial.situation,
            pass_str,
            trial.stat_used.label(),
            player_name,
            trial.player_roll,
            trial.trial_roll,
            brain,
            trial.narrative,
        ));
    }
    out.push_str(&format!("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n**{}**\n{}", outcome, result.summary));

    if level_up.any() {
        fn fmt_stat(levelled: bool, val: u8) -> String {
            if levelled {
                format!("{} -> **{}**", val - 1, val)
            } else {
                val.to_string()
            }
        }
        out.push_str(&format!(
            "\n{} has improved! - Strength {}, Smarts {}, Stealth {}, Experience {}",
            player_name,
            fmt_stat(level_up.str_up, player.strength),
            fmt_stat(level_up.smt_up, player.smarts),
            fmt_stat(level_up.sth_up, player.stealth),
            fmt_stat(level_up.exp_up, player.experience),
        ));
    }
    out
}

fn format_chudlerboard() -> String {
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

    fn leaders_for(
        players: &[crate::player::Player],
        accessor: fn(&crate::player::Player) -> u8,
    ) -> Vec<String> {
        let max_val = players.iter().map(|p| accessor(p)).max().unwrap_or(0);
        let mut names: Vec<String> = players
            .iter()
            .filter(|p| accessor(p) == max_val)
            .map(|p| p.name.clone())
            .collect();
        names.sort();
        names
    }

    let stat_defs: [(&str, Vec<String>); 4] = [
        ("strongest", leaders_for(&players, |p| p.strength)),
        ("smartest",  leaders_for(&players, |p| p.smarts)),
        ("sneakiest", leaders_for(&players, |p| p.stealth)),
        ("pro",       leaders_for(&players, |p| p.experience)),
    ];

    let mut groups: Vec<(Vec<String>, Vec<&str>)> = Vec::new();
    for (adjective, names) in &stat_defs {
        if let Some(group) = groups.iter_mut().find(|(n, _)| n == names) {
            group.1.push(adjective);
        } else {
            groups.push((names.clone(), vec![adjective]));
        }
    }

    let mut lines = Vec::new();
    for (names, titles) in &groups {
        let name_str = match names.len() {
            1 => names[0].clone(),
            2 => format!("{} and {}", names[0], names[1]),
            _ => {
                let (last, rest) = names.split_last().unwrap();
                format!("{}, and {}", rest.join(", "), last)
            }
        };
        let verb = if names.len() == 1 { "is" } else { "are" };
        let title_str = match titles.len() {
            1 => format!("the {}", titles[0]),
            2 => format!("the {} and the {}", titles[0], titles[1]),
            _ => {
                let (last, rest) = titles.split_last().unwrap();
                let parts: Vec<String> = rest.iter().map(|t| format!("the {}", t)).collect();
                format!("{}, and the {}", parts.join(", "), last)
            }
        };
        lines.push(format!("{} {} {}", name_str, verb, title_str));
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

    // --- Chudlerboard message ---
    let cb_content = {
        let cb = format_chudlerboard();
        if cb.is_empty() { "*No chuds yet.*".to_string() } else { cb }
    };
    if let Some(msg_id) = board.chudlerboard_message_id {
        let ok = http
            .edit_message(ch, MessageId::new(msg_id), &EditMessage::new().content(&cb_content), vec![])
            .await
            .is_ok();
        if !ok {
            tracing::warn!(msg_id, "failed to edit chudlerboard message, posting new one");
            match http.send_message(ch, vec![], &CreateMessage::new().content(&cb_content)).await {
                Ok(msg) => { board.chudlerboard_message_id = Some(msg.id.get()); board_dirty = true; }
                Err(e) => tracing::warn!(err = %e, "failed to post chudlerboard message"),
            }
        } else {
            tracing::debug!(msg_id, "chudlerboard message edited");
        }
    } else {
        match http.send_message(ch, vec![], &CreateMessage::new().content(&cb_content)).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "chudlerboard message posted");
                board.chudlerboard_message_id = Some(msg.id.get());
                board_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post chudlerboard message"),
        }
    }

    // --- Job slot messages ---
    for i in 0..max_jobs {
        let slot_content = format_job_slot(board, i);
        if let Some(&msg_id) = board.job_slot_message_ids.get(i) {
            let ok = http
                .edit_message(ch, MessageId::new(msg_id), &EditMessage::new().content(&slot_content), vec![])
                .await
                .is_ok();
            if !ok {
                tracing::warn!(msg_id, slot = i, "failed to edit job slot message, posting new one");
                match http.send_message(ch, vec![], &CreateMessage::new().content(&slot_content)).await {
                    Ok(msg) => { board.job_slot_message_ids[i] = msg.id.get(); board_dirty = true; }
                    Err(e) => tracing::warn!(err = %e, slot = i, "failed to post job slot message"),
                }
            } else {
                tracing::debug!(msg_id, slot = i, "job slot message edited");
            }
        } else {
            match http.send_message(ch, vec![], &CreateMessage::new().content(&slot_content)).await {
                Ok(msg) => {
                    tracing::info!(msg_id = msg.id.get(), slot = i, "job slot message posted");
                    board.job_slot_message_ids.push(msg.id.get());
                    board_dirty = true;
                }
                Err(e) => tracing::warn!(err = %e, slot = i, "failed to post job slot message"),
            }
        }
    }

    // --- Divider message ---
    const DIVIDER: &str = "\u{200B}\n\u{200B}";
    if let Some(msg_id) = board.divider_message_id {
        let ok = http
            .edit_message(ch, MessageId::new(msg_id), &EditMessage::new().content(DIVIDER), vec![])
            .await
            .is_ok();
        if !ok {
            tracing::warn!(msg_id, "failed to edit divider message, posting new one");
            match http.send_message(ch, vec![], &CreateMessage::new().content(DIVIDER)).await {
                Ok(msg) => { board.divider_message_id = Some(msg.id.get()); board_dirty = true; }
                Err(e) => tracing::warn!(err = %e, "failed to post divider message"),
            }
        }
    } else {
        match http.send_message(ch, vec![], &CreateMessage::new().content(DIVIDER)).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "divider message posted");
                board.divider_message_id = Some(msg.id.get());
                board_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post divider message"),
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
) -> anyhow::Result<()> {
    let mut board = board.lock().await;

    let resolved = engine::run_tick(&mut *board).await?;

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
        post_buffered_message(http, channel_id, &mut *board, max_buffer, &content).await;

        let dm_content = format_dm_report(&qr.player_name, &qr.result, &qr.player, &qr.level_up);
        let dm_map = serde_json::json!({ "recipient_id": qr.discord_user_id.to_string() });
        match http.create_private_channel(&dm_map).await {
            Ok(dm) => {
                if let Err(e) = http.send_message(dm.id, vec![], &CreateMessage::new().content(&dm_content)).await {
                    tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to DM quest report");
                }
            }
            Err(e) => tracing::warn!(discord_user_id = qr.discord_user_id, err = %e, "failed to open DM channel"),
        }
    }

    if !resolved.is_empty() {
        storage::save_board(&*board)?;
    }

    update_board_message(http, channel_id, &mut *board, max_jobs).await?;
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
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    if board.quests.len() >= ctx.data().max_jobs {
        ctx.say(format!("Board is full ({} jobs max).", ctx.data().max_jobs)).await?;
        return Ok(());
    }
    engine::generate_job(&ctx.data().generator, &mut *board, description, difficulty).await?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board, ctx.data().max_jobs).await?;
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
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    if board.quests.len() >= ctx.data().max_jobs {
        ctx.say(format!("Board is full ({} jobs max).", ctx.data().max_jobs)).await?;
        return Ok(());
    }
    engine::write_job(&ctx.data().generator, &mut *board, title, giver, description, goal, difficulty).await?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board, ctx.data().max_jobs).await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Remove a quest from the board by its ID.
#[poise::command(slash_command)]
pub async fn delete_quest(ctx: Context<'_>, quest_id: u32) -> Result<(), Error> {
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
pub async fn add_chud(ctx: Context<'_>, target_user_id: u64, name: String, description: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    engine::add_chud(target_user_id, name, description)?;
    ctx.say("ok").await?;
    Ok(())
}

/// Create your chud and join the game.
#[poise::command(slash_command)]
pub async fn chud(ctx: Context<'_>, name: String, description: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let player = engine::add_chud(ctx.author().id.get(), name, description)?;
    let content = format!(
        "A chudly **{}** saunters through the door.\n{}",
        player.name, player.description
    );
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;
    post_buffered_message(http, channel_id, &mut *board, max_buffer, &content).await;
    storage::save_board(&*board)?;
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
pub async fn assign(ctx: Context<'_>, target_user_id: u64, quest_id: u32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
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
    ctx.data().generation_queue.send(engine::GenerationJob { board_quest, player: info.player })
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
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;

    let quest_id = board
        .open_quests()
        .find(|q| q.generated.quest_title.to_lowercase() == title.to_lowercase())
        .map(|q| q.id)
        .ok_or_else(|| anyhow::anyhow!("No open quest found with that title"))?;

    let info = engine::assign_chud_to_quest(&mut *board, ctx.author().id.get(), quest_id)?;

    // Clone the now-Active quest and hand it off to the generation worker.
    // This command returns immediately; the LLM call happens in the background.
    let board_quest = board.quests.iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();

    let first_name = info.player.name.split_whitespace().next().unwrap_or(&info.player.name).to_string();
    let content = format!("**{}** ripped **{}** off the board", first_name, info.quest_title);

    ctx.data().generation_queue.send(engine::GenerationJob { board_quest, player: info.player })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;

    ctx.say("ok").await?;

    post_buffered_message(http, channel_id, &mut *board, max_buffer, &content).await;
    storage::save_board(&*board)?;

    update_board_message(http, channel_id, &mut *board, ctx.data().max_jobs).await?;
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
            let msg = format!("**{}**\n{}\n{}", p.name, p.description, p.format_stats());
            ctx.say(msg).await?
        }
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

