use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TrialStats {
    pub strength: u8,
    pub smarts: u8,
    pub stealth: u8,
}
