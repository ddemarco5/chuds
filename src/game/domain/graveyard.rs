use serde::{Deserialize, Serialize};

use crate::game::domain::player::Chud;

/// A fallen chud resting in the graveyard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraveyardEntry {
    pub discord_user_id: u64,
    pub chud: Chud,
    pub epitaph: String,
    pub death_trial: String,
    pub death_outcome: String,
}

/// Graveyard state containing all buried chuds (newest first).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graveyard {
    pub entries: Vec<GraveyardEntry>,
}

impl Graveyard {
    pub fn bury(&mut self, entry: GraveyardEntry) {
        self.entries.insert(0, entry);
    }
}
