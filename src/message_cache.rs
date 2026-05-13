use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MessageCache {
    /// Last content string sent to the chudlerboard Discord message.
    #[serde(default)]
    pub chudlerboard: String,
    /// Last content string sent to each job slot Discord message (indexed by slot).
    #[serde(default)]
    pub job_slots: Vec<String>,
    /// Discord message IDs for each job slot (index == slot position, length ≤ MAX_JOBS).
    #[serde(default)]
    pub job_slot_message_ids: Vec<u64>,
    /// Discord message ID of the persistent divider posted between job slots and buffered messages.
    #[serde(default)]
    pub divider_message_id: Option<u64>,
    /// Discord message IDs to delete at the start of the next tick.
    #[serde(default)]
    pub pending_deletes: Vec<u64>,
}
