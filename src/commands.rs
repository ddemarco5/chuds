use poise::serenity_prelude::{self as serenity, CreateMessage, EditMessage, MessageId};

use crate::board::{Board, QuestStatus};
use crate::engine;
use crate::quest_generator::QuestGenerator;
use crate::storage;

pub struct Data {
    pub generator: QuestGenerator,
    pub board: tokio::sync::Mutex<Board>,
    pub admin_user_id: u64,
    pub channel_id: u64,
    pub max_buffer_messages: usize,
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

fn format_board(board: &Board) -> String {
    if board.quests.is_empty() {
        return "The job board is as empty as it's ever been.".to_string();
    }
    board
        .quests
        .iter()
        .map(|q| {
            let title = &q.generated.quest_title;
            let giver = &q.generated.quest_giver;
            let desc = &q.generated.description;
            match &q.status {
                QuestStatus::Open => format!("**{}** - *{}*\n{}", title, giver, desc),
                QuestStatus::Active { .. } => {
                    format!("~~**{}** - *{}*~~\n~~{}~~", title, giver, desc)
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
) -> String {
    let mut out = String::new();
    let outcome = if result.passed { "PASSED" } else { "FAILED" };
    out.push_str(&format!(
        "**{}** - {}\n{}\n\n",
        result.quest_title, result.quest_giver, result.quest_description
    ));
    out.push_str(&format!("{}", player.format_stats()));
    out.push_str("\n\n");
    for (i, trial) in result.trials.iter().enumerate() {
        let pass_str = if trial.passed { "\u{2705}" } else { "\u{274c}" };
        let brain = if trial.chose_optimal { " \u{1F9E0}" } else { "" };
        out.push_str(&format!(
            "**Trial {}** - {}\n  {} {} | {} rolled {} vs {}{}\n  *{}*\n",
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
    out
}

pub(crate) async fn update_board_message(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
) -> anyhow::Result<()> {
    let content = format_board(board);
    let ch = serenity::ChannelId::new(channel_id);

    if let Some(msg_id) = board.board_message_id {
        let edit_result = http
            .edit_message(
                ch,
                MessageId::new(msg_id),
                &EditMessage::new().content(&content),
                vec![],
            )
            .await;
        if edit_result.is_ok() {
            tracing::debug!(msg_id, "board message edited");
            return Ok(());
        }
        tracing::warn!(msg_id, "failed to edit board message, posting new one");
    }

    let msg = http
        .send_message(ch, vec![], &CreateMessage::new().content(&content))
        .await
        .map_err(|e| anyhow::anyhow!("failed to post board message: {e}"))?;

    board.board_message_id = Some(msg.id.get());
    storage::save_board(board)?;
    tracing::info!(msg_id = msg.id.get(), "board message posted");
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Advance the board by one tick, resolving any due quests via the LLM.
#[poise::command(slash_command)]
pub async fn tick(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;

    let resolved = engine::run_tick(&ctx.data().generator, &mut *board).await?;

    for qr in &resolved {
        let content = format!( "{}", qr.summary );
        post_buffered_message(http, channel_id, &mut *board, max_buffer, &content).await;

        let dm_content = format_dm_report(&qr.player_name, &qr.result, &qr.player);
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

    update_board_message(http, channel_id, &mut *board).await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Generate a new quest via the LLM and post it to the board.
#[poise::command(slash_command)]
pub async fn add_quest(
    ctx: Context<'_>,
    description: String,
    difficulty: u8,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    engine::add_quest(&ctx.data().generator, &mut *board, description, difficulty).await?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board).await?;
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
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board).await?;
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
    let msg = format!(
        "**{}** has entered the world!\n{}",
        player.name, player.description
    );
    ctx.say(msg).await?;
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
    let max_buffer = ctx.data().max_buffer_messages;
    let mut board = ctx.data().board.lock().await;
    let info = engine::assign_chud_to_quest(&mut *board, target_user_id, quest_id)?;

    let content = format!("**{}** ripped **{}** off the board", info.player_name, info.quest_title);
    post_buffered_message(http, channel_id, &mut *board, max_buffer, &content).await;
    storage::save_board(&*board)?;

    update_board_message(http, channel_id, &mut *board).await?;
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
        .quests
        .iter()
        .find(|q| {
            matches!(&q.status, crate::board::QuestStatus::Open)
                && q.generated.quest_title.to_lowercase() == title.to_lowercase()
        })
        .map(|q| q.id)
        .ok_or_else(|| anyhow::anyhow!("No open quest found with that title"))?;

    let info = engine::assign_chud_to_quest(&mut *board, ctx.author().id.get(), quest_id)?;

    let content = format!("**{}** ripped **{}** off the board", info.player_name, info.quest_title);
    post_buffered_message(http, channel_id, &mut *board, max_buffer, &content).await;
    storage::save_board(&*board)?;

    update_board_message(http, channel_id, &mut *board).await?;
    ctx.say("ok").await?;
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

