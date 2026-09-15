use std::sync::LazyLock;

use poise::serenity_prelude::{self as serenity, Http, MessageId};
use tokio::sync::Mutex;

use crate::chud_msg;
use crate::discord::components_v2::{
    ActionRow, Button, Component, Container, ContainerChild, ComponentsV2Message, Separator,
    TextDisplay,
};
use crate::discord::formatting::subtext_lines;
use crate::game::guild_status::{self, GuildHallStatus};

/// Accent colors for job-slot containers (RGB integers).
const ACCENT_INACTIVE: u32 = 0x4E5058; // empty slots and taken regular jobs
const ACCENT_JOB_OPEN: u32 = 0x57F287;
pub(crate) const ACCENT_STORY_JOB: u32 = 0xE67E22;
const ACCENT_STORY_JOB_INACTIVE: u32 = 0x6B5A4E;
use crate::discord::channel::{cleanup_non_bot_messages, delete_all_messages_in_channel};
use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::Player;
use crate::game::engine;
use crate::discord::ui::update_merchant_message;
use crate::game::merchant::MerchantState;
use crate::game::persistence::message_cache::JobSlot;
use crate::game::persistence::storage;

/// Serialize Discord board renders so overlapping tick/worker/button updates cannot
/// both post new slot messages or clobber cache state.
static BOARD_RENDER_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
    if q.is_story() {
        if q.has_active() {
            ACCENT_STORY_JOB_INACTIVE
        } else {
            ACCENT_STORY_JOB
        }
    } else if q.has_active() {
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

/// First open slot, preferring placeholders that already have a Discord message.
fn first_empty_slot(slots: &[JobSlot]) -> Option<usize> {
    slots
        .iter()
        .position(|s| s.job_id.is_none() && s.message_id.is_some())
        .or_else(|| slots.iter().position(|s| s.job_id.is_none()))
}

fn status_section(header: &str, names: &[String]) -> String {
    format!("*{header}*\n-# {}", names.join(", "))
}

struct StatusSectionHeaders {
    idle: Option<String>,
    all_busy: Option<String>,
    no_chuds: Option<String>,
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

    let no_chuds = if status.show_no_chuds {
        if cache.status_no_chuds_header.is_none() {
            cache.status_no_chuds_header = Some(chud_msg!("no_chuds"));
            dirty = true;
        }
        cache.status_no_chuds_header.clone()
    } else if cache.status_no_chuds_header.is_some() {
        cache.status_no_chuds_header = None;
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
            no_chuds,
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
    } else if status.show_no_chuds {
        inner.push(ContainerChild::Text(TextDisplay::new(format!(
            "*{}*",
            headers
                .no_chuds
                .as_deref()
                .expect("no_chuds header when show_no_chuds")
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
    board: &Board,
    max_jobs: usize,
) -> anyhow::Result<()> {
    let hospital = storage::load_hospital()?;
    let status = guild_status::compute_guild_hall_status(board, &hospital)?;
    update_board_message(http, channel_id, board, max_jobs, Some(&status)).await
}

pub(crate) async fn send_cv2(
    http: &Http,
    channel: serenity::ChannelId,
    message: &ComponentsV2Message,
) -> Result<serenity::Message, serenity::Error> {
    http.send_message(channel, vec![], message).await
}

pub(crate) async fn edit_cv2(
    http: &Http,
    channel: serenity::ChannelId,
    message_id: MessageId,
    message: &ComponentsV2Message,
) -> Result<(), serenity::Error> {
    http.edit_message(channel, message_id, message, vec![])
        .await
        .map(|_| ())
}

async fn sync_job_slot(
    http: &Http,
    ch: serenity::ChannelId,
    slot: &mut JobSlot,
    slot_index: usize,
    slot_message: &ComponentsV2Message,
    expected_key: &str,
) -> bool {
    if expected_key == slot.content && slot.message_id.is_some() {
        return false;
    }

    if let Some(msg_id) = slot.message_id {
        match edit_cv2(http, ch, MessageId::new(msg_id), slot_message).await {
            Ok(()) => {
                slot.content = expected_key.to_string();
                tracing::debug!(msg_id, slot = slot_index, "job slot message edited");
                return true;
            }
            Err(e) => {
                tracing::warn!(
                    msg_id,
                    slot = slot_index,
                    err = %e,
                    "failed to edit job slot message, will post replacement"
                );
                if let Err(del_err) = http.delete_message(ch, MessageId::new(msg_id), None).await {
                    tracing::debug!(msg_id, err = %del_err, "could not delete stale job slot message");
                }
                slot.message_id = None;
            }
        }
    }

    match send_cv2(http, ch, slot_message).await {
        Ok(msg) => {
            tracing::info!(msg_id = msg.id.get(), slot = slot_index, "job slot message posted");
            slot.message_id = Some(msg.id.get());
            slot.content = expected_key.to_string();
            true
        }
        Err(e) => {
            tracing::warn!(err = %e, slot = slot_index, "failed to post job slot message");
            false
        }
    }
}

pub async fn format_chudlerboard(http: &serenity::Http) -> String {
    let players = storage::load_chuds();
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
        Some(first.chud_ref().name.clone())
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
        ("strongest", |p| p.chud_ref().strength),
        ("smartest", |p| p.chud_ref().smarts),
        ("sneakiest", |p| p.chud_ref().stealth),
        ("expert", |p| p.chud_ref().experience),
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

/// Purge every message in the channel and reset the persistent message cache to default.
/// Shared by the job-board redraw and the attract/complete phase transitions.
pub async fn reset_channel_cache(http: &serenity::Http, channel_id: u64) {
    let _cache_guard = storage::message_cache_lock().await;
    tracing::info!("channel reset triggered");
    delete_all_messages_in_channel(http, channel_id).await;
    if let Err(e) = storage::save_message_cache(&Default::default()) {
        tracing::warn!(err = %e, "failed to clear message cache");
    }
}

/// Purge the channel, reset the message cache, and re-post all persistent board UI.
pub async fn recover_persistent_board_messages(
    http: &serenity::Http,
    channel_id: u64,
    board: &mut Board,
    job_queue: &mut JobQueue,
    merchant: &MerchantState,
    bot_user_id: u64,
    max_non_bot_messages: usize,
    max_jobs: usize,
    job_timeout_tick: u32,
) -> anyhow::Result<()> {
    reset_channel_cache(http, channel_id).await;
    cleanup_non_bot_messages(http, channel_id, bot_user_id, max_non_bot_messages).await;

    engine::refill_board(board, job_queue, max_jobs, job_timeout_tick, None, None);
    storage::save_board(board)?;
    storage::save_job_queue(job_queue)?;
    update_board_message(http, channel_id, board, max_jobs, None).await?;
    update_merchant_message(http, channel_id, merchant).await
}

pub async fn update_board_message(
    http: &serenity::Http,
    channel_id: u64,
    board: &Board,
    max_jobs: usize,
    status: Option<&GuildHallStatus>,
) -> anyhow::Result<()> {
    let _render_guard = BOARD_RENDER_LOCK.lock().await;
    let ch = serenity::ChannelId::new(channel_id);

    let mut cache = {
        let _cache_guard = storage::message_cache_lock().await;
        storage::load_message_cache().unwrap_or_default()
    };
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
        if edit_cv2(http, ch, MessageId::new(msg_id), &header_message)
            .await
            .is_ok()
        {
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

    for slot in cache.slots.iter_mut() {
        if let Some(jid) = slot.job_id {
            if !board.quests.iter().any(|q| q.id == jid) {
                slot.job_id = None;
                cache_dirty = true;
            }
        }
    }

    for quest in &board.quests {
        let already_slotted = cache.slots.iter().any(|s| s.job_id == Some(quest.id));
        if !already_slotted {
            if let Some(i) = first_empty_slot(&cache.slots) {
                cache.slots[i].job_id = Some(quest.id);
                cache_dirty = true;
            }
        }
    }

    for (i, slot) in cache.slots.iter_mut().enumerate() {
        let quest = slot.job_id.and_then(|jid| board.quests.iter().find(|q| q.id == jid));
        let slot_message = build_job_slot(quest);
        let expected_key = slot_message.cache_key();
        if sync_job_slot(http, ch, slot, i, &slot_message, &expected_key).await {
            cache_dirty = true;
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
        if edit_cv2(http, ch, MessageId::new(msg_id), &status_message)
            .await
            .is_ok()
        {
            cache.status_content = status_key;
            cache_dirty = true;
            tracing::debug!(msg_id, "guild hall status message edited");
        } else {
            tracing::warn!(msg_id, "failed to edit guild hall status message");
        }
    }

    if cache_dirty {
        let _cache_guard = storage::message_cache_lock().await;
        let mut disk = storage::load_message_cache().unwrap_or_default();
        crate::game::persistence::message_cache::merge_board_ui_cache(&cache, &mut disk);
        if let Err(e) = storage::save_message_cache(&disk) {
            tracing::warn!(err = %e, "failed to save message cache");
        }
    }
    Ok(())
}