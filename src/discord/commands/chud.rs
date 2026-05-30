use crate::discord::channel::post_buffered_message;
use crate::discord::context::{Context, Error};
use crate::game::engine;
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
    ctx.say("ok").await?;
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
            let msg = format!(
                "**{}**\n{}\n{}\n\nYou've got ${} worth of loose change.",
                p.name,
                p.description,
                p.format_stats(),
                p.cash,
            );
            ctx.say(msg).await?
        }
    };
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
