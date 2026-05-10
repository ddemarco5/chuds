use rand::Rng;
use serde::{Deserialize, Serialize};
use rand_distr::{Distribution, Normal};

use crate::player::Player;
use crate::quest_generator::{GeneratedQuest, QuestData, TrialStats};

/// Standard deviation for the trial-count normal distribution. Tweak to taste.
pub const TRIAL_COUNT_STDDEV: f64 = 1.0;
const MIN_TRIALS: usize = 1;
/// Probability (0.0-1.0) that any given stat is irrelevant (set to 0) for a trial.
const STAT_ZERO_CHANCE: f64 = 0.10;
/// Probability a non-optimal valid stat is dropped from consideration at maximum experience (10).
/// Higher experience → higher dropout → chud focuses more consistently on the optimal stat.
const EXP_DROPOUT_MAX: f64 = 0.70;
/// Probability a non-optimal valid stat is dropped from consideration at minimum experience (1).
/// Lower experience → lower dropout → more random / exploratory stat selection.
const EXP_DROPOUT_MIN: f64 = 0.05;

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

    fn player_stat(&self, player: &Player) -> u8 {
        match self {
            Self::Strength => player.strength,
            Self::Smarts => player.smarts,
            Self::Stealth => player.stealth,
        }
    }

    fn trial_required(&self, stats: &TrialStats) -> u8 {
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
    pub player_roll: u8,
    pub trial_roll: u8,
    pub player_stat: u8,
    pub required: u8,
    pub passed: bool,
}

pub struct PlayedQuest {
    pub outcomes: Vec<TrialOutcome>,
}

fn choose_stat(stats: &TrialStats, player: &Player, rng: &mut impl Rng) -> StatChoice {
    let candidates = [
        (StatChoice::Strength, stats.strength, player.strength),
        (StatChoice::Smarts,   stats.smarts,   player.smarts),
        (StatChoice::Stealth,  stats.stealth,  player.stealth),
    ];
    let valid: Vec<(StatChoice, i16)> = candidates.iter()
        .filter(|&&(_, req, _)| req > 0)
        .map(|&(s, req, ps)| (s, ps as i16 - req as i16))
        .collect();
    let best_margin = valid.iter().map(|&(_, m)| m).max().unwrap_or(0);
    let t = (player.experience - 1) as f64 / 9.0;
    let dropout_prob = EXP_DROPOUT_MIN + t * (EXP_DROPOUT_MAX - EXP_DROPOUT_MIN);
    let considered: Vec<StatChoice> = valid.iter()
        .filter(|&&(_, m)| m == best_margin || !rng.gen_bool(dropout_prob))
        .map(|&(s, _)| s)
        .collect();
    let pool = if considered.is_empty() { valid.iter().map(|&(s, _)| s).collect() } else { considered };
    pool[rng.gen_range(0..pool.len())]
}

pub fn play_quest(quest: &QuestData, generated: &GeneratedQuest, player: &Player) -> anyhow::Result<PlayedQuest> {
    let mut rng = rand::thread_rng();
    let mut outcomes = Vec::new();

    for (stats, situation) in quest.trials.iter().zip(generated.trials.iter()) {
        let stat_used = choose_stat(stats, player, &mut rng);
        let player_stat = stat_used.player_stat(player);
        let required = stat_used.trial_required(stats);
        let player_roll: u8 = rng.gen_range(1..=player_stat);
        let trial_roll: u8  = rng.gen_range(1..=required);
        let passed = player_roll >= trial_roll;

        outcomes.push(TrialOutcome {
            stats: stats.clone(),
            stat_used,
            player_roll,
            trial_roll,
            player_stat,
            required,
            passed,
        });

        if !passed {
            break;
        }
    }

    Ok(PlayedQuest { outcomes })
}

pub fn build_quest(description: String, difficulty: u8) -> QuestData {
    let mut rng = rand::thread_rng();
    let trial_count = roll_trial_count(difficulty, &mut rng);
    let trials = (0..trial_count).map(|_| roll_stats(difficulty, &mut rng)).collect();
    QuestData {
        quest_description: description,
        quest_difficulty: difficulty,
        trials,
    }
}
