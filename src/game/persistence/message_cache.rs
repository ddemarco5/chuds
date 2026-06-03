use serde::{Deserialize, Serialize};

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
    /// Discord message IDs to delete at the start of the next tick.
    #[serde(default)]
    pub pending_deletes: Vec<u64>,
}
