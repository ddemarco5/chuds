use poise::serenity_prelude::{self as serenity, Http, MessageId};

use crate::discord::components_v2::{
    ActionRow, Button, Component, Container, ContainerChild, ComponentsV2Message, TextDisplay,
};

/// Accent colors for job-slot containers (RGB integers).
const ACCENT_INACTIVE: u32 = 0x4E5058; // empty slots and taken jobs
const ACCENT_JOB_OPEN: u32 = 0x57F287;
use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::player::Player;
use crate::game::persistence::message_cache::JobSlot;
use crate::game::persistence::storage;

fn subtext_lines(text: &str) -> String {
    text.lines()
        .map(|line| format!("-# {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_job_slot(quest: Option<&BoardQuest>) -> String {
    match quest {
        Some(q) => {
            let title = &q.generated.quest_title;
            let giver = &q.generated.quest_giver;
            let desc = &q.generated.description;
            let reward = q.generated.reward;
            if q.has_active() {
                format!(
                    "-# **{title}** - *{giver}* | ${reward}\n{}",
                    subtext_lines(desc)
                )
            } else {
                format!("**{title}** - *{giver}* | ${reward}\n{desc}")
            }
        }
        None => "*Nothing posted here*".to_string(),
    }
}

fn job_slot_accent(q: &BoardQuest) -> u32 {
    if q.has_active() {
        ACCENT_INACTIVE
    } else {
        ACCENT_JOB_OPEN
    }
}

fn build_job_board_header(filled: usize, max_jobs: usize) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(format!(
        "\u{200B}\n\u{200B}\t\u{200B}\t**CHUD GUILD JOB BOARD** *{filled}/{max_jobs} posted*\n\u{200B}"
    )))])
}

fn build_job_slot(quest: Option<&BoardQuest>) -> ComponentsV2Message {
    let (accent, inner) = match quest {
        None => (
            ACCENT_INACTIVE,
            vec![ContainerChild::Text(TextDisplay::new(format_job_slot(None)))],
        ),
        Some(q) => {
            let mut inner =
                vec![ContainerChild::Text(TextDisplay::new(format_job_slot(Some(q))))];
            let mut buttons = Vec::new();
            if !q.has_active() {
                buttons.push(Button::primary(format!("take:{}", q.id), "Take"));
            }
            buttons.push(Button::secondary(format!("scout:{}", q.id), "Scout"));
            inner.push(ContainerChild::ActionRow(ActionRow::buttons(buttons)));
            (job_slot_accent(q), inner)
        }
    };
    ComponentsV2Message::channel(vec![Component::Container(Container::with_accent(
        accent, inner,
    ))])
}

fn build_divider() -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![Component::Text(TextDisplay::new(
        "\u{200B}\n\u{200B}",
    ))])
}

async fn send_cv2(
    http: &Http,
    channel: serenity::ChannelId,
    message: &ComponentsV2Message,
) -> Result<serenity::Message, serenity::Error> {
    http.send_message(channel, vec![], message).await
}

async fn edit_cv2(
    http: &Http,
    channel: serenity::ChannelId,
    message_id: MessageId,
    message: &ComponentsV2Message,
) -> bool {
    http.edit_message(channel, message_id, message, vec![])
        .await
        .is_ok()
}

pub async fn format_chudlerboard(http: &serenity::Http) -> String {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for chudlerboard");
            return String::new();
        }
    };
    if ids.is_empty() {
        return String::new();
    }

    let players: Vec<Player> = ids
        .iter()
        .filter_map(|&id| match storage::load_player(id) {
            Ok(Some(p)) => Some(p),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(discord_user_id = id, err = %e, "failed to load player for chudlerboard");
                None
            }
        })
        .collect();

    if players.is_empty() {
        return String::new();
    }

    fn sole_leader(players: &[Player], accessor: fn(&Player) -> u8) -> Option<String> {
        let max_val = players.iter().map(|p| accessor(p)).max().unwrap_or(0);
        let mut leaders = players.iter().filter(|p| accessor(p) == max_val);
        let first = leaders.next()?;
        if leaders.next().is_some() {
            return None;
        }
        Some(first.name.clone())
    }

    let mut lines = Vec::new();

    if let Some(richest) = players
        .iter()
        .max_by_key(|p| p.cash)
        .filter(|_| {
            let max_cash = players.iter().map(|p| p.cash).max().unwrap_or(0);
            players.iter().filter(|p| p.cash == max_cash).count() == 1
        })
    {
        let discord_name = match http
            .get_user(serenity::UserId::new(richest.discord_user_id))
            .await
        {
            Ok(user) => user.global_name.unwrap_or(user.name),
            Err(_) => richest.discord_user_id.to_string(),
        };
        lines.push(format!("{} is the richest chudlord", discord_name));
    }

    let stat_defs: [(&str, fn(&Player) -> u8); 4] = [
        ("strongest", |p| p.strength),
        ("smartest", |p| p.smarts),
        ("sneakiest", |p| p.stealth),
        ("expert", |p| p.experience),
    ];

    for (adjective, accessor) in &stat_defs {
        if let Some(name) = sole_leader(&players, *accessor) {
            lines.push(format!("{} is the {}", name, adjective));
        }
    }

    if lines.is_empty() {
        return String::new();
    }

    format!("```\n----- Chudlerboard -----\n{}\n```", lines.join("\n"))
}

pub async fn update_board_message(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    max_jobs: usize,
) -> anyhow::Result<()> {
    let _cache_guard = storage::message_cache_lock().await;
    let ch = serenity::ChannelId::new(channel_id);

    let mut cache = storage::load_message_cache().unwrap_or_default();
    let mut cache_dirty = false;

    let filled = board.quests.len();
    let header_message = build_job_board_header(filled, max_jobs);
    let header_key = header_message.cache_key();
    if cache.header_message_id.is_none() {
        match send_cv2(http, ch, &header_message).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "job board header message posted");
                cache.header_message_id = Some(msg.id.get());
                cache.job_board_header = header_key;
                cache_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post job board header message"),
        }
    } else if header_key != cache.job_board_header {
        let msg_id = cache.header_message_id.unwrap();
        if edit_cv2(http, ch, MessageId::new(msg_id), &header_message).await {
            cache.job_board_header = header_key;
            cache_dirty = true;
            tracing::debug!(msg_id, "job board header message edited");
        } else {
            tracing::warn!(msg_id, "failed to edit job board header message");
        }
    }

    while cache.slots.len() < max_jobs {
        cache.slots.push(JobSlot::default());
        cache_dirty = true;
    }

    let mut clear_reactions_slots = vec![false; cache.slots.len()];

    for (i, slot) in cache.slots.iter_mut().enumerate() {
        if let Some(jid) = slot.job_id {
            if !board.quests.iter().any(|q| q.id == jid) {
                slot.job_id = None;
                cache_dirty = true;
                clear_reactions_slots[i] = true;
            }
        }
    }

    for quest in &board.quests {
        let already_slotted = cache.slots.iter().any(|s| s.job_id == Some(quest.id));
        if !already_slotted {
            if let Some(i) = cache.slots.iter().position(|s| s.job_id.is_none()) {
                cache.slots[i].job_id = Some(quest.id);
                cache_dirty = true;
                clear_reactions_slots[i] = true;
            }
        }
    }

    for (i, slot) in cache.slots.iter_mut().enumerate() {
        let quest = slot.job_id.and_then(|jid| board.quests.iter().find(|q| q.id == jid));
        let slot_message = build_job_slot(quest);
        let expected_key = slot_message.cache_key();
        if expected_key == slot.content && slot.message_id.is_some() {
            continue;
        }
        if let Some(msg_id) = slot.message_id {
            if !edit_cv2(http, ch, MessageId::new(msg_id), &slot_message).await {
                tracing::warn!(msg_id, slot = i, "failed to edit job slot message, posting new one");
                match send_cv2(http, ch, &slot_message).await {
                    Ok(msg) => {
                        slot.message_id = Some(msg.id.get());
                        slot.content = expected_key;
                        cache_dirty = true;
                        if clear_reactions_slots[i] {
                            if let Err(e) = http.delete_message_reactions(ch, msg.id).await {
                                tracing::warn!(msg_id = msg.id.get(), slot = i, err = %e, "failed to clear reactions on job slot");
                            }
                        }
                    }
                    Err(e) => tracing::warn!(err = %e, slot = i, "failed to post replacement job slot message"),
                }
            } else {
                slot.content = expected_key;
                cache_dirty = true;
                tracing::debug!(msg_id, slot = i, "job slot message edited");
                if clear_reactions_slots[i] {
                    if let Err(e) = http.delete_message_reactions(ch, MessageId::new(msg_id)).await {
                        tracing::warn!(msg_id, slot = i, err = %e, "failed to clear reactions on job slot");
                    }
                }
            }
        } else {
            match send_cv2(http, ch, &slot_message).await {
                Ok(msg) => {
                    tracing::info!(msg_id = msg.id.get(), slot = i, "job slot message posted");
                    slot.message_id = Some(msg.id.get());
                    slot.content = expected_key;
                    cache_dirty = true;
                    if clear_reactions_slots[i] {
                        if let Err(e) = http.delete_message_reactions(ch, msg.id).await {
                            tracing::warn!(msg_id = msg.id.get(), slot = i, err = %e, "failed to clear reactions on job slot");
                        }
                    }
                }
                Err(e) => tracing::warn!(err = %e, slot = i, "failed to post job slot message"),
            }
        }
    }

    if cache.divider_message_id.is_none() {
        let divider = build_divider();
        match send_cv2(http, ch, &divider).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "divider message posted");
                cache.divider_message_id = Some(msg.id.get());
                cache_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post divider message"),
        }
    }

    if cache_dirty {
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache");
        }
    }
    Ok(())
}