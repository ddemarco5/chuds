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
#[derive(Debug, Default, Serialize, Deserialize)]
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
    /// Cached random header for the hospital roster section (cleared when the section is empty).
    #[serde(default)]
    pub status_hospital_header: Option<String>,
    /// Discord message ID of the persistent activity log below the board.
    #[serde(default)]
    pub activity_log_message_id: Option<u64>,
    /// Activity log entries (kind controls bullet vs italic rendering on sync).
    #[serde(default, deserialize_with = "deserialize_activity_log_entries")]
    pub activity_log_entries: Vec<ActivityLogEntry>,
}
