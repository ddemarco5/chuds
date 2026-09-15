use poise::serenity_prelude as serenity;

use crate::discord::components_v2::{Component, ComponentsV2Message, Separator, TextDisplay};
use crate::discord::context::{Context, Error};
use crate::discord::edit_ephemeral_message;
use crate::discord::formatting::subtext_lines;
use crate::discord::send_ephemeral_followup;
use crate::game::persistence::storage;

async fn guild_member_nickname(http: &serenity::Http, guild_id: u64, user_id: u64) -> String {
    match http
        .get_member(
            serenity::GuildId::new(guild_id),
            serenity::UserId::new(user_id),
        )
        .await
    {
        Ok(member) => member
            .nick
            .or(member.user.global_name)
            .unwrap_or(member.user.name),
        Err(_) => match http.get_user(serenity::UserId::new(user_id)).await {
            Ok(user) => user.global_name.unwrap_or(user.name),
            Err(_) => user_id.to_string(),
        },
    }
}

fn format_chud_entry(chud_name: &str, nickname: &str, description: &str) -> String {
    format!(
        "**{chud_name}**, {nickname}'s chud\n{}",
        subtext_lines(description),
    )
}

async fn build_all_chuds_messages(http: &serenity::Http, guild_id: u64) -> Vec<ComponentsV2Message> {
    const MAX_COMPONENTS: usize = 40;

    let mut players = storage::load_chuds();
    if players.is_empty() {
        return Vec::new();
    }

    players.sort_by(|a, b| {
        a.chud_ref()
            .name
            .to_ascii_lowercase()
            .cmp(&b.chud_ref().name.to_ascii_lowercase())
    });

    let mut messages = Vec::new();
    let mut components: Vec<Component> = Vec::new();

    for (i, player) in players.iter().enumerate() {
        let chud = player.chud_ref();
        let nickname = guild_member_nickname(http, guild_id, player.discord_user_id).await;
        let entry = format_chud_entry(&chud.name, &nickname, &chud.description);

        if i > 0 {
            if components.len() + 2 > MAX_COMPONENTS {
                messages.push(ComponentsV2Message::ephemeral(std::mem::take(&mut components)));
            }
            if !components.is_empty() {
                components.push(Component::Separator(Separator::section()));
            }
        }

        if components.len() + 1 > MAX_COMPONENTS {
            messages.push(ComponentsV2Message::ephemeral(std::mem::take(&mut components)));
        }

        components.push(Component::Text(TextDisplay::new(entry)));
    }

    if !components.is_empty() {
        messages.push(ComponentsV2Message::ephemeral(components));
    }

    messages
}

#[poise::command(slash_command)]
pub async fn all_chuds(ctx: Context<'_>) -> Result<(), Error> {
    let token = match &ctx {
        poise::Context::Application(app) => app.interaction.token.clone(),
        _ => return Ok(()),
    };
    let http = ctx.serenity_context().http.clone();

    ctx.defer_ephemeral().await?;
    let runtime = &ctx.data().runtime;
    let messages = build_all_chuds_messages(&runtime.http, runtime.guild_id).await;
    if messages.is_empty() {
        let msg = ComponentsV2Message::ephemeral(vec![Component::Text(TextDisplay::new(
            "*No chuds yet.*",
        ))]);
        edit_ephemeral_message(&http, &token, &msg).await?;
        return Ok(());
    }

    edit_ephemeral_message(&http, &token, &messages[0]).await?;
    for msg in messages.iter().skip(1) {
        send_ephemeral_followup(&http, &token, msg).await?;
    }
    Ok(())
}
