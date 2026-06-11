use std::sync::atomic::Ordering;

use poise::Modal;

use crate::discord::guild_hall;
use crate::discord::context::{
    admin_guard, chudmaster_check, parse_difficulty, parse_difficulty_optional, require_playing,
    say_ephemeral, Context, Error, GenerateJobModal, WriteJobModal,
};
use crate::game::engine;
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
    // Intentionally not gated on the Playing phase: chudmasters pre-fill the job queue during
    // attract so jobs are ready the moment the game starts.

    let poise::Context::Application(app_ctx) = ctx else {
        return Ok(());
    };
    let Some(data) = GenerateJobModal::execute(app_ctx).await? else {
        return Ok(());
    };

    let difficulty = match parse_difficulty_optional(data.difficulty.as_deref()) {
        Ok(d) => d,
        Err(msg) => {
            say_ephemeral(ctx, msg).await?;
            return Ok(());
        }
    };
    let max_job_queue = ctx.data().runtime.max_job_queue;
    let queued_count = {
        let queue = ctx.data().runtime.job_queue.lock().await;
        let pending = ctx.data().runtime.pending_quests.load(Ordering::SeqCst);
        if queue.entries.len() + pending >= max_job_queue {
            say_ephemeral(
                ctx,
                format!("Job queue is full ({} queued max).", max_job_queue),
            )
            .await?;
            return Ok(());
        }
        ctx.data().runtime.pending_quests.fetch_add(1, Ordering::SeqCst);
        queue.entries.len() + pending + 1
    };
    let goal = data
        .goal
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    tracing::info!(
        author = %ctx.author().name,
        description = %data.description,
        has_goal = goal.is_some(),
        difficulty,
        auto_difficulty = data.difficulty.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none(),
        "submitted generate_job"
    );
    ctx.data()
        .runtime
        .generation_queue
        .send(engine::make_quest_creation_job(
            data.description,
            difficulty,
            goal,
        ))
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))?;
    say_ephemeral(
        ctx,
        format!("{}/{} jobs in queue", queued_count, max_job_queue),
    )
    .await?;
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
    // Intentionally not gated on the Playing phase: chudmasters pre-fill the job queue during
    // attract. The board redraw below is skipped while we're not playing.

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
    let max_job_queue = ctx.data().runtime.max_job_queue;
    {
        let queue = ctx.data().runtime.job_queue.lock().await;
        if queue.entries.len() >= max_job_queue {
            say_ephemeral(
                ctx,
                format!("Job queue is full ({} queued max).", max_job_queue),
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
    let (quest_data, generated) = engine::generate_written_job(
        &ctx.data().runtime.generator,
        data.title,
        data.giver,
        data.description,
        data.goal,
        difficulty,
    )
    .await?;
    let mut queue = ctx.data().runtime.job_queue.lock().await;
    engine::enqueue_quest(&mut *queue, quest_data, generated)?;
    drop(queue);

    let max_jobs = ctx.data().runtime.max_jobs;
    let channel_id = ctx.data().runtime.channel_id;
    let board = {
        let mut board = ctx.data().runtime.board.lock().await;
        let mut queue = ctx.data().runtime.job_queue.lock().await;
        engine::refill_board(
            &mut *board,
            &mut *queue,
            max_jobs,
            ctx.data().runtime.job_timeout_tick,
            Some(&ctx.data().runtime.generation_queue),
            Some(&ctx.data().runtime.pending_quests),
        );
        storage::save_board(&*board)?;
        storage::save_job_queue(&*queue)?;
        board.clone()
    };
    // Only redraw the board while playing; attract/complete own the channel.
    if ctx.data().runtime.is_playing().await {
        guild_hall::update_board_message(
            &ctx.serenity_context().http,
            channel_id,
            &board,
            max_jobs,
            None,
        )
        .await?;
    }
    say_ephemeral(ctx, "ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn delete_job(ctx: Context<'_>, quest_id: u32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    if !require_playing(ctx).await {
        return Ok(());
    }
    let max_jobs = ctx.data().runtime.max_jobs;
    let channel_id = ctx.data().runtime.channel_id;
    let board = {
        let mut board = ctx.data().runtime.board.lock().await;
        let mut queue = ctx.data().runtime.job_queue.lock().await;
        engine::delete_quest(&mut *board, quest_id)?;
        engine::refill_board(
            &mut *board,
            &mut *queue,
            max_jobs,
            ctx.data().runtime.job_timeout_tick,
            Some(&ctx.data().runtime.generation_queue),
            Some(&ctx.data().runtime.pending_quests),
        );
        storage::save_job_queue(&*queue)?;
        board.clone()
    };
    guild_hall::update_board_message(
        &ctx.serenity_context().http,
        channel_id,
        &board,
        max_jobs,
        None,
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
    if !require_playing(ctx).await {
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
    let max_jobs = ctx.data().runtime.max_jobs;
    let channel_id = ctx.data().runtime.channel_id;
    let hospital = storage::load_hospital()?;
    let (board, status) = {
        let mut board = ctx.data().runtime.board.lock().await;
        let status = crate::game::guild_status::compute_guild_hall_status(&*board, &hospital)?;
        engine::take_and_enqueue_quest(
            &mut *board,
            &hospital,
            target_user_id,
            quest_id,
            &ctx.data().runtime.generation_queue,
            false,
            Some(&status),
            ctx.data().runtime.job_timeout_tick,
        )?;
        let status = crate::game::guild_status::compute_guild_hall_status(&*board, &hospital)?;
        (board.clone(), status)
    };
    guild_hall::update_board_message(
        http,
        channel_id,
        &board,
        max_jobs,
        Some(&status),
    )
    .await?;
    ctx.say("ok").await?;
    Ok(())
}
