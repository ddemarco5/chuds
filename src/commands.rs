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
    let mut board = ctx.data().board.lock().await;
    engine::run_tick(&ctx.data().generator, &mut *board).await?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board).await?;
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

/// Create a chud for the calling user.
#[poise::command(slash_command)]
pub async fn add_chud(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    engine::add_chud(ctx.author().id.get())?;
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

/// Assign your chud to an open quest by its ID.
#[poise::command(slash_command)]
pub async fn assign(ctx: Context<'_>, quest_id: u32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    engine::assign_chud_to_quest(&mut *board, ctx.author().id.get(), quest_id)?;
    update_board_message(&ctx.serenity_context().http, ctx.data().channel_id, &mut *board).await?;
    ctx.say("ok").await?;
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

