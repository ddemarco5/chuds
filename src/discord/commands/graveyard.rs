use crate::chud_msg;
use crate::discord::guild_hall;
use crate::discord::channel::append_activity_log;
use crate::discord::context::{Context, Error};
use crate::discord::formatting;
use crate::discord::edit_ephemeral_message;
use crate::game::persistence::storage;

#[poise::command(slash_command)]
pub async fn graveyard(ctx: Context<'_>) -> Result<(), Error> {
    let token = match &ctx {
        poise::Context::Application(app) => app.interaction.token.clone(),
        _ => return Ok(()),
    };
    let http = ctx.serenity_context().http.clone();

    ctx.defer_ephemeral().await?;

    let player = match storage::load_player(ctx.author().id.get())? {
        Some(p) if p.has_chud() => p,
        _ => {
            ctx.say("you need a chud").await?;
            return Ok(());
        }
    };

    let graveyard = storage::load_graveyard()?;
    if graveyard.entries.is_empty() {
        ctx.say(chud_msg!("graveyard_empty")).await?;
        return Ok(());
    }

    let message = formatting::build_graveyard_components(&graveyard.entries);
    edit_ephemeral_message(&http, &token, &message).await?;

    // Only touch the channel while playing; attract/complete own the screen.
    if ctx.data().runtime.is_playing().await {
        let content = chud_msg!("graveyard_visit", player.chud_ref().name);
        append_activity_log(&ctx.data().runtime.activity_log, &content).await;
        let board = {
            let guard = ctx.data().runtime.board.lock().await;
            guard.clone()
        };
        guild_hall::refresh_board_status(
            &http,
            ctx.data().runtime.channel_id,
            &board,
            ctx.data().runtime.max_jobs,
        )
        .await?;
    }

    Ok(())
}
