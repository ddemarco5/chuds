use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

use crate::game::domain::item::Item;
use crate::game::tuneable_rolls::roll_chud_stats;
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
    pub chud: Chud,
}

impl Deref for Player {
    type Target = Chud;
    fn deref(&self) -> &Self::Target {
        &self.chud
    }
}

impl DerefMut for Player {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.chud
    }
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
    pub fn format_stats(&self) -> String {
        format!(
            "{} -- *Strength {}, Smarts {}, Stealth {}, Experience {}*\n{} job completed, {} failed",
            self.name, self.strength, self.smarts, self.stealth, self.experience,
            self.total_job_successes, self.total_job_failures
        )
    }

    pub fn format_job_record(&self) -> String {
        format!(
            "{} job completed, {} failed",
            self.total_job_successes, self.total_job_failures
        )
    }

    /// Base chud bio plus one sentence per equipped item (gear → weapon → misc slots).
    pub fn generate_description<'a>(
        &self,
        equipped_items: impl IntoIterator<Item = &'a Item>,
    ) -> String {
        let mut out = self.description.clone();
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

    /// Discord stats line with strikethrough on raw values when equipment modifies them.
    pub fn format_effective_stats_line(&self, effective: (u8, u8, u8)) -> String {
        let (eff_str, eff_smt, eff_sth) = effective;
        format!(
            "Stats: {}, {}, {}, Experience {}",
            format_stat_value("Strength", self.strength, eff_str),
            format_stat_value("Smarts", self.smarts, eff_smt),
            format_stat_value("Stealth", self.stealth, eff_sth),
            self.experience,
        )
    }

    pub fn record_quest(&mut self, result: &QuestResult) -> LevelUp {
        let (str_wins, smt_wins, sth_wins) = result.stat_wins();

        let str_up = apply_wins(&mut self.chud.strength,  &mut self.chud.str_successes, str_wins, STAT_LEVEL_BASE);
        let smt_up = apply_wins(&mut self.chud.smarts,    &mut self.chud.smt_successes, smt_wins, STAT_LEVEL_BASE);
        let sth_up = apply_wins(&mut self.chud.stealth,   &mut self.chud.sth_successes, sth_wins, STAT_LEVEL_BASE);

        let exp_up = if result.passed {
            self.chud.total_job_successes += 1;
            apply_wins(&mut self.chud.experience, &mut self.chud.job_successes, 1, EXP_LEVEL_BASE)
        } else {
            self.chud.total_job_failures += 1;
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
            exp = %format!("{} ({}/{})", self.experience, self.job_successes, EXP_LEVEL_BASE * self.experience as u32),
            "chud stats updated"
        );

        LevelUp { str_up, smt_up, sth_up, exp_up }
    }
}

/// Create a new chud for the given Discord user with rolled stats.
/// Stats are each rolled 1–3, then trimmed (highest first) until the sum is ≤ 5.
pub fn create_chud(discord_user_id: u64, name: String, description: String) -> Player {
    let mut rng = rand::thread_rng();
    let (strength, smarts, stealth) = roll_chud_stats(&mut rng);

    Player {
        discord_user_id,
        cash: 0,
        stash: Stash::default(),
        chud: Chud {
            name,
            description,
            strength,
            smarts,
            stealth,
            experience: 1,
            str_successes: 0,
            smt_successes: 0,
            sth_successes: 0,
            job_successes: 0,
            total_job_successes: 0,
            total_job_failures: 0,
            equipment: ChudEquipment::default(),
        },
    }
}
