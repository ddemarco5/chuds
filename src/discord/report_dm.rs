use poise::serenity_prelude::{CreateMessage, Http};

use crate::discord::formatting;
use crate::game::engine::KillResult;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::tick::QuestResolved;

pub const DM_CHAR_LIMIT: usize = 2000;

pub(crate) fn pack_dm_segments(segments: &[String], limit: usize) -> Vec<String> {
    let mut messages: Vec<String> = Vec::new();
    let mut current = String::new();

    for segment in segments.iter() {
        if segment.is_empty() {
            continue;
        }
        if current.is_empty() {
            if segment.len() <= limit {
                current = segment.clone();
            } else {
                messages.push(segment.clone());
            }
            continue;
        }

        let combined = format!("{current}{segment}");
        if combined.len() <= limit {
            current = combined;
        } else {
            messages.push(current);
            if segment.len() <= limit {
                current = segment.clone();
            } else {
                messages.push(segment.clone());
                current = String::new();
            }
        }
    }

    if !current.is_empty() {
        messages.push(current);
    }

    messages
}

async fn open_dm_channel(http: &Http, discord_user_id: u64) -> Option<poise::serenity_prelude::ChannelId> {
    let dm_map = serde_json::json!({ "recipient_id": discord_user_id.to_string() });
    match http.create_private_channel(&dm_map).await {
        Ok(dm) => Some(dm.id),
        Err(e) => {
            tracing::warn!(
                discord_user_id,
                err = %e,
                "failed to open DM channel"
            );
            None
        }
    }
}

/// DM a hospital release notice; logs warnings and never fails the caller.
pub async fn send_hospital_release_dm(http: &Http, discord_user_id: u64, message: &str) {
    let Some(dm_channel) = open_dm_channel(http, discord_user_id).await else {
        return;
    };

    let dm = formatting::build_dm_notice_components(message, true);
    if let Err(e) = http.send_message(dm_channel, vec![], &dm).await {
        tracing::warn!(
            discord_user_id,
            err = %e,
            "failed to DM hospital release"
        );
    }
}

/// DM a finished job report; logs warnings and never fails the tick.
pub async fn send_job_completion_dm(
    http: &Http,
    registry: &ItemRegistry,
    qr: &QuestResolved,
) {
    let content = formatting::build_dm_completion_content(
        &qr.player_name,
        &qr.result,
        &qr.player,
        registry,
        &qr.level_up,
        qr.reward,
        qr.item_awarded.as_ref(),
        qr.item_award_disposition,
        qr.hospitalized,
    );
    let plain_len = content.plain().len();

    let Some(dm_channel) = open_dm_channel(http, qr.discord_user_id).await else {
        return;
    };

    tracing::info!(
        discord_user_id = qr.discord_user_id,
        player = %qr.player_name,
        segmented = plain_len > DM_CHAR_LIMIT,
        plain_len,
        "sending job completion DM"
    );

    if plain_len <= DM_CHAR_LIMIT {
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

    for part in pack_dm_segments(content.segments(), DM_CHAR_LIMIT) {
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
}

/// DM a death notice; logs warnings and never fails the caller.
pub async fn send_death_dm(http: &Http, kill: &KillResult, summary: Option<&str>) {
    let content = formatting::build_dm_death_content(kill, summary);
    let plain_len = content.plain().len();

    let Some(dm_channel) = open_dm_channel(http, kill.discord_user_id).await else {
        return;
    };

    if plain_len <= DM_CHAR_LIMIT {
        let dm_report = formatting::build_dm_death_components(&content);
        if let Err(e) = http.send_message(dm_channel, vec![], &dm_report).await {
            tracing::warn!(
                discord_user_id = kill.discord_user_id,
                err = %e,
                "failed to DM death notice"
            );
        }
        return;
    }

    for part in pack_dm_segments(&[content.body.clone()], DM_CHAR_LIMIT) {
        if let Err(e) = http
            .send_message(dm_channel, vec![], &CreateMessage::new().content(&part))
            .await
        {
            tracing::warn!(
                discord_user_id = kill.discord_user_id,
                err = %e,
                "failed to DM death notice segment"
            );
            return;
        }
    }
}
