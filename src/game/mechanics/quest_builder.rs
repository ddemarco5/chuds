use rand::Rng;
use serde::{Deserialize, Serialize};
use rand_distr::{Distribution, Normal};

use crate::game::domain::player::Player;
use crate::game::generation::quest_generator::TrialStats;

/// Standard deviation for the trial-count normal distribution. Tweak to taste.
pub const TRIAL_COUNT_STDDEV: f64 = 1.0;
const MIN_TRIALS: usize = 1;
/// Probability (0.0-1.0) that any given stat is irrelevant (set to 0) for a trial.
const STAT_ZERO_CHANCE: f64 = 0.10;
pub fn roll_trial_count(difficulty: u8, rng: &mut impl Rng) -> usize {
    let mean = difficulty as f64 / 2.0;
    let normal = Normal::new(mean, TRIAL_COUNT_STDDEV).expect("valid normal distribution");
    let sample = normal.sample(rng).round() as i64;
    sample.max(MIN_TRIALS as i64) as usize
}

pub fn roll_stats(difficulty: u8, rng: &mut impl Rng) -> TrialStats {
    let floor = difficulty.max(1);
    let str_val = if rng.gen_bool(STAT_ZERO_CHANCE) { 0 } else { rng.gen_range(1..=floor) };
    let smt_val = if rng.gen_bool(STAT_ZERO_CHANCE) { 0 } else { rng.gen_range(1..=floor) };
    let sth_val = if rng.gen_bool(STAT_ZERO_CHANCE) { 0 } else { rng.gen_range(1..=floor) };
    let mut stats = TrialStats { strength: str_val, smarts: smt_val, stealth: sth_val };
    if stats.strength == 0 && stats.smarts == 0 && stats.stealth == 0 {
        match rng.gen_range(0..3u8) {
            0 => stats.strength = rng.gen_range(1..=floor),
            1 => stats.smarts   = rng.gen_range(1..=floor),
            _ => stats.stealth  = rng.gen_range(1..=floor),
        }
    }
    stats
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatChoice {
    Strength,
    Smarts,
    Stealth,
}

impl StatChoice {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Strength => "strength",
            Self::Smarts => "smarts",
            Self::Stealth => "stealth",
        }
    }

    pub fn player_stat(&self, player: &Player) -> u8 {
        match self {
            Self::Strength => player.strength,
            Self::Smarts => player.smarts,
            Self::Stealth => player.stealth,
        }
    }

    pub fn trial_required(&self, stats: &TrialStats) -> u8 {
        match self {
            Self::Strength => stats.strength,
            Self::Smarts => stats.smarts,
            Self::Stealth => stats.stealth,
        }
    }
}

pub struct TrialOutcome {
    pub stats: TrialStats,
    pub stat_used: StatChoice,
    pub raw_roll: u8,
    pub roll_modifier: i16,
    pub floor_applied: bool,
    pub player_roll: u8,
    pub trial_roll: u8,
    pub player_stat: u8,
    pub required: u8,
    pub passed: bool,
}

pub struct PlayedQuest {
    pub outcomes: Vec<TrialOutcome>,
}

pub fn roll_trials(difficulty: u8) -> Vec<TrialStats> {
    let mut rng = rand::thread_rng();
    let trial_count = roll_trial_count(difficulty, &mut rng);
    (0..trial_count).map(|_| roll_stats(difficulty, &mut rng)).collect()
}
