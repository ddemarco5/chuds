use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude::{self as serenity, GetMessages, MessageId};

use crate::game::persistence::message_cache::{
    persistent_message_ids, ActivityLogEntry, ActivityLogKind, MessageCache,
};
use crate::game::persistence::storage;

const DISCORD_MESSAGE_LIMIT: usize = 2000;
/// U+2043 HYPHEN BULLET — renders as a small bullet in Discord, not a round list marker.
const LOG_BULLET: char = '\u{2043}';
/// Zero-width space + tab pairs (same trick as the job board header) for a slight indent.
const WORLD_INDENT: &str = "\u{200B}\t\u{200B}\t";

/// Shared state for debounced activity-log Discord edits.
pub struct ActivityLogSync {
    http: Arc<serenity::Http>,
    pub channel_id: u64,
    pub max_lines: usize,
    pub debounce: Duration,
    sync_scheduled: AtomicBool,
}

impl ActivityLogSync {
    pub fn new(
        http: Arc<serenity::Http>,
        channel_id: u64,
        max_lines: usize,
        debounce: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            http,
            channel_id,
            max_lines,
            debounce,
            sync_scheduled: AtomicBool::new(false),
        })
    }
}

fn format_standard_entry(raw: &str) -> String {
    let mut lines = raw.lines();
    let first = lines.next().unwrap_or("");
    let mut out = format!("{LOG_BULLET} {first}");
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out
}

fn format_world_entry(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{WORLD_INDENT}*{line}*")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_log_entry(entry: &ActivityLogEntry) -> String {
    match entry.kind {
        ActivityLogKind::Standard => format_standard_entry(&entry.text),
        ActivityLogKind::World => format_world_entry(&entry.text),
    }
}

fn render_activity_log(entries: &[ActivityLogEntry]) -> String {
    entries
        .iter()
        .map(format_log_entry)
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim_activity_log_entries(entries: &mut Vec<ActivityLogEntry>, max_lines: usize) {
    while entries.len() > max_lines {
        entries.remove(0);
    }
    while !entries.is_empty() && render_activity_log(entries).len() > DISCORD_MESSAGE_LIMIT {
        entries.remove(0);
    }
}

async fn append_to_activity_log_cache(
    max_lines: usize,
    kind: ActivityLogKind,
    content: &str,
) {
    let _guard = storage::message_cache_lock().await;
    let mut cache = storage::load_message_cache().unwrap_or_default();
    cache.activity_log_entries.push(ActivityLogEntry {
        kind,
        text: content.to_string(),
    });
    trim_activity_log_entries(&mut cache.activity_log_entries, max_lines);
    if let Err(e) = storage::save_message_cache(&cache) {
        tracing::warn!(err = %e, "failed to save message cache after activity log append");
    }
}

/// Append to the activity log cache only (no Discord sync).
pub async fn append_activity_log_deferred(
    log: &ActivityLogSync,
    kind: ActivityLogKind,
    content: &str,
) {
    append_to_activity_log_cache(log.max_lines, kind, content).await;
}

/// Append a standard (bulleted) entry and spawn a debounced Discord sync if none is scheduled.
pub async fn append_activity_log(log: &Arc<ActivityLogSync>, content: &str) {
    append_activity_log_with_kind(log, ActivityLogKind::Standard, content).await;
}

/// Append to cache and spawn a debounced Discord sync if none is already scheduled.
pub async fn append_activity_log_with_kind(
    log: &Arc<ActivityLogSync>,
    kind: ActivityLogKind,
    content: &str,
) {
    append_to_activity_log_cache(log.max_lines, kind, content).await;

    if log
        .sync_scheduled
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let log = Arc::clone(log);
    tokio::spawn(async move {
        tokio::time::sleep(log.debounce).await;
        sync_activity_log_now(&log.http, log.channel_id).await;
        log.sync_scheduled.store(false, Ordering::Release);
    });
}

/// Create or edit the persistent activity log message from cache.
pub async fn sync_activity_log_now(http: &serenity::Http, channel_id: u64) {
    let (body, existing_msg_id) = {
        let _guard = storage::message_cache_lock().await;
        let cache = storage::load_message_cache().unwrap_or_default();
        if cache.activity_log_entries.is_empty() {
            return;
        }
        (
            render_activity_log(&cache.activity_log_entries),
            cache.activity_log_message_id,
        )
    };

    let ch = serenity::ChannelId::new(channel_id);

    let posted_msg_id = if let Some(msg_id) = existing_msg_id {
        match http
            .edit_message(
                ch,
                MessageId::new(msg_id),
                &serenity::EditMessage::new().content(&body),
                vec![],
            )
            .await
        {
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(msg_id, err = %e, "failed to edit activity log, posting new message");
                match http
                    .send_message(ch, vec![], &serenity::CreateMessage::new().content(&body))
                    .await
                {
                    Ok(msg) => Some(msg.id.get()),
                    Err(e) => {
                        tracing::warn!(err = %e, "failed to post activity log message");
                        return;
                    }
                }
            }
        }
    } else {
        match http
            .send_message(ch, vec![], &serenity::CreateMessage::new().content(&body))
            .await
        {
            Ok(msg) => Some(msg.id.get()),
            Err(e) => {
                tracing::warn!(err = %e, "failed to post activity log message");
                return;
            }
        }
    };

    if let Some(new_id) = posted_msg_id {
        let _guard = storage::message_cache_lock().await;
        let mut cache = storage::load_message_cache().unwrap_or_default();
        cache.activity_log_message_id = Some(new_id);
        if let Err(e) = storage::save_message_cache(&cache) {
            tracing::warn!(err = %e, "failed to save message cache after activity log post");
        }
    }
}

/// Delete all messages in the channel (both bot and non-bot messages).
pub async fn delete_all_messages_in_channel(http: &serenity::Http, channel_id: u64) {
    let ch = serenity::ChannelId::new(channel_id);
    let messages = match ch.messages(http, GetMessages::new().limit(100)).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(err = %e, "failed to fetch channel messages for purge");
            return;
        }
    };
    if messages.is_empty() {
        return;
    }
    tracing::info!(count = messages.len(), "purging all messages in channel");
    for msg in &messages {
        if let Err(e) = http.delete_message(ch, msg.id, None).await {
            tracing::warn!(msg_id = msg.id.get(), err = %e, "failed to delete message during purge");
        }
    }
}

async fn clear_message_reactions_if_any(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    message_id: MessageId,
    kind: &str,
) {
    let msg = match http.get_message(channel_id, message_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(kind, msg_id = message_id.get(), err = %e, "skipped reaction clear: message not found");
            return;
        }
    };
    if msg.reactions.is_empty() {
        return;
    }
    if let Err(e) = http.delete_message_reactions(channel_id, message_id).await {
        tracing::warn!(kind, msg_id = message_id.get(), err = %e, "failed to clear reactions on persistent message");
    }
}

/// Clear reactions on every persistent channel message when any are present.
pub async fn clear_persistent_message_reactions(http: &serenity::Http, channel_id: u64) {
    let message_ids = {
        let _guard = storage::message_cache_lock().await;
        let cache = storage::load_message_cache().unwrap_or_default();
        persistent_message_ids(&cache)
    };

    if message_ids.is_empty() {
        return;
    }

    let ch = serenity::ChannelId::new(channel_id);
    for (kind, id) in message_ids {
        clear_message_reactions_if_any(http, ch, MessageId::new(id), kind).await;
    }
}

/// Validate that all cached message IDs still exist in Discord.
pub async fn validate_cached_messages_exist(
    http: &serenity::Http,
    channel_id: u64,
    cache: &MessageCache,
) -> bool {
    let ch = serenity::ChannelId::new(channel_id);

    let message_ids = persistent_message_ids(cache);

    if message_ids.is_empty() {
        return true;
    }

    for (kind, id) in message_ids {
        let msg_id = serenity::MessageId::new(id);
        match http.get_message(ch, msg_id).await {
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(kind, msg_id = id, err = %e, "cached message not found");
                return false;
            }
        }
    }

    true
}

/// Delete all non-bot messages in the channel when their count exceeds `threshold`.
pub async fn cleanup_non_bot_messages(
    http: &serenity::Http,
    channel_id: u64,
    bot_user_id: u64,
    threshold: usize,
) {
    let ch = serenity::ChannelId::new(channel_id);
    let messages = match ch.messages(http, GetMessages::new().limit(100)).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(err = %e, "failed to fetch channel messages for cleanup");
            return;
        }
    };
    let non_bot: Vec<_> = messages
        .iter()
        .filter(|m| m.author.id.get() != bot_user_id)
        .collect();
    if non_bot.len() > threshold {
        tracing::info!(count = non_bot.len(), threshold, "cleaning up non-bot messages in channel");
        for msg in &non_bot {
            if let Err(e) = http.delete_message(ch, msg.id, None).await {
                tracing::warn!(msg_id = msg.id.get(), err = %e, "failed to delete non-bot message during cleanup");
            }
        }
    }
}
