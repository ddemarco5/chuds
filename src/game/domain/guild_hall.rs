use serde::{Deserialize, Serialize};

use crate::game::merchant::MerchantState;

/// Persistent guild-hall state (merchant visits, etc.).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GuildHall {
    pub merchant: MerchantState,
}
