use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartingBenefitEntry {
    pub discord_user_id: u64,
    pub money: u32,
}

/// Head-start money awarded to a player when their next chud is created.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StartingBenefits {
    pub entries: Vec<StartingBenefitEntry>,
}

impl StartingBenefits {
    pub fn add(&mut self, discord_user_id: u64, amount: u32) {
        if amount == 0 {
            return;
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.discord_user_id == discord_user_id)
        {
            entry.money = entry.money.saturating_add(amount);
        } else {
            self.entries.push(StartingBenefitEntry {
                discord_user_id,
                money: amount,
            });
        }
    }

    /// Claim and remove benefits for a player. Returns 0 if none pending.
    pub fn take(&mut self, discord_user_id: u64) -> u32 {
        let idx = match self
            .entries
            .iter()
            .position(|e| e.discord_user_id == discord_user_id)
        {
            Some(idx) => idx,
            None => return 0,
        };
        self.entries.remove(idx).money
    }
}
