use crate::discord::board_ui;
use crate::discord::channel::post_buffered_message;
use crate::discord::context::{Context, Error};
use crate::game::domain::stash::STASH_CAPACITY;
use crate::game::engine;
use crate::game::mechanics::simulation::effective_stats;
use crate::game::persistence::storage;

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
    post_buffered_message(
        http,
        ctx.data().channel_id,
        ctx.data().max_buffer_messages,
        &content,
    )
    .await;
    let mut board = ctx.data().board.lock().await;
    board_ui::refresh_board_status(
        http,
        ctx.data().channel_id,
        &mut *board,
        ctx.data().max_jobs,
    )
    .await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn chudlerboard(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let content = crate::discord::board_ui::format_chudlerboard(&ctx.serenity_context().http).await;
    let msg = if content.is_empty() {
        "*No chuds yet.*".to_string()
    } else {
        content
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
        Some(p) => {
            let registry = ctx.data().item_registry.lock().await;
            let mut equipment_lines = Vec::new();
            if let Some(id) = p.chud.equipment.gear {
                if let Some(item) = registry.get(id) {
                    equipment_lines.push(format!(
                        "Gear: **{}** ({}) [{}]",
                        item.name,
                        item.subtype,
                        item.stats.format_triplet(),
                    ));
                }
            }
            if let Some(id) = p.chud.equipment.weapon {
                if let Some(item) = registry.get(id) {
                    equipment_lines.push(format!(
                        "Weapon: **{}** [{}]",
                        item.name,
                        item.stats.format_triplet(),
                    ));
                }
            }
            for (i, slot) in p.chud.equipment.misc.iter().enumerate() {
                if let Some(id) = slot {
                    if let Some(item) = registry.get(*id) {
                        equipment_lines.push(format!(
                            "Misc {}: **{}** ({}) [{}]",
                            i + 1,
                            item.name,
                            item.subtype,
                            item.stats.format_triplet(),
                        ));
                    }
                }
            }
            let equipment = if equipment_lines.is_empty() {
                String::new()
            } else {
                equipment_lines.join("\n")
            };
            let stash_line = if p.stash.is_empty() {
                format!("Stash: empty (0/{STASH_CAPACITY}) — use /gear to manage loadout")
            } else {
                format!(
                    "Stash: {}/{} items — use /gear to manage loadout",
                    p.stash.len(),
                    STASH_CAPACITY
                )
            };
            let effective = effective_stats(&p, &registry);
            let stats_line = p.format_effective_stats_line(effective);
            let msg = if equipment.is_empty() {
                format!(
                    "**{}**\n{}\n\n{}\n{}\n{}\n\nYou've got ${} worth of loose change.",
                    p.name,
                    p.description,
                    stash_line,
                    stats_line,
                    p.format_job_record(),
                    p.cash,
                )
            } else {
                format!(
                    "**{}**\n{}\n\n{}\n\n{}\n{}\n{}\n\nYou've got ${} worth of loose change.",
                    p.name,
                    p.description,
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
    let Some(player) = player else {
        ctx.say("You don't have a chud.").await?;
        return Ok(());
    };

    let registry = ctx.data().item_registry.lock().await;
    let message = crate::discord::gear_ui::build_gear_message(&player, &registry, None, None);
    drop(registry);

    if let Err(e) = crate::discord::gear_ui::edit_gear_message(&http, &token, &message).await {
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
        Some(p) => p,
    };
    let board = ctx.data().board.lock().await;
    match board.active_quest_for(user_id) {
        None => ctx
            .say(format!("**{}** is not on a job.", player.name))
            .await?,
        Some(q) => {
            let mut msg = format!(
                "**{}** - *{}*\n{}",
                q.generated.quest_title, q.generated.quest_giver, q.generated.description,
            );
            if q.quest_data.trials.len() > 1 {
                msg.push_str(&format!(
                    "\n\n{} is overcoming trials and tribulations.",
                    player.name
                ));
            }
            ctx.say(msg).await?
        }
    };
    Ok(())
}
