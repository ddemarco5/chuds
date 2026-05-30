use std::sync::atomic::Ordering;

use poise::Modal;

use crate::discord::board_ui;
use crate::discord::context::{
    admin_guard, chudmaster_check, parse_difficulty, say_ephemeral, Context, Error,
    GenerateJobModal, WriteJobModal,
};
use crate::game::engine::{self, GenerationJob};
use crate::game::persistence::storage;

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

    let poise::Context::Application(app_ctx) = ctx else {
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
        let queue = ctx.data().job_queue.lock().await;
        let pending = ctx.data().pending_quests.load(Ordering::SeqCst);
        if queue.entries.len() + pending >= ctx.data().max_job_queue {
            say_ephemeral(
                ctx,
                format!("Job queue is full ({} queued max).", ctx.data().max_job_queue),
            )
            .await?;
            return Ok(());
        }
        ctx.data().pending_quests.fetch_add(1, Ordering::SeqCst);
    }
    let goal = data
        .goal
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    tracing::info!(
        author = %ctx.author().name,
        description = %data.description,
        has_goal = goal.is_some(),
        "submitted generate_job"
    );
    ctx.data()
        .generation_queue
        .send(engine::make_quest_creation_job(
            data.description,
            difficulty,
            goal,
        ))
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;
    say_ephemeral(ctx, "ok").await?;
    Ok(())
}

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

    let poise::Context::Application(app_ctx) = ctx else {
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
    {
        let queue = ctx.data().job_queue.lock().await;
        if queue.entries.len() >= ctx.data().max_job_queue {
            say_ephemeral(
                ctx,
                format!("Job queue is full ({} queued max).", ctx.data().max_job_queue),
            )
            .await?;
            return Ok(());
        }
    }
    tracing::info!(
        "{} submitted write_job with description '{}'",
        ctx.author().name,
        data.description
    );
    let mut queue = ctx.data().job_queue.lock().await;
    engine::write_job(
        &ctx.data().generator,
        &mut *queue,
        data.title,
        data.giver,
        data.description,
        data.goal,
        difficulty,
    )
    .await?;
    drop(queue);

    let mut board = ctx.data().board.lock().await;
    let mut queue = ctx.data().job_queue.lock().await;
    engine::refill_board_from_queue(&mut *board, &mut *queue, ctx.data().max_jobs);
    storage::save_board(&*board)?;
    storage::save_job_queue(&*queue)?;
    board_ui::update_board_message(
        &ctx.serenity_context().http,
        ctx.data().channel_id,
        &mut *board,
        ctx.data().max_jobs,
    )
    .await?;
    say_ephemeral(ctx, "ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn delete_job(ctx: Context<'_>, quest_id: u32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let mut board = ctx.data().board.lock().await;
    let mut queue = ctx.data().job_queue.lock().await;
    engine::delete_quest(&mut *board, quest_id)?;
    engine::refill_board_from_queue(&mut *board, &mut *queue, ctx.data().max_jobs);
    storage::save_job_queue(&*queue)?;
    board_ui::update_board_message(
        &ctx.serenity_context().http,
        ctx.data().channel_id,
        &mut *board,
        ctx.data().max_jobs,
    )
    .await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn assign(
    ctx: Context<'_>,
    target_user_id: String,
    quest_id: u32,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let target_user_id: u64 = match target_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };
    let http = &ctx.serenity_context().http;
    let channel_id = ctx.data().channel_id;
    let hospital = storage::load_hospital()?;
    let mut board = ctx.data().board.lock().await;
    let info = engine::assign_chud_to_quest(&mut *board, &hospital, target_user_id, quest_id)?;

    let board_quest = board
        .quests
        .iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();

    ctx.data()
        .generation_queue
        .send(GenerationJob::QuestResult {
            board_quest,
            player: info.player,
        })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;

    storage::save_board(&*board)?;
    board_ui::update_board_message(http, channel_id, &mut *board, ctx.data().max_jobs).await?;
    ctx.say("ok").await?;
    Ok(())
}
