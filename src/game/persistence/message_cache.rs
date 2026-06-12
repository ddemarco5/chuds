use serde::{Deserialize, Deserializer, Serialize};

/// How an activity log line is rendered when synced to Discord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLogKind {
    /// U+2043 hyphen bullet prefix.
    #[default]
    Standard,
    /// Italic text, no bullet (world / ambient narration).
    World,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityLogEntry {
    pub kind: ActivityLogKind,
    pub text: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ActivityLogEntryWire {
    Plain(String),
    Structured {
        #[serde(default)]
        kind: ActivityLogKind,
        text: String,
    },
}

impl From<ActivityLogEntryWire> for ActivityLogEntry {
    fn from(w: ActivityLogEntryWire) -> Self {
        match w {
            ActivityLogEntryWire::Plain(text) => ActivityLogEntry {
                kind: ActivityLogKind::Standard,
                text,
            },
            ActivityLogEntryWire::Structured { kind, text } => ActivityLogEntry { kind, text },
        }
    }
}

fn deserialize_activity_log_entries<'de, D>(
    deserializer: D,
) -> Result<Vec<ActivityLogEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let wires = Vec::<ActivityLogEntryWire>::deserialize(deserializer)?;
    Ok(wires.into_iter().map(Into::into).collect())
}

/// State for a single persistent job-slot Discord message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobSlot {
    /// Discord message ID for this slot's persistent post (None until first posted).
    pub message_id: Option<u64>,
    /// Last content string sent to this slot's Discord message.
    #[serde(default)]
    pub content: String,
    /// The quest ID currently shown in this slot (None = empty / "Nothing posted here").
    pub job_id: Option<u32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MessageCache {
    /// Persistent job-slot state: one entry per slot, length ≤ MAX_JOBS.
    /// Each slot tracks its Discord message ID, last-sent content, and the
    /// quest ID currently assigned to it (None = empty).
    #[serde(default)]
    pub slots: Vec<JobSlot>,
    /// Discord message ID of the persistent job board header message.
    #[serde(default)]
    pub header_message_id: Option<u64>,
    /// Last content string sent to the job board header message.
    #[serde(default)]
    pub job_board_header: String,
    /// Discord message ID of the persistent guild-hall status area below job slots.
    #[serde(default, alias = "divider_message_id")]
    pub status_message_id: Option<u64>,
    /// Last content fingerprint sent to the status message.
    #[serde(default)]
    pub status_content: String,
    /// Cached random header for the idle roster section (cleared when the section is empty).
    #[serde(default)]
    pub status_idle_header: Option<String>,
    /// Cached random header for the all-busy section (cleared when the section is empty).
    #[serde(default)]
    pub status_all_busy_header: Option<String>,
    /// Cached random header for the no-chuds section (cleared when chuds exist again).
    #[serde(default)]
    pub status_no_chuds_header: Option<String>,
    /// Cached random header for the hospital roster section (cleared when the section is empty).
    #[serde(default)]
    pub status_hospital_header: Option<String>,
    /// Discord message ID of the persistent merchant area below guild status.
    #[serde(default)]
    pub merchant_message_id: Option<u64>,
    /// Last content fingerprint sent to the merchant message.
    #[serde(default)]
    pub merchant_content: String,
    /// Cached visiting announcement for the current merchant visit (cleared when no visit).
    #[serde(default)]
    pub merchant_visit_text: Option<String>,
    /// Identity of the merchant being announced (`index:name`), used to invalidate the cache.
    #[serde(default)]
    pub merchant_visit_identity: Option<String>,
    /// Discord message ID of the single attract/complete phase-screen message.
    #[serde(default)]
    pub phase_screen_message_id: Option<u64>,
    /// Last content fingerprint sent to the phase-screen message.
    #[serde(default)]
    pub phase_screen_content: String,
    /// Discord message ID of the persistent activity log below the board.
    #[serde(default)]
    pub activity_log_message_id: Option<u64>,
    /// Activity log entries (kind controls bullet vs italic rendering on sync).
    #[serde(default, deserialize_with = "deserialize_activity_log_entries")]
    pub activity_log_entries: Vec<ActivityLogEntry>,
}

/// Copy job-board UI fields from a working cache into the on-disk cache without
/// clobbering activity log, merchant, or phase-screen state updated concurrently.
pub fn merge_board_ui_cache(from: &MessageCache, into: &mut MessageCache) {
    into.slots = from.slots.clone();
    into.header_message_id = from.header_message_id;
    into.job_board_header = from.job_board_header.clone();
    into.status_message_id = from.status_message_id;
    into.status_content = from.status_content.clone();
    into.status_idle_header = from.status_idle_header.clone();
    into.status_all_busy_header = from.status_all_busy_header.clone();
    into.status_no_chuds_header = from.status_no_chuds_header.clone();
    into.status_hospital_header = from.status_hospital_header.clone();
}

/// Discord message IDs for every persistent channel post tracked in cache.
pub fn persistent_message_ids(cache: &MessageCache) -> Vec<(&'static str, u64)> {
    let mut message_ids = Vec::new();

    if let Some(id) = cache.header_message_id {
        message_ids.push(("header", id));
    }
    if let Some(id) = cache.status_message_id {
        message_ids.push(("status", id));
    }
    if let Some(id) = cache.merchant_message_id {
        message_ids.push(("merchant", id));
    }
    if let Some(id) = cache.activity_log_message_id {
        message_ids.push(("activity_log", id));
    }
    if let Some(id) = cache.phase_screen_message_id {
        message_ids.push(("phase_screen", id));
    }
    for slot in &cache.slots {
        if let Some(id) = slot.message_id {
            message_ids.push(("slot", id));
        }
    }

    message_ids
}

/// Copy merchant UI fields from a working cache into the on-disk cache.
pub fn merge_merchant_ui_cache(from: &MessageCache, into: &mut MessageCache) {
    into.merchant_message_id = from.merchant_message_id;
    into.merchant_content = from.merchant_content.clone();
    into.merchant_visit_text = from.merchant_visit_text.clone();
    into.merchant_visit_identity = from.merchant_visit_identity.clone();
}
