use poise::serenity_prelude::{self as serenity, Http, MessageId};

use crate::chud_msg;
use crate::discord::components_v2::{
    ActionRow, Button, Component, Container, ContainerChild, ComponentsV2Message, Separator,
    TextDisplay,
};
use crate::game::guild_status::{self, GuildHallStatus};

/// Accent colors for job-slot containers (RGB integers).
const ACCENT_INACTIVE: u32 = 0x4E5058; // empty slots and taken jobs
const ACCENT_JOB_OPEN: u32 = 0x57F287;
use crate::discord::channel::{cleanup_non_bot_messages, delete_all_messages_in_channel};
use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::Player;
use crate::game::engine;
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

fn status_section(header: &str, names: &[String]) -> String {
    format!("*{header}*\n-# {}", names.join(", "))
}

struct StatusSectionHeaders {
    idle: Option<String>,
    all_busy: Option<String>,
    hospital: Option<String>,
}

/// Picks or reuses cached `chud_msg!` headers. A new random line is chosen only when a section
/// becomes visible again after having no contents (header cache cleared while empty).
fn resolve_status_headers(
    status: &GuildHallStatus,
    cache: &mut crate::game::persistence::message_cache::MessageCache,
) -> (StatusSectionHeaders, bool) {
    let mut dirty = false;

    let idle = if !status.idle_names.is_empty() {
        if cache.status_idle_header.is_none() {
            cache.status_idle_header = Some(chud_msg!("idleing"));
            dirty = true;
        }
        cache.status_idle_header.clone()
    } else if cache.status_idle_header.is_some() {
        cache.status_idle_header = None;
        dirty = true;
        None
    } else {
        None
    };

    let all_busy = if status.show_all_busy {
        if cache.status_all_busy_header.is_none() {
            cache.status_all_busy_header = Some(chud_msg!("all_busy"));
            dirty = true;
        }
        cache.status_all_busy_header.clone()
    } else if cache.status_all_busy_header.is_some() {
        cache.status_all_busy_header = None;
        dirty = true;
        None
    } else {
        None
    };

    let hospital = if !status.hospital_names.is_empty() {
        if cache.status_hospital_header.is_none() {
            cache.status_hospital_header = Some(chud_msg!("hospital_waiting"));
            dirty = true;
        }
        cache.status_hospital_header.clone()
    } else if cache.status_hospital_header.is_some() {
        cache.status_hospital_header = None;
        dirty = true;
        None
    } else {
        None
    };

    (
        StatusSectionHeaders {
            idle,
            all_busy,
            hospital,
        },
        dirty,
    )
}

fn build_status_message(status: &GuildHallStatus, headers: &StatusSectionHeaders) -> ComponentsV2Message {
    let mut inner: Vec<ContainerChild> = Vec::new();

    if !status.idle_names.is_empty() {
        inner.push(ContainerChild::Text(TextDisplay::new(status_section(
            headers.idle.as_deref().expect("idle header when idle names present"),
            &status.idle_names,
        ))));
    } else if status.show_all_busy {
        inner.push(ContainerChild::Text(TextDisplay::new(format!(
            "*{}*",
            headers
                .all_busy
                .as_deref()
                .expect("all_busy header when show_all_busy")
        ))));
    } else {
        inner.push(ContainerChild::Text(TextDisplay::new("\u{200B}\n\u{200B}")));
    }

    if !status.hospital_names.is_empty() {
        if !inner.is_empty() {
            inner.push(ContainerChild::Separator(Separator::section()));
        }
        inner.push(ContainerChild::Text(TextDisplay::new(status_section(
            headers
                .hospital
                .as_deref()
                .expect("hospital header when hospital names present"),
            &status.hospital_names,
        ))));
    }

    ComponentsV2Message::channel_box(inner, None)
}

/// Recompute guild-hall status and refresh the persistent status message (and job slots).
pub async fn refresh_board_status(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    max_jobs: usize,
) -> anyhow::Result<()> {
    let hospital = storage::load_hospital()?;
    let status = guild_status::compute_guild_hall_status(board, &hospital)?;
    update_board_message(http, channel_id, board, max_jobs, Some(&status)).await
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

/// Purge the channel, reset the message cache, and re-post all persistent board UI.
pub async fn recover_persistent_board_messages(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    job_queue: &mut JobQueue,
    bot_user_id: u64,
    max_non_bot_messages: usize,
    max_jobs: usize,
) -> anyhow::Result<()> {
    {
        let _cache_guard = storage::message_cache_lock().await;
        tracing::info!("channel redraw triggered");
        delete_all_messages_in_channel(http, channel_id).await;
        if let Err(e) = storage::save_message_cache(&Default::default()) {
            tracing::warn!(err = %e, "failed to clear message cache");
        }
    }

    cleanup_non_bot_messages(http, channel_id, bot_user_id, max_non_bot_messages).await;

    engine::refill_board_from_queue(board, job_queue, max_jobs);
    storage::save_board(board)?;
    storage::save_job_queue(job_queue)?;
    update_board_message(http, channel_id, board, max_jobs, None).await
}

pub async fn update_board_message(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    max_jobs: usize,
    status: Option<&GuildHallStatus>,
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

    let resolved_status = match status {
        Some(s) => s.clone(),
        None => {
            let hospital = storage::load_hospital()?;
            guild_status::compute_guild_hall_status(board, &hospital)?
        }
    };
    let (status_headers, status_headers_dirty) = resolve_status_headers(&resolved_status, &mut cache);
    if status_headers_dirty {
        cache_dirty = true;
    }
    let status_message = build_status_message(&resolved_status, &status_headers);
    let status_key = status_message.cache_key();
    if cache.status_message_id.is_none() {
        match send_cv2(http, ch, &status_message).await {
            Ok(msg) => {
                tracing::info!(msg_id = msg.id.get(), "guild hall status message posted");
                cache.status_message_id = Some(msg.id.get());
                cache.status_content = status_key;
                cache_dirty = true;
            }
            Err(e) => tracing::warn!(err = %e, "failed to post guild hall status message"),
        }
    } else if status_key != cache.status_content {
        let msg_id = cache.status_message_id.unwrap();
        if edit_cv2(http, ch, MessageId::new(msg_id), &status_message).await {
            cache.status_content = status_key;
            cache_dirty = true;
            tracing::debug!(msg_id, "guild hall status message edited");
        } else {
            tracing::warn!(msg_id, "failed to edit guild hall status message");
        }
    }

    if cache_dirty {
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache");
        }
    }
    Ok(())
}