use crate::chud_msg;
use crate::discord::formatting::format_item_slot_label;
use crate::discord::guild_hall;
use crate::discord::game_screens;
use crate::discord::channel::append_activity_log;
use crate::discord::context::{Context, Error, GameRuntime};
use crate::game::busy::BusyReason;
use crate::game::domain::player::Player;
use crate::game::domain::session::GamePhase;
use crate::game::domain::stash::STASH_CAPACITY;
use crate::game::engine;
use crate::game::mechanics::simulation::effective_stats;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;

fn format_equipment_summary(player: &Player, registry: &ItemRegistry) -> String {
    let chud = player.chud_ref();
    let mut lines = Vec::new();
    if let Some(id) = chud.equipment.gear {
        if let Some(item) = registry.get(id) {
            lines.push(format!(
                "Gear: **{}** ({}) [{}]",
                item.name,
                format_item_slot_label(item),
                item.stats.format_triplet(),
            ));
        }
    }
    if let Some(id) = chud.equipment.weapon {
        if let Some(item) = registry.get(id) {
            lines.push(format!(
                "Weapon: **{}** ({}) [{}]",
                item.name,
                format_item_slot_label(item),
                item.stats.format_triplet(),
            ));
        }
    }
    for (i, slot) in chud.equipment.misc.iter().enumerate() {
        if let Some(id) = slot {
            if let Some(item) = registry.get(*id) {
                lines.push(format!(
                    "Misc {}: **{}** ({}) [{}]",
                    i + 1,
                    item.name,
                    format_item_slot_label(item),
                    item.stats.format_triplet(),
                ));
            }
        }
    }
    lines.join("\n")
}

fn format_inspect_description(player: &Player, registry: &ItemRegistry) -> String {
    let chud = player.chud_ref();
    let mut msg = chud_msg!("inspect_description", chud.name, chud.description);
    let equipment = format_equipment_summary(player, registry);
    if !equipment.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&equipment);
    }
    msg
}

/// Create a chud for `user_id` and update the channel for the current phase.
///
/// During Attract the attract roster is refreshed (no activity log); otherwise the normal
/// "saunters through the door" log line and board-status refresh are used. Callers must
/// reject the Complete phase before calling.
pub async fn handle_chud_join(
    runtime: &GameRuntime,
    user_id: u64,
    name: String,
    description: String,
) -> anyhow::Result<engine::AddChudResult> {
    let result = engine::add_chud(user_id, name, description)?;

    let phase = runtime.session.lock().await.phase;
    if phase == GamePhase::Attract {
        game_screens::update_attract_screen(runtime).await?;
    } else {
        let content = {
            let chud = result.player.chud_ref();
            chud_msg!("chud_joins", chud.name, chud.description)
        };
        append_activity_log(&runtime.activity_log, &content).await;
        let board = {
            let guard = runtime.board.lock().await;
            guard.clone()
        };
        guild_hall::refresh_board_status(
            &runtime.http,
            runtime.channel_id,
            &board,
            runtime.max_jobs,
        )
        .await?;
    }
    Ok(result)
}

#[poise::command(slash_command)]
pub async fn chud(ctx: Context<'_>, name: String, description: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();

    if ctx.data().runtime.session.lock().await.phase == GamePhase::Complete {
        ctx.say("the game is over, bud").await?;
        return Ok(());
    }
    if storage::load_player(user_id)?.is_some_and(|p| p.has_chud()) {
        ctx.say("you've already got a chud").await?;
        return Ok(());
    }

    let result = handle_chud_join(&ctx.data().runtime, user_id, name, description).await?;

    if result.benefits_claimed > 0 {
        let msg = chud_msg!("dm_starting_benefits", result.benefits_claimed);
        ctx.say(msg).await?;
    } else {
        ctx.say("ok").await?;
    }
    Ok(())
}

#[poise::command(slash_command)]
pub async fn chudlerboard(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let content = crate::discord::guild_hall::format_chudlerboard(&ctx.serenity_context().http).await;
    let msg = if content.is_empty() {
        "*No chuds yet.*".to_string()
    } else {
        content
    };
    ctx.say(msg).await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn inspect(ctx: Context<'_>, name: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let name = name.trim().to_string();
    let author_id = ctx.author().id.get();
    let msg = match storage::find_player_by_name(&name)? {
        None => chud_msg!("inspect_not_found", name),
        Some(player) if player.discord_user_id == author_id => {
            chud_msg!("inspect_self", player.chud_ref().name)
        }
        Some(player) => {
            let chud = player.chud_ref();
            let board = ctx.data().runtime.board.lock().await;
            let hospital = storage::load_hospital()?;
            let registry = ctx.data().runtime.item_registry.lock().await;
            match crate::game::busy::is_player_busy(&*board, &hospital, player.discord_user_id) {
                None => format_inspect_description(&player, &registry),
                Some(BusyReason::ActiveQuest { .. }) => chud_msg!("inspect_busy_job", chud.name),
                Some(BusyReason::Scouting) => chud_msg!("inspect_busy_scouting", chud.name),
                Some(BusyReason::Hospitalized) => {
                    format!(
                        "{}\n\n{}",
                        chud_msg!("inspect_hospital_prefix"),
                        format_inspect_description(&player, &registry),
                    )
                }
            }
        }
    };
    ctx.say(msg).await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    let player = storage::load_player(user_id)?;
    match player {
        None => ctx.say("You don't have a chud.").await?,
        Some(p) if !p.has_chud() => ctx.say("You don't have a chud.").await?,
        Some(p) => {
            let chud = p.chud_ref();
            let registry = ctx.data().runtime.item_registry.lock().await;
            let equipment = format_equipment_summary(&p, &registry);
            let stash_line = if p.stash.is_empty() {
                format!("Stash: empty (0/{STASH_CAPACITY}) - use `/gear` to manage loadout")
            } else {
                format!(
                    "Stash: {}/{} items - use `/gear` to manage loadout",
                    p.stash.len(),
                    STASH_CAPACITY
                )
            };
            let effective = effective_stats(&p, &registry);
            let stats_line = p.format_effective_stats_line(effective);
            let msg = if equipment.is_empty() {
                format!(
                    "**{}**\n{}\n\n{}\n{}\n{}\n\nYou've got ${} worth of loose change.",
                    chud.name,
                    chud.description,
                    stash_line,
                    stats_line,
                    p.format_job_record(),
                    p.cash,
                )
            } else {
                format!(
                    "**{}**\n{}\n\n{}\n\n{}\n{}\n{}\n\nYou've got ${} worth of loose change.",
                    chud.name,
                    chud.description,
                    equipment,
                    stash_line,
                    stats_line,
                    p.format_job_record(),
                    p.cash,
                )
            };
            ctx.say(msg).await?
        }
    };
    Ok(())
}

#[poise::command(slash_command)]
pub async fn gear(ctx: Context<'_>) -> Result<(), Error> {
    let token = match &ctx {
        poise::Context::Application(app) => app.interaction.token.clone(),
        _ => return Ok(()),
    };
    let http = ctx.serenity_context().http.clone();

    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    let player = storage::load_player(user_id)?;
    let Some(player) = player.filter(|p| p.has_chud()) else {
        ctx.say("You don't have a chud.").await?;
        return Ok(());
    };

    let message = crate::discord::build_gear_open_message(ctx.data(), &player).await?;

    if let Err(e) = crate::discord::edit_ephemeral_message(&http, &token, &message).await {
        tracing::debug!(err = %e, user_id, "gear command edit failed (ephemeral may be dismissed)");
    }
    Ok(())
}

#[poise::command(slash_command)]
pub async fn cash(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    match storage::load_player(user_id)? {
        None => ctx.say("You don't have a chud.").await?,
        Some(p) if p.cash == 0 => {
            ctx.say("https://tenor.com/view/poor-no-money-gif-24226168")
                .await?
        }
        Some(p) => ctx.say(format!("You've got {} buckeroos", p.cash)).await?,
    };
    Ok(())
}

#[poise::command(slash_command)]
pub async fn job(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = ctx.author().id.get();
    let player = storage::load_player(user_id)?;
    let player = match player {
        None => {
            ctx.say("You don't have a chud.").await?;
            return Ok(());
        }
        Some(p) if !p.has_chud() => {
            ctx.say("You don't have a chud.").await?;
            return Ok(());
        }
        Some(p) => p,
    };
    let chud_name = player.chud_ref().name.clone();
    let board = ctx.data().runtime.board.lock().await;
    match board.active_quest_for(user_id) {
        None => ctx
            .say(format!("**{chud_name}** is not on a job."))
            .await?,
        Some(q) => {
            let mut msg = format!(
                "**{}** - *{}*\n{}",
                q.generated.quest_title, q.generated.quest_giver, q.generated.description,
            );
            if q.quest_data.trials.len() > 1 {
                msg.push_str(&format!(
                    "\n\n{chud_name} is overcoming trials and tribulations.",
                ));
            }
            ctx.say(msg).await?
        }
    };
    Ok(())
}
