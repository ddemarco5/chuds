use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateInteractionResponseFollowup, EditInteractionResponse,
};

use crate::chud_msg;
use crate::discord::guild_hall;
use crate::discord::channel::append_activity_log;
use crate::discord::context::Data;
use crate::game::busy::BusyReason;
use crate::game::domain::board::Board;
use crate::game::domain::player::Player;
use crate::game::engine;
use crate::game::guild_status::{self, GuildHallStatus};
use crate::game::persistence::storage;

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

async fn ephemeral_hospitalized_response(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    player: &Player,
    user_id: u64,
) -> anyhow::Result<()> {
    let hospital = storage::load_hospital()?;
    let days = hospital.get_ticks_remaining(user_id).unwrap_or(0);
    let days_remaining = if days == 1 {
        "1 day".to_string()
    } else {
        format!("{days} days")
    };

    if let Some(heal_price) = hospital.get_heal_price(user_id) {
        if player.cash >= heal_price {
            let msg = chud_msg!("busy_hospitalized", player.chud_ref().name, &days_remaining);
            ephemeral_with_heal_button(ctx, interaction, &msg, user_id, heal_price).await?;
        } else {
            let msg = chud_msg!("heal_insufficient_funds", player.chud_ref().name);
            ephemeral_followup(ctx, interaction, &msg).await?;
        }
    } else {
        let msg = chud_msg!("busy_hospitalized", player.chud_ref().name, &days_remaining);
        ephemeral_followup(ctx, interaction, &msg).await?;
    }
    Ok(())
}

async fn respond_to_busy(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    player: &Player,
    user_id: u64,
    reason: BusyReason,
) -> anyhow::Result<()> {
    match reason {
        BusyReason::ActiveQuest { quest_title } => {
            let msg = chud_msg!("busy_active_quest", player.chud_ref().name, quest_title);
            ephemeral_followup(ctx, interaction, &msg).await?;
        }
        BusyReason::Scouting => {
            let msg = chud_msg!("busy_scouting", player.chud_ref().name);
            ephemeral_followup(ctx, interaction, &msg).await?;
        }
        BusyReason::Hospitalized => {
            ephemeral_hospitalized_response(ctx, interaction, player, user_id).await?;
        }
    }
    Ok(())
}

async fn announce_taken_quest(
    http: &serenity::Http,
    board: &Board,
    data: &Data,
    info: &engine::AssignInfo,
    status: &GuildHallStatus,
) -> anyhow::Result<()> {
    let chud_name = info.player.chud_ref().name.clone();
    let first_name = chud_name
        .split_whitespace()
        .next()
        .unwrap_or(&chud_name)
        .to_string();
    let content = chud_msg!("chud_takes_job", first_name, info.quest_title);

    append_activity_log(&data.runtime.activity_log, &content).await;
    guild_hall::update_board_message(
        http,
        data.runtime.channel_id,
        board,
        data.runtime.max_jobs,
        Some(status),
    )
    .await?;
    Ok(())
}

pub async fn post_quest_taken_announcement(
    http: &serenity::Http,
    board: &Board,
    data: &Data,
    info: &engine::AssignInfo,
    status: &GuildHallStatus,
) -> anyhow::Result<()> {
    announce_taken_quest(http, board, data, info, status).await
}

pub async fn handle_take_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    if !data.runtime.is_playing().await {
        ephemeral_followup(ctx, interaction, "the game isn't running right now").await?;
        return Ok(());
    }

    let quest_id: u32 = match interaction
        .data
        .custom_id
        .strip_prefix("take:")
        .and_then(|s| s.parse().ok())
    {
        Some(id) => id,
        None => return Ok(()),
    };
    let user_id = interaction.user.id.get();

    let player = match storage::load_player(user_id)? {
        Some(p) if p.has_chud() => p,
        _ => {
            ephemeral_followup(ctx, interaction, "You don't have a chud.").await?;
            return Ok(());
        }
    };

    let hospital = storage::load_hospital()?;

    enum TakeOutcome {
        Busy(BusyReason),
        Unavailable,
        Taken {
            board: Board,
            status: GuildHallStatus,
            info: engine::AssignInfo,
        },
    }

    let take_outcome = {
        let mut board = data.runtime.board.lock().await;
        let status = guild_status::compute_guild_hall_status(&*board, &hospital)?;
        if let Some(reason) = status.busy_reason(user_id) {
            TakeOutcome::Busy(reason)
        } else {
            match engine::take_and_enqueue_quest(
                &mut *board,
                &hospital,
                user_id,
                quest_id,
                &data.runtime.generation_queue,
                false,
                Some(&status),
                data.runtime.job_timeout_tick,
            ) {
                Ok(info) => {
                    let status = guild_status::compute_guild_hall_status(&*board, &hospital)?;
                    TakeOutcome::Taken {
                        board: board.clone(),
                        status,
                        info,
                    }
                }
                Err(_) => TakeOutcome::Unavailable,
            }
        }
    };

    match take_outcome {
        TakeOutcome::Busy(reason) => {
            respond_to_busy(ctx, interaction, &player, user_id, reason).await?;
        }
        TakeOutcome::Unavailable => {
            ephemeral_followup(ctx, interaction, "That job is no longer available.").await?;
        }
        TakeOutcome::Taken { board, status, info } => {
            post_quest_taken_announcement(&ctx.http, &board, data, &info, &status).await?;
        }
    }
    Ok(())
}

pub async fn handle_scout_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    if !data.runtime.is_playing().await {
        ephemeral_followup(ctx, interaction, "the game isn't running right now").await?;
        return Ok(());
    }

    let quest_id: u32 = match interaction
        .data
        .custom_id
        .strip_prefix("scout:")
        .and_then(|s| s.parse().ok())
    {
        Some(id) => id,
        None => return Ok(()),
    };
    let user_id = interaction.user.id.get();

    let player = match storage::load_player(user_id)? {
        Some(p) if p.has_chud() => p,
        _ => {
            ephemeral_followup(ctx, interaction, "You don't have a chud.").await?;
            return Ok(());
        }
    };

    let hospital = storage::load_hospital()?;

    enum ScoutOutcome {
        Busy(BusyReason),
        Unavailable,
        Scouted { board: Board, status: GuildHallStatus },
    }

    let scout_outcome = {
        let mut board = data.runtime.board.lock().await;
        let status = guild_status::compute_guild_hall_status(&*board, &hospital)?;
        if let Some(reason) = status.busy_reason(user_id) {
            ScoutOutcome::Busy(reason)
        } else if !board.scout(quest_id, user_id, data.runtime.job_timeout_tick) {
            ScoutOutcome::Unavailable
        } else {
            storage::save_board(&*board)?;
            let status = guild_status::compute_guild_hall_status(&*board, &hospital)?;
            ScoutOutcome::Scouted {
                board: board.clone(),
                status,
            }
        }
    };

    match scout_outcome {
        ScoutOutcome::Busy(reason) => {
            respond_to_busy(ctx, interaction, &player, user_id, reason).await?;
        }
        ScoutOutcome::Unavailable => {
            ephemeral_followup(ctx, interaction, "That job is no longer available.").await?;
        }
        ScoutOutcome::Scouted { board, status } => {
            let chud_name = player.chud_ref().name.clone();
            let first_name = chud_name
                .split_whitespace()
                .next()
                .unwrap_or(&chud_name)
                .to_string();
            let content = chud_msg!("chud_scouts_out", first_name);
            append_activity_log(&data.runtime.activity_log, &content).await;
            guild_hall::update_board_message(
                &ctx.http,
                data.runtime.channel_id,
                &board,
                data.runtime.max_jobs,
                Some(&status),
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn handle_heal_button(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    data: &Data,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    if !data.runtime.is_playing().await {
        ephemeral_followup(ctx, interaction, "the game isn't running right now").await?;
        return Ok(());
    }

    let user_id: u64 = interaction
        .data
        .custom_id
        .strip_prefix("heal:")
        .and_then(|s| s.parse().ok())
        .expect("valid heal button ID");

    assert_eq!(
        interaction.user.id.get(),
        user_id,
        "heal button clicked by non-owner"
    );

    let mut player = storage::load_player(user_id)?
        .expect("player with heal button exists");

    let mut hospital = storage::load_hospital()?;
    let heal_price = hospital
        .get_heal_price(user_id)
        .expect("chud with heal button is hospitalized");

    let mut episode_stats = storage::load_episode_stats()?;
    if !player.try_spend_cash(heal_price, Some(&mut episode_stats)) {
        anyhow::bail!("player cannot afford heal they were offered");
    }
    storage::save_player(&player)?;
    storage::save_episode_stats(&episode_stats)?;

    let chud_name = hospital
        .get_chud_name(user_id)
        .expect("hospitalized chud has name")
        .to_string();
    let release_msg = hospital
        .release(user_id)
        .expect("hospitalized chud exists");
    storage::save_hospital(&hospital)?;
    append_activity_log(&data.runtime.activity_log, &release_msg).await;

    let msg = chud_msg!("heal_success", chud_name, heal_price);
    interaction
        .edit_response(
            &ctx.http,
            EditInteractionResponse::new().content(msg).components(vec![]),
        )
        .await?;

    let board = {
        let guard = data.runtime.board.lock().await;
        guard.clone()
    };
    guild_hall::refresh_board_status(
        &ctx.http,
        data.runtime.channel_id,
        &board,
        data.runtime.max_jobs,
    )
    .await?;

    Ok(())
}
