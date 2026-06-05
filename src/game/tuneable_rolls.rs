use rand::Rng;
use rand_distr::{Distribution, Normal};

use crate::game::domain::item::{stat_contribution, ItemSeed, ItemStats, ItemType};
use crate::game::domain::quest::TrialStats;

// ── Quest reward ──────────────────────────────────────────────────────────────

const REWARD_QUADRATIC: f64 = 0.3;
const REWARD_LINEAR: f64 = 3.0;
/// Random spread around the difficulty-derived base reward: final gold = base × Normal(1.0, σ).
const REWARD_JITTER_STDDEV: f64 = 0.10;

// ── Quest trial generation ────────────────────────────────────────────────────

/// Standard deviation for the trial-count normal distribution. Tweak to taste.
pub const TRIAL_COUNT_STDDEV: f64 = 1.0;
const MIN_TRIALS: usize = 1;
/// Probability (0.0-1.0) that any given stat is irrelevant (set to 0) for a trial.
const STAT_ZERO_CHANCE: f64 = 0.10;

// ── Quest simulation ──────────────────────────────────────────────────────────

/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at minimum experience (1).
pub const EXP_DROPOUT_MIN: f64 = 0.05;
/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at maximum experience (10).
pub const EXP_DROPOUT_MAX: f64 = 0.70;

// ── Item drops & stat rolls ───────────────────────────────────────────────────

pub const ITEM_DROP_CHANCE: f64 = 0.10;
/// Probability a rolled item stat is neutral ("-").
const ITEM_STAT_NEUTRAL_CHANCE: f64 = 0.50;
/// Upper bound of the modifier band (neutral + modifier spans [NEUTRAL, MODIFIER)).
const ITEM_STAT_MODIFIER_CHANCE: f64 = 0.85;
/// When forcing a positive stat, probability it is a signed modifier (+N) vs a floor (N).
const ITEM_STAT_POSITIVE_MODIFIER_CHANCE: f64 = 0.70;
/// Within the modifier band, probability the modifier is positive (+N) vs negative (-N).
const ITEM_STAT_SIGN_CHANCE: f64 = 0.50;

const GEAR_SUBTYPES: &[&str] = &["helmet", "chest", "legs", "feet", "hands"];
const MISC_SUBTYPES: &[&str] = &["trinket"]; // we want to add consumables and others in the future

// ── Item gold value ───────────────────────────────────────────────────────────

/// Net stat value at which base gold should approximate `ITEM_VALUE_ANCHOR_GOLD_LOW`.
const ITEM_VALUE_ANCHOR_STAT_LOW: f64 = 3.0;
const ITEM_VALUE_ANCHOR_GOLD_LOW: f64 = 100.0;
/// Net stat value at which base gold should approximate `ITEM_VALUE_ANCHOR_GOLD_HIGH`.
const ITEM_VALUE_ANCHOR_STAT_HIGH: f64 = 8.0;
const ITEM_VALUE_ANCHOR_GOLD_HIGH: f64 = 1000.0;
/// Random spread around the stat-derived base price: final gold = base × Normal(1.0, σ).
/// Raise σ for wider swings (same stats can roll noticeably higher or lower); lower σ tightens
/// prices toward the anchor curve; 0 removes jitter entirely.
const ITEM_VALUE_JITTER_STDDEV: f64 = 0.15;

// ── Hospital pricing ──────────────────────────────────────────────────────────

/// Base heal price per tick of recovery. Increase to make hospital stays more expensive.
const HEAL_PRICE_PER_TICK_BASE: f64 = 5.0;
/// Price jitter: multiplier sampled from Normal(mean=1.0, stddev=HEAL_PRICE_JITTER_STDDEV).
const HEAL_PRICE_JITTER_STDDEV: f64 = 0.20;

// ── Chud creation ─────────────────────────────────────────────────────────────

/// Each starting stat is rolled in 1..=CHUD_STAT_ROLL_MAX, then trimmed until sum ≤ cap.
const CHUD_STAT_ROLL_MAX: u8 = 3;
const CHUD_STAT_TOTAL_CAP: u8 = 5;

// ── Shared RNG helpers ────────────────────────────────────────────────────────

fn sample_unit_jitter(rng: &mut impl Rng, stddev: f64) -> f64 {
    let normal = Normal::new(1.0, stddev).expect("valid normal distribution");
    normal.sample(rng).max(0.0)
}

fn sample_normal_rounded(rng: &mut impl Rng, mean: f64, stddev: f64, min: i64) -> usize {
    let normal = Normal::new(mean, stddev).expect("valid normal distribution");
    let sample = normal.sample(rng).round() as i64;
    sample.max(min) as usize
}

// ── Quest reward rolls ────────────────────────────────────────────────────────

pub fn calculate_quest_reward(trials: &[TrialStats]) -> u32 {
    let cumulative: f64 = trials
        .iter()
        .map(|t| {
            let vals = [t.strength, t.smarts, t.stealth];
            let nonzero: Vec<f64> = vals
                .iter()
                .filter(|&&v| v != 0)
                .map(|&v| v as f64)
                .collect();
            if nonzero.is_empty() {
                0.0
            } else {
                nonzero.iter().sum::<f64>() / nonzero.len() as f64
            }
        })
        .sum();
    let base = REWARD_QUADRATIC * cumulative * cumulative + REWARD_LINEAR * cumulative;
    let mut rng = rand::thread_rng();
    let multiplier = sample_unit_jitter(&mut rng, REWARD_JITTER_STDDEV);
    (base * multiplier).round() as u32
}

// ── Quest trial rolls ─────────────────────────────────────────────────────────

pub fn roll_trial_count(difficulty: u8, rng: &mut impl Rng) -> usize {
    let mean = difficulty as f64 / 2.0;
    sample_normal_rounded(rng, mean, TRIAL_COUNT_STDDEV, MIN_TRIALS as i64)
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

pub fn roll_trials(difficulty: u8) -> Vec<TrialStats> {
    let mut rng = rand::thread_rng();
    let trial_count = roll_trial_count(difficulty, &mut rng);
    (0..trial_count).map(|_| roll_stats(difficulty, &mut rng)).collect()
}

// ── Item rolls ────────────────────────────────────────────────────────────────

pub fn roll_item_drop(rng: &mut impl Rng) -> bool {
    rng.gen_bool(ITEM_DROP_CHANCE)
}

pub fn roll_item(difficulty: u8, rng: &mut impl Rng) -> ItemSeed {
    let item_type = roll_item_type(rng);
    let subtype = roll_subtype(item_type, rng);
    let stats = roll_item_stats(difficulty, rng);
    ItemSeed {
        item_type,
        subtype,
        stats,
        trigger: None,
        effect: None,
    }
}

fn roll_item_type(rng: &mut impl Rng) -> ItemType {
    match rng.gen_range(0..3) {
        0 => ItemType::Gear,
        1 => ItemType::Weapon,
        _ => ItemType::Misc,
    }
}

fn roll_subtype(item_type: ItemType, rng: &mut impl Rng) -> String {
    match item_type {
        ItemType::Gear => GEAR_SUBTYPES[rng.gen_range(0..GEAR_SUBTYPES.len())].to_string(),
        ItemType::Weapon => String::new(),
        ItemType::Misc => MISC_SUBTYPES[rng.gen_range(0..MISC_SUBTYPES.len())].to_string(),
    }
}

fn roll_item_stats(difficulty: u8, rng: &mut impl Rng) -> ItemStats {
    let cap = (difficulty / 2).max(1);
    let mut stats = ItemStats {
        strength: roll_single_stat(cap, rng),
        smarts: roll_single_stat(cap, rng),
        stealth: roll_single_stat(cap, rng),
    };
    if !has_positive_stat(&stats) {
        let positive = roll_positive_stat(cap, rng);
        match rng.gen_range(0..3) {
            0 => stats.strength = positive,
            1 => stats.smarts = positive,
            _ => stats.stealth = positive,
        }
    }
    stats
}

fn has_positive_stat(stats: &ItemStats) -> bool {
    stat_contribution(&stats.strength) > 0
        || stat_contribution(&stats.smarts) > 0
        || stat_contribution(&stats.stealth) > 0
}

fn roll_positive_stat(cap: u8, rng: &mut impl Rng) -> String {
    if rng.gen_bool(ITEM_STAT_POSITIVE_MODIFIER_CHANCE) {
        format!("+{}", rng.gen_range(1..=cap))
    } else {
        rng.gen_range(1..=cap).to_string()
    }
}

fn roll_single_stat(cap: u8, rng: &mut impl Rng) -> String {
    let roll = rng.gen_range(0.0..1.0);
    if roll < ITEM_STAT_NEUTRAL_CHANCE {
        "-".to_string()
    } else if roll < ITEM_STAT_MODIFIER_CHANCE {
        let magnitude = rng.gen_range(1..=cap);
        if rng.gen_bool(ITEM_STAT_SIGN_CHANCE) {
            format!("+{magnitude}")
        } else {
            format!("-{magnitude}")
        }
    } else {
        rng.gen_range(1..=cap).to_string()
    }
}

/// Roll gold value from stats: exponential base from net stat value × normal jitter.
pub fn roll_item_value(stats: &ItemStats, rng: &mut impl Rng) -> u32 {
    let net = stats.net_stat_value() as f64;
    let rate = (ITEM_VALUE_ANCHOR_GOLD_HIGH / ITEM_VALUE_ANCHOR_GOLD_LOW)
        .powf(1.0 / (ITEM_VALUE_ANCHOR_STAT_HIGH - ITEM_VALUE_ANCHOR_STAT_LOW));
    let base_coeff = ITEM_VALUE_ANCHOR_GOLD_LOW / rate.powf(ITEM_VALUE_ANCHOR_STAT_LOW);
    let base = base_coeff * rate.powf(net);
    let multiplier = sample_unit_jitter(rng, ITEM_VALUE_JITTER_STDDEV);
    (base * multiplier).round() as u32
}

// ── Hospital pricing rolls ────────────────────────────────────────────────────

pub fn roll_heal_price(ticks: u32, rng: &mut impl Rng) -> u32 {
    let base_price = HEAL_PRICE_PER_TICK_BASE * ticks as f64;
    let multiplier = sample_unit_jitter(rng, HEAL_PRICE_JITTER_STDDEV);
    (base_price * multiplier).round() as u32
}

// ── Chud creation rolls ───────────────────────────────────────────────────────

pub fn roll_chud_stats(rng: &mut impl Rng) -> (u8, u8, u8) {
    let mut strength = rng.gen_range(1u8..=CHUD_STAT_ROLL_MAX);
    let mut smarts   = rng.gen_range(1u8..=CHUD_STAT_ROLL_MAX);
    let mut stealth  = rng.gen_range(1u8..=CHUD_STAT_ROLL_MAX);

    while strength + smarts + stealth > CHUD_STAT_TOTAL_CAP {
        if strength >= smarts && strength >= stealth {
            strength -= 1;
        } else if smarts >= stealth {
            smarts -= 1;
        } else {
            stealth -= 1;
        }
    }

    (strength, smarts, stealth)
}
