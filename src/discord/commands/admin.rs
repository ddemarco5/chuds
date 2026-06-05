use poise::serenity_prelude as serenity;

use crate::discord::board_ui::recover_persistent_board_messages;
use crate::discord::buttons::post_quest_taken_announcement;
use crate::discord::channel::{append_activity_log, append_activity_log_deferred};
use crate::discord::context::{admin_guard, Context, Error};
use crate::discord::report_dm;
use crate::discord::tick;
use crate::game::engine::{self, DeathContext};
use crate::game::guild_status;
use crate::game::persistence::message_cache::ActivityLogKind;
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
    append_activity_log(&ctx.data().activity_log, &content).await;
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
pub async fn admin_redraw(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let http = &ctx.serenity_context().http;
    let mut board = ctx.data().board.lock().await;
    let mut queue = ctx.data().job_queue.lock().await;
    recover_persistent_board_messages(
        http,
        ctx.data().channel_id,
        &mut *board,
        &mut *queue,
        ctx.data().bot_user_id,
        ctx.data().max_non_bot_messages,
        ctx.data().max_jobs,
    )
    .await?;
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
        &ctx.serenity_context().cache,
        ctx.data().guild_id,
        Some(ctx.data().last_mobile.as_ref()),
        &ctx.data().board,
        &ctx.data().job_queue,
        &ctx.data().item_registry,
        ctx.data().channel_id,
        &ctx.data().activity_log,
        ctx.data().max_jobs,
        ctx.data().bot_user_id,
        ctx.data().max_non_bot_messages,
    )
    .await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn admin_take_gen_item(
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
    let hospital = storage::load_hospital()?;
    let mut board = ctx.data().board.lock().await;
    let status = crate::game::guild_status::compute_guild_hall_status(&*board, &hospital)?;
    let info = engine::take_and_enqueue_quest(
        &mut *board,
        &hospital,
        target_user_id,
        quest_id,
        &ctx.data().generation_queue,
        true,
        Some(&status),
    )?;
    let status = crate::game::guild_status::compute_guild_hall_status(&*board, &hospital)?;
    post_quest_taken_announcement(http, &mut *board, ctx.data(), &info, &status).await?;
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
    let registry = ctx.data().item_registry.lock().await;
    engine::save_all(&*board, &*registry)?;
    crate::game::persistence::llm_memory::save_all_llm_memory(
        &ctx.data().generator,
        &ctx.data().item_generator,
        &ctx.data().gravestone_generator,
    )?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn admin_kill_chud(
    ctx: Context<'_>,
    target_user_id: String,
    #[description = "Trial situation that killed them"] death_trial: Option<String>,
    #[description = "Outcome that killed them"] death_outcome: Option<String>,
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

    let hospital = storage::load_hospital()?;
    let board = ctx.data().board.lock().await;
    let status = guild_status::compute_guild_hall_status(&*board, &hospital)?;
    if status.is_busy(target_user_id) {
        ctx.say("chud is busy").await?;
        return Ok(());
    }
    drop(board);

    let death_ctx = DeathContext {
        trial: death_trial.unwrap_or_else(|| "Unknown peril".into()),
        outcome: death_outcome.unwrap_or_else(|| "They did not make it".into()),
    };

    let mut graveyard = storage::load_graveyard()?;
    let mut starting_benefits = storage::load_starting_benefits()?;
    let mut registry = ctx.data().item_registry.lock().await;

    let kill = engine::kill_chud(
        target_user_id,
        &death_ctx,
        &mut graveyard,
        &mut starting_benefits,
        &mut *registry,
        &ctx.data().gravestone_generator,
    )
    .await?;

    let first_name = kill
        .chud_name
        .split_whitespace()
        .next()
        .unwrap_or(&kill.chud_name)
        .to_string();
    let return_msg = crate::chud_msg!("return_died", first_name);
    append_activity_log_deferred(
        &ctx.data().activity_log,
        ActivityLogKind::Standard,
        &return_msg,
    )
    .await;

    report_dm::send_death_dm(
        &ctx.serenity_context().http,
        &ctx.serenity_context().cache,
        poise::serenity_prelude::GuildId::new(ctx.data().guild_id),
        Some(ctx.data().last_mobile.as_ref()),
        &kill,
        None,
    )
    .await;

    let mut board = ctx.data().board.lock().await;
    crate::discord::board_ui::refresh_board_status(
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
pub async fn load(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let (new_board, new_registry) = engine::load_all()?;
    let mut board = ctx.data().board.lock().await;
    *board = new_board;
    let mut registry = ctx.data().item_registry.lock().await;
    *registry = new_registry;
    tracing::info!("board and item registry reloaded from disk");
    ctx.say("ok").await?;
    Ok(())
}
