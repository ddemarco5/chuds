use std::collections::HashSet;
use std::sync::RwLock;

use poise::serenity_prelude::{
    self as serenity, Cache, CreateAttachment, CreateMessage, GuildId, Http, UserId,
};

use crate::discord::formatting;
use crate::game::tick::QuestResolved;

pub const DM_CHAR_LIMIT: usize = 2000;

/// Users last seen on a mobile client this session (survives offline after cache drops them).
pub type LastMobileUsers = RwLock<HashSet<UserId>>;

/// DM a finished job report; logs warnings and never fails the tick.
pub async fn send_job_completion_dm(
    http: &Http,
    cache: &Cache,
    guild_id: GuildId,
    last_mobile: Option<&LastMobileUsers>,
    qr: &QuestResolved,
) {
    let content = formatting::build_dm_completion_content(
        &qr.player_name,
        &qr.result,
        &qr.player,
        &qr.level_up,
        qr.reward,
        qr.item_awarded.as_ref(),
        qr.item_auto_sold_gold,
        qr.hospitalized,
    );
    let dm_plain = content.plain();

    let user_id = UserId::new(qr.discord_user_id);
    let mobile_from_cache = cache.guild(guild_id).is_some_and(|guild| {
        guild
            .presences
            .get(&user_id)
            .and_then(|p| p.client_status.as_ref())
            .is_some_and(|cs| cs.mobile.is_some())
    });
    let mobile_sticky = last_mobile
        .and_then(|set| set.read().ok())
        .is_some_and(|set| set.contains(&user_id));
    let mobile = mobile_from_cache || mobile_sticky;
    let client = if mobile { "mobile" } else { "desktop" };

    let dm_map = serde_json::json!({ "recipient_id": qr.discord_user_id.to_string() });
    let dm_channel = match http.create_private_channel(&dm_map).await {
        Ok(dm) => dm.id,
        Err(e) => {
            tracing::warn!(
                discord_user_id = qr.discord_user_id,
                err = %e,
                "failed to open DM channel"
            );
            return;
        }
    };

    let delivery = if dm_plain.len() <= DM_CHAR_LIMIT {
        "components_v2"
    } else if mobile {
        "segmented"
    } else {
        "attachment"
    };
    tracing::info!(
        discord_user_id = qr.discord_user_id,
        player = %qr.player_name,
        client,
        delivery,
        plain_len = dm_plain.len(),
        mobile_from_cache,
        mobile_sticky,
        "sending job completion DM"
    );

    if dm_plain.len() <= DM_CHAR_LIMIT {
        let dm_report = formatting::build_dm_completion_components(&content);
        if let Err(e) = http.send_message(dm_channel, vec![], &dm_report).await {
            tracing::warn!(
                discord_user_id = qr.discord_user_id,
                err = %e,
                "failed to DM quest report"
            );
        }
        return;
    }

    if mobile {
        for part in formatting::build_dm_mobile_plain_parts(&content, DM_CHAR_LIMIT) {
            if let Err(e) = http
                .send_message(dm_channel, vec![], &CreateMessage::new().content(&part))
                .await
            {
                tracing::warn!(
                    discord_user_id = qr.discord_user_id,
                    err = %e,
                    "failed to DM quest report segment"
                );
                return;
            }
        }
        let outcome = formatting::build_dm_summary_components(&content);
        if let Err(e) = http.send_message(dm_channel, vec![], &outcome).await {
            tracing::warn!(
                discord_user_id = qr.discord_user_id,
                err = %e,
                "failed to DM quest report outcome"
            );
        }
    } else {
        let attachment = CreateAttachment::bytes(dm_plain.into_bytes(), "job_report.txt");
        let msg = CreateMessage::new().content(format!(
            "{}'s attempt at {} was too epic for discords character limit",
            qr.player_name, qr.quest_title
        ));
        if let Err(e) = http.send_message(dm_channel, vec![attachment], &msg).await {
            tracing::warn!(
                discord_user_id = qr.discord_user_id,
                err = %e,
                "failed to DM quest report"
            );
        }
    }
}

pub fn record_mobile_presence(last_mobile: &LastMobileUsers, presence: &serenity::Presence) {
    if presence
        .client_status
        .as_ref()
        .is_some_and(|cs| cs.mobile.is_some())
        && let Ok(mut set) = last_mobile.write()
    {
        set.insert(presence.user.id);
    }
}
