use serde::{Deserialize, Serialize};

use crate::game::domain::episode_stats::EpisodeStats;
use crate::game::domain::item::Item;
use crate::game::domain::quest_result::QuestResult;
use crate::game::domain::stash::Stash;

const MAX_STAT: u8 = 10;

#[derive(Debug, Default)]
pub struct LevelUp {
    pub str_up: bool,
    pub smt_up: bool,
    pub sth_up: bool,
    pub exp_up: bool,
}

impl LevelUp {
    pub fn any(&self) -> bool {
        self.str_up || self.smt_up || self.sth_up || self.exp_up
    }
}

/// Wins required to advance a combat stat one level = STAT_LEVEL_BASE × current_level.
/// A "win" is a trial passed using that specific stat.
///
/// Wins needed per step, and cumulative total to reach level 6:
///
/// base │ 1→2  2→3  3→4  4→5  5→6 │ total (1→6)
/// ─────┼──────────────────────────┼────────────
///   2  │   2    4    6    8   10  │     30
///   3  │   3    6    9   12   15  │     45   ← current
///   4  │   4    8   12   16   20  │     60
///   5  │   5   10   15   20   25  │     75
pub const STAT_LEVEL_BASE: u32 = 3;

/// Quests passed required to advance experience one level = EXP_LEVEL_BASE × current_level.
/// Only successful quest completions count; failures do not.
///
/// Quests needed per step, and cumulative total to reach level 6:
///
/// base │ 1→2  2→3  3→4  4→5  5→6 │ total (1→6)
/// ─────┼──────────────────────────┼────────────
///   1  │   1    2    3    4    5  │     15
///   2  │   2    4    6    8   10  │     30   ← current
///   3  │   3    6    9   12   15  │     45
///   4  │   4    8   12   16   20  │     60
pub const EXP_LEVEL_BASE: u32 = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChudEquipment {
    pub gear: Option<u32>,
    pub weapon: Option<u32>,
    #[serde(default)]
    pub misc: [Option<u32>; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chud {
    pub name: String,
    pub description: String,
    pub strength: u8,
    pub smarts: u8,
    pub stealth: u8,
    pub experience: u8,
    pub str_successes: u32,
    pub smt_successes: u32,
    pub sth_successes: u32,
    pub job_successes: u32,
    pub total_job_successes: u32,
    pub total_job_failures: u32,
    #[serde(default)]
    pub equipment: ChudEquipment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub discord_user_id: u64,
    pub cash: u32,
    #[serde(default)]
    pub stash: Stash,
    #[serde(default)]
    pub chud: Option<Chud>,
}

fn format_stat_value(label: &str, raw: u8, effective: u8) -> String {
    if raw != effective {
        format!("{label} ~~{raw}~~ {effective}")
    } else {
        format!("{label} {raw}")
    }
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
    pub fn has_chud(&self) -> bool {
        self.chud.is_some()
    }

    pub fn chud_ref(&self) -> &Chud {
        self.chud.as_ref().expect("player has no chud")
    }

    pub fn chud_mut(&mut self) -> &mut Chud {
        self.chud.as_mut().expect("player has no chud")
    }

    /// Credit cash and optionally record the earning against episode stats.
    pub fn earn_cash(&mut self, amount: u32, stats: Option<&mut EpisodeStats>) {
        if amount == 0 {
            return;
        }
        self.cash = self.cash.saturating_add(amount);
        if let Some(stats) = stats {
            if self.has_chud() {
                stats.record_money_earned(self.discord_user_id, &self.chud_ref().name, amount);
            }
        }
    }

    /// Debit cash when affordable and optionally record the spend against episode stats.
    pub fn try_spend_cash(&mut self, amount: u32, stats: Option<&mut EpisodeStats>) -> bool {
        if amount == 0 {
            return true;
        }
        if self.cash < amount {
            return false;
        }
        self.cash -= amount;
        if let Some(stats) = stats {
            if self.has_chud() {
                stats.record_money_spent(self.discord_user_id, &self.chud_ref().name, amount);
            }
        }
        true
    }

    pub fn format_job_record(&self) -> String {
        let chud = self.chud_ref();
        format!(
            "{} job completed, {} failed",
            chud.total_job_successes, chud.total_job_failures
        )
    }

    /// Base chud bio plus one sentence per equipped item (gear → weapon → misc slots).
    pub fn generate_description<'a>(
        &self,
        equipped_items: impl IntoIterator<Item = &'a Item>,
    ) -> String {
        let mut out = self.chud_ref().description.clone();
        for item in equipped_items {
            if let Some(sentence) = item.equipped_description_sentence() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&sentence);
            }
        }
        out
    }

    /// Equipment-adjusted stat values; strikethrough on raw values when gear modifies them.
    pub fn format_effective_stats(&self, effective: (u8, u8, u8)) -> String {
        let chud = self.chud_ref();
        let (eff_str, eff_smt, eff_sth) = effective;
        format!(
            "{}, {}, {}, Experience {}",
            format_stat_value("Strength", chud.strength, eff_str),
            format_stat_value("Smarts", chud.smarts, eff_smt),
            format_stat_value("Stealth", chud.stealth, eff_sth),
            chud.experience,
        )
    }

    pub fn format_effective_stats_line(&self, effective: (u8, u8, u8)) -> String {
        format!("Stats: {}", self.format_effective_stats(effective))
    }

    pub fn record_quest(&mut self, result: &QuestResult) -> LevelUp {
        let chud = self.chud_mut();
        let (str_wins, smt_wins, sth_wins) = result.stat_wins();

        let str_up = apply_wins(&mut chud.strength, &mut chud.str_successes, str_wins, STAT_LEVEL_BASE);
        let smt_up = apply_wins(&mut chud.smarts, &mut chud.smt_successes, smt_wins, STAT_LEVEL_BASE);
        let sth_up = apply_wins(&mut chud.stealth, &mut chud.sth_successes, sth_wins, STAT_LEVEL_BASE);

        let exp_up = if result.passed {
            chud.total_job_successes += 1;
            apply_wins(&mut chud.experience, &mut chud.job_successes, 1, EXP_LEVEL_BASE)
        } else {
            chud.total_job_failures += 1;
            false
        };

        if str_up {
            tracing::info!(name = %chud.name, stat = "strength", value = chud.strength, "[LEVEL UP]");
        }
        if smt_up {
            tracing::info!(name = %chud.name, stat = "smarts", value = chud.smarts, "[LEVEL UP]");
        }
        if sth_up {
            tracing::info!(name = %chud.name, stat = "stealth", value = chud.stealth, "[LEVEL UP]");
        }
        if exp_up {
            tracing::info!(name = %chud.name, stat = "experience", value = chud.experience, "[LEVEL UP]");
        }

        tracing::info!(
            name = %chud.name,
            str = %format!("{} ({}/{})", chud.strength, chud.str_successes, STAT_LEVEL_BASE * chud.strength as u32),
            smt = %format!("{} ({}/{})", chud.smarts, chud.smt_successes, STAT_LEVEL_BASE * chud.smarts as u32),
            sth = %format!("{} ({}/{})", chud.stealth, chud.sth_successes, STAT_LEVEL_BASE * chud.stealth as u32),
            exp = %format!("{} ({}/{})", chud.experience, chud.job_successes, EXP_LEVEL_BASE * chud.experience as u32),
            "chud stats updated"
        );

        LevelUp { str_up, smt_up, sth_up, exp_up }
    }
}

/// Create a new chud for the given Discord user. All stats start at 1.
pub fn create_chud(discord_user_id: u64, name: String, description: String) -> Player {
    Player {
        discord_user_id,
        cash: 0,
        stash: Stash::default(),
        chud: Some(Chud {
            name,
            description,
            strength: 1,
            smarts: 1,
            stealth: 1,
            experience: 1,
            str_successes: 0,
            smt_successes: 0,
            sth_successes: 0,
            job_successes: 0,
            total_job_successes: 0,
            total_job_failures: 0,
            equipment: ChudEquipment::default(),
        }),
    }
}
