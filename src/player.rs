use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::quest_result::QuestResult;

const MAX_STAT: u8 = 10;
/// Threshold = STAT_LEVEL_BASE * current_stat. Scales cost upward as the stat grows.
pub const STAT_LEVEL_BASE: u32 = 8;
/// Threshold = EXP_LEVEL_BASE * current_experience. Scales cost upward as experience grows.
pub const EXP_LEVEL_BASE: u32 = 5;

#[derive(Debug, Serialize, Deserialize)]
pub struct Player {
    pub discord_user_id: u64,
    pub name: String,
    pub description: String,
    pub strength: u8,
    pub smarts: u8,
    pub stealth: u8,
    pub experience: u8,
    pub str_successes: u32,
    pub smt_successes: u32,
    pub sth_successes: u32,
    pub quest_successes: u32,
    pub quest_failures: u32,
}

fn apply_wins(val: &mut u8, counter: &mut u32, wins: u32, base: u32) -> bool {
    *counter += wins;
    let threshold = base * *val as u32;
    if *counter >= threshold && *val < MAX_STAT {
        *val += 1;
        *counter -= threshold;
        true
    } else {
        false
    }
}

impl Player {
    pub fn format_stats(&self) -> String {
        format!(
            "{} -- *Strength {}, Smarts {}, Stealth {}, Experience {}*",
            self.name, self.strength, self.smarts, self.stealth, self.experience
        )
    }

    pub fn record_quest(&mut self, result: &QuestResult) {
        let (str_wins, smt_wins, sth_wins) = result.stat_wins();

        let str_up = apply_wins(&mut self.strength,  &mut self.str_successes, str_wins, STAT_LEVEL_BASE);
        let smt_up = apply_wins(&mut self.smarts,    &mut self.smt_successes, smt_wins, STAT_LEVEL_BASE);
        let sth_up = apply_wins(&mut self.stealth,   &mut self.sth_successes, sth_wins, STAT_LEVEL_BASE);

        let exp_up = if result.passed {
            apply_wins(&mut self.experience, &mut self.quest_successes, 1, EXP_LEVEL_BASE)
        } else {
            self.quest_failures += 1;
            false
        };

        if str_up { tracing::info!(name = %self.name, stat = "strength", value = self.strength, "[LEVEL UP]"); }
        if smt_up { tracing::info!(name = %self.name, stat = "smarts",   value = self.smarts,   "[LEVEL UP]"); }
        if sth_up { tracing::info!(name = %self.name, stat = "stealth",  value = self.stealth,  "[LEVEL UP]"); }
        if exp_up { tracing::info!(name = %self.name, stat = "experience", value = self.experience, "[LEVEL UP]"); }

        tracing::info!(
            name = %self.name,
            str = %format!("{} ({}/{})", self.strength, self.str_successes, STAT_LEVEL_BASE * self.strength as u32),
            smt = %format!("{} ({}/{})", self.smarts, self.smt_successes, STAT_LEVEL_BASE * self.smarts as u32),
            sth = %format!("{} ({}/{})", self.stealth, self.sth_successes, STAT_LEVEL_BASE * self.stealth as u32),
            exp = %format!("{} ({}/{})", self.experience, self.quest_successes, EXP_LEVEL_BASE * self.experience as u32),
            "chud stats updated"
        );
    }
}

/// Create a new chud for the given Discord user with rolled stats.
/// Stats are each rolled 1–3, then trimmed (highest first) until the sum is ≤ 5.
pub fn create_chud(discord_user_id: u64, name: String, description: String) -> Player {
    let mut rng = rand::thread_rng();

    let mut strength = rng.gen_range(1u8..=3);
    let mut smarts   = rng.gen_range(1u8..=3);
    let mut stealth  = rng.gen_range(1u8..=3);

    while strength + smarts + stealth > 5 {
        if strength >= smarts && strength >= stealth {
            strength -= 1;
        } else if smarts >= stealth {
            smarts -= 1;
        } else {
            stealth -= 1;
        }
    }

    Player {
        discord_user_id,
        name,
        description,
        strength,
        smarts,
        stealth,
        experience: 1,
        str_successes: 0,
        smt_successes: 0,
        sth_successes: 0,
        quest_successes: 0,
        quest_failures: 0,
    }
}
