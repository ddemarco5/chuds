use poise::serenity_prelude as serenity;

use crate::discord::channel::post_buffered_message;
use crate::discord::context::{admin_guard, Context, Error};
use crate::discord::tick;
use crate::game::engine;
use crate::game::persistence::storage;

#[poise::command(slash_command)]
pub async fn add_cm(ctx: Context<'_>, discord_user_id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let discord_user_id: u64 = match discord_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
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
    post_buffered_message(
        http,
        ctx.data().channel_id,
        ctx.data().max_buffer_messages,
        &content,
    )
    .await;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn delete_cm(ctx: Context<'_>, discord_user_id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let discord_user_id: u64 = match discord_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };
    let removed = storage::remove_chudmaster(discord_user_id)?;
    if !removed {
        ctx.say("that user is not a Chudmaster™").await?;
        return Ok(());
    }
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn tick(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    tick::execute_tick(
        &ctx.serenity_context().http,
        &ctx.data().board,
        &ctx.data().job_queue,
        ctx.data().channel_id,
        ctx.data().max_buffer_messages,
        ctx.data().max_jobs,
        ctx.data().bot_user_id,
        ctx.data().max_non_bot_messages,
    )
    .await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn add_chud(
    ctx: Context<'_>,
    target_user_id: String,
    name: String,
    description: String,
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
    engine::add_chud(target_user_id, name, description)?;
    ctx.say("ok").await?;
    Ok(())
}

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
