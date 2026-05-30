use poise::serenity_prelude::{self as serenity, GetMessages, MessageId};

use crate::game::domain::board::Board;
use crate::game::persistence::message_cache::MessageCache;
use crate::game::persistence::storage;

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

/// Validate that all cached message IDs still exist in Discord.
pub async fn validate_cached_messages_exist(
    http: &serenity::Http,
    channel_id: u64,
    board: &Board,
    cache: &MessageCache,
) -> bool {
    let ch = serenity::ChannelId::new(channel_id);

    let mut message_ids = Vec::new();

    if let Some(id) = board.chudlerboard_message_id {
        message_ids.push(("chudlerboard", id));
    }

    if let Some(id) = cache.header_message_id {
        message_ids.push(("header", id));
    }

    if let Some(id) = cache.divider_message_id {
        message_ids.push(("divider", id));
    }

    for slot in &cache.slots {
        if let Some(id) = slot.message_id {
            message_ids.push(("slot", id));
        }
    }

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

/// Post a message to the channel buffer area below the job board.
pub async fn post_buffered_message(
    http: &serenity::Http,
    channel_id: u64,
    max_buffer: usize,
    content: &str,
) {
    post_buffered_message_inner(http, channel_id, max_buffer, content, true).await;
}

/// Post a buffered message without evicting older ones. Call [`trim_buffered_messages`]
/// once the batch is complete (e.g. at the end of a tick).
pub async fn post_buffered_message_deferred(
    http: &serenity::Http,
    channel_id: u64,
    content: &str,
) {
    post_buffered_message_inner(http, channel_id, 0, content, false).await;
}

async fn post_buffered_message_inner(
    http: &serenity::Http,
    channel_id: u64,
    max_buffer: usize,
    content: &str,
    evict_before_post: bool,
) {
    let _guard = storage::message_cache_lock().await;
    let mut cache = storage::load_message_cache().unwrap_or_default();
    let ch = serenity::ChannelId::new(channel_id);
    if evict_before_post && cache.pending_deletes.len() >= max_buffer {
        let old_id = cache.pending_deletes.remove(0);
        if let Err(e) = http.delete_message(ch, MessageId::new(old_id), None).await {
            tracing::warn!(msg_id = old_id, err = %e, "failed to evict oldest buffered message");
        }
    }
    match http
        .send_message(ch, vec![], &serenity::CreateMessage::new().content(content))
        .await
    {
        Ok(msg) => {
            cache.pending_deletes.push(msg.id.get());
            if let Err(e) = storage::save_message_cache(&cache) {
                tracing::warn!(err = %e, "failed to save message cache after buffered post");
            }
        }
        Err(e) => tracing::warn!(err = %e, "failed to post buffered message"),
    }
}

/// Delete oldest buffered messages until at most `max_buffer` remain.
pub async fn trim_buffered_messages(
    http: &serenity::Http,
    channel_id: u64,
    max_buffer: usize,
) {
    let _guard = storage::message_cache_lock().await;
    let mut cache = storage::load_message_cache().unwrap_or_default();
    let ch = serenity::ChannelId::new(channel_id);
    while cache.pending_deletes.len() > max_buffer {
        let old_id = cache.pending_deletes.remove(0);
        if let Err(e) = http.delete_message(ch, MessageId::new(old_id), None).await {
            tracing::warn!(msg_id = old_id, err = %e, "failed to evict oldest buffered message");
        }
    }
    if let Err(e) = storage::save_message_cache(&cache) {
        tracing::warn!(err = %e, "failed to save message cache after buffer trim");
    }
}
