use rand::{Rng, RngExt};
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};

use crate::game::domain::item::{ItemSeed, ItemStats, ItemType};
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

/// Spread of auto-generated job difficulty around the mean top-two effective stat level of all chuds.
/// Difficulty is sampled from Normal(mean, this), rounded, and clamped to 1..=10.
/// Deliberately broad so easy and dangerous outliers both appear: raise it for an even
/// wider mix of trivial-to-brutal jobs, lower it to cluster difficulty tightly on the
/// average player's power level.
pub const AUTO_JOB_DIFFICULTY_STDDEV: f64 = 1.0;
/// Additive bias applied to the sampled difficulty before rounding. Raise to skew auto jobs
/// slightly harder than the computed mean; lower (negative) to ease them.
pub const AUTO_JOB_DIFFICULTY_TRAJ: f64 = 0.5;
/// Probability (0.0-1.0) that any given stat is irrelevant (set to 0) for a trial.
const STAT_ZERO_CHANCE: f64 = 0.10;

// ── Quest simulation ──────────────────────────────────────────────────────────

/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at minimum experience (1).
pub const EXP_DROPOUT_MIN: f64 = 0.05;
/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at maximum experience (10).
pub const EXP_DROPOUT_MAX: f64 = 0.70;

// ── Item drops & stat rolls ───────────────────────────────────────────────────

/// Flat bonus added to drop chance for each quest trial (0.04 = +4% per trial).
/// Raise to reward longer jobs; lower to make trial count matter less. Applies equally at all difficulties.
const ITEM_DROP_PER_TRIAL: f64 = 0.04;
/// Base item drop chance at difficulty 1, before the per-trial bonus is added.
/// Raise to increase drops on easy jobs; lower to tighten loot across the board.
/// Harder jobs scale down linearly to 0% base at ITEM_DROP_BASE_DIFF_HIGH.
const ITEM_DROP_BASE_AT_DIFF_1: f64 = 0.30;
/// Endpoints for linear base scaling: difficulty 1 uses full base, difficulty 9 uses zero base.
/// Extending DIFF_HIGH lowers drops on mid-tier jobs; lowering DIFF_LOW shifts where the curve starts.
const ITEM_DROP_BASE_DIFF_LOW: u8 = 1;
const ITEM_DROP_BASE_DIFF_HIGH: u8 = 9;
/// Stddev for the item stat-cap normal distribution. Mean is difficulty / 2.
pub const ITEM_STAT_CAP_STDDEV: f64 = 1.0;
/// Stddev for target net stat budget jitter around difficulty / 2.
const ITEM_TARGET_NET_JITTER_STDDEV: f64 = 0.5;
/// Minimum net stat total for each rarity tier (used to derive rarity and story-reward floors).
const ITEM_RARITY_MIN_NET_COMMON: u8 = 1;
const ITEM_RARITY_MIN_NET_UNCOMMON: u8 = 2;
const ITEM_RARITY_MIN_NET_RARE: u8 = 4;
const ITEM_RARITY_MIN_NET_EXCEPTIONAL: u8 = 6;
/// Probability a positive allocation is a roll floor (N) vs a signed modifier (+N).
const ITEM_STAT_FLOOR_CHANCE: f64 = 0.30;
/// When two positive stats strictly exceed the target net, probability of adding a curse (-N).
const ITEM_CURSE_CHANCE: f64 = 0.40;

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
/// Extra price multiplier added on top of stat-derived base (0.10 → +10% gold).
const ITEM_RARITY_PRICE_BONUS_UNCOMMON: f64 = 0.10;
const ITEM_RARITY_PRICE_BONUS_RARE: f64 = 0.20;
const ITEM_RARITY_PRICE_BONUS_EXCEPTIONAL: f64 = 0.30;

// ── Hospital pricing ──────────────────────────────────────────────────────────

/// Base heal price per tick of recovery. Increase to make hospital stays more expensive.
const HEAL_PRICE_PER_TICK_BASE: f64 = 5.0;
/// Price jitter: multiplier sampled from Normal(mean=1.0, stddev=HEAL_PRICE_JITTER_STDDEV).
const HEAL_PRICE_JITTER_STDDEV: f64 = 0.20;
/// Injury chance per point of negative margin (0.10 → margin -3 = 30%).
/// Raise to hospitalize more often; lower to make injuries rarer at small margins.
const INJURY_CHANCE_PER_MARGIN: f64 = 0.10;
/// Stddev for hospital stay length: days ~ Normal(abs(margin), σ), rounded, min 1.
/// Raise for wider stay-length swings; lower to keep days close to abs(margin).
const INJURY_HOSPITAL_STDDEV: f64 = 1.0;

// ── Death checks ──────────────────────────────────────────────────────────
/// First margin that can roll for death. Margins above this (e.g. -5) never kill.
const DEATH_MARGIN_THRESHOLD: i16 = -6;
/// Margins at or beyond this are always lethal.
const DEATH_MARGIN_GUARANTEED: i16 = -10;
/// Death chance at margin -6. Raise to make near-catastrophic failures deadlier.
const DEATH_CHANCE_AT_MARGIN_6: f64 = 0.01;
/// Death chance at margin -7.
const DEATH_CHANCE_AT_MARGIN_7: f64 = 0.05;
/// Death chance at margin -8.
const DEATH_CHANCE_AT_MARGIN_8: f64 = 0.20;
/// Death chance at margin -9.
const DEATH_CHANCE_AT_MARGIN_9: f64 = 0.70;

// ── Shared RNG helpers ────────────────────────────────────────────────────────

fn sample_unit_jitter(rng: &mut (impl Rng + ?Sized), stddev: f64) -> f64 {
    let normal = Normal::new(1.0, stddev).expect("valid normal distribution");
    normal.sample(rng).max(0.0)
}

fn sample_normal_rounded(rng: &mut (impl Rng + ?Sized), mean: f64, stddev: f64, min: i64) -> usize {
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
    let mut rng = rand::rng();
    let multiplier = sample_unit_jitter(&mut rng, REWARD_JITTER_STDDEV);
    (base * multiplier).round() as u32
}

// ── Quest trial rolls ─────────────────────────────────────────────────────────

pub fn roll_trial_count(difficulty: u8, rng: &mut (impl Rng + ?Sized)) -> usize {
    let mean = difficulty as f64 / 2.0;
    sample_normal_rounded(rng, mean, TRIAL_COUNT_STDDEV, MIN_TRIALS as i64)
}

pub fn roll_stats(difficulty: u8, rng: &mut (impl Rng + ?Sized)) -> TrialStats {
    let floor = difficulty.max(1);
    let str_val = if rng.random_bool(STAT_ZERO_CHANCE) { 0 } else { rng.random_range(1..=floor) };
    let smt_val = if rng.random_bool(STAT_ZERO_CHANCE) { 0 } else { rng.random_range(1..=floor) };
    let sth_val = if rng.random_bool(STAT_ZERO_CHANCE) { 0 } else { rng.random_range(1..=floor) };
    let mut stats = TrialStats { strength: str_val, smarts: smt_val, stealth: sth_val };
    if stats.strength == 0 && stats.smarts == 0 && stats.stealth == 0 {
        match rng.random_range(0..3u8) {
            0 => stats.strength = rng.random_range(1..=floor),
            1 => stats.smarts   = rng.random_range(1..=floor),
            _ => stats.stealth  = rng.random_range(1..=floor),
        }
    }
    stats
}

pub fn roll_trials(difficulty: u8) -> Vec<TrialStats> {
    let mut rng = rand::rng();
    let trial_count = roll_trial_count(difficulty, &mut rng);
    (0..trial_count).map(|_| roll_stats(difficulty, &mut rng)).collect()
}

/// Difficulty (1..=10) for an auto-generated job: a broad normal roll centered on the
/// mean top-two effective stat level of all chuds (base + item modifiers), so jobs mildly
/// favor the average player's power while still spawning easier and harder outliers.
pub fn roll_auto_job_difficulty(mean_stat: f64, rng: &mut (impl Rng + ?Sized)) -> u8 {
    let normal = Normal::new(mean_stat, AUTO_JOB_DIFFICULTY_STDDEV)
        .expect("valid normal distribution");
    let sample = (normal.sample(rng) + AUTO_JOB_DIFFICULTY_TRAJ).round();
    sample.clamp(1.0, 10.0) as u8
}

// ── Item rolls ────────────────────────────────────────────────────────────────

fn item_drop_base_chance(difficulty: u8) -> f64 {
    let d = difficulty.clamp(ITEM_DROP_BASE_DIFF_LOW, ITEM_DROP_BASE_DIFF_HIGH) as f64;
    let span = (ITEM_DROP_BASE_DIFF_HIGH - ITEM_DROP_BASE_DIFF_LOW) as f64;
    ITEM_DROP_BASE_AT_DIFF_1 * (ITEM_DROP_BASE_DIFF_HIGH as f64 - d) / span
}

pub fn item_drop_chance(difficulty: u8, trial_count: usize) -> f64 {
    let base = item_drop_base_chance(difficulty);
    (base + trial_count as f64 * ITEM_DROP_PER_TRIAL).clamp(0.0, 1.0)
}

pub fn roll_item_drop(difficulty: u8, trial_count: usize, rng: &mut (impl Rng + ?Sized)) -> bool {
    rng.random_bool(item_drop_chance(difficulty, trial_count))
}

pub fn roll_item(
    difficulty: u8,
    rng: &mut (impl Rng + ?Sized),
    rarity_override: Option<&str>,
    stats_override: Option<&ItemStats>,
) -> ItemSeed {
    let item_type = roll_item_type(rng);
    let subtype = roll_subtype(item_type, rng);
    let (stats, rarity) = if let Some(stats) = stats_override {
        (stats.clone(), rarity_override.unwrap_or("common").to_string())
    } else {
        roll_item_stats(difficulty, rarity_override, rng)
    };
    ItemSeed {
        item_type,
        subtype,
        stats,
        rarity,
        trigger: None,
        effect: None,
    }
}

fn roll_item_type(rng: &mut (impl Rng + ?Sized)) -> ItemType {
    match rng.random_range(0..3) {
        0 => ItemType::Gear,
        1 => ItemType::Weapon,
        _ => ItemType::Misc,
    }
}

fn roll_subtype(item_type: ItemType, rng: &mut (impl Rng + ?Sized)) -> String {
    match item_type {
        ItemType::Gear => GEAR_SUBTYPES[rng.random_range(0..GEAR_SUBTYPES.len())].to_string(),
        ItemType::Weapon => String::new(),
        ItemType::Misc => MISC_SUBTYPES[rng.random_range(0..MISC_SUBTYPES.len())].to_string(),
    }
}

fn roll_item_cap(difficulty: u8, rng: &mut (impl Rng + ?Sized)) -> u8 {
    let mean = difficulty as f64 / 2.0;
    let normal = Normal::new(mean, ITEM_STAT_CAP_STDDEV).expect("valid normal distribution");
    normal.sample(rng).round().max(1.0) as u8
}

fn rarity_min_net(rarity: &str) -> u8 {
    let r = rarity.to_ascii_lowercase();
    match r.as_str() {
        "exceptional" => ITEM_RARITY_MIN_NET_EXCEPTIONAL,
        "rare" => ITEM_RARITY_MIN_NET_RARE,
        "uncommon" => ITEM_RARITY_MIN_NET_UNCOMMON,
        _ => ITEM_RARITY_MIN_NET_COMMON,
    }
}

fn rarity_price_bonus(rarity: &str) -> f64 {
    let r = rarity.to_ascii_lowercase();
    match r.as_str() {
        "exceptional" => ITEM_RARITY_PRICE_BONUS_EXCEPTIONAL,
        "rare" => ITEM_RARITY_PRICE_BONUS_RARE,
        "uncommon" => ITEM_RARITY_PRICE_BONUS_UNCOMMON,
        _ => 0.0,
    }
}

fn rarity_from_net(net: u8) -> String {
    if net >= ITEM_RARITY_MIN_NET_EXCEPTIONAL {
        "exceptional".to_string()
    } else if net >= ITEM_RARITY_MIN_NET_RARE {
        "rare".to_string()
    } else if net >= ITEM_RARITY_MIN_NET_UNCOMMON {
        "uncommon".to_string()
    } else {
        "common".to_string()
    }
}

fn roll_target_net(difficulty: u8, cap: u8, min_net: u8, rng: &mut (impl Rng + ?Sized)) -> u8 {
    let mean = difficulty as f64 / 2.0;
    let normal =
        Normal::new(mean, ITEM_TARGET_NET_JITTER_STDDEV).expect("valid normal distribution");
    let sample = normal.sample(rng).round() as i64;
    let max_net = ((3 * cap as u32).min(15) as i64).max(min_net as i64);
    sample.clamp(min_net as i64, max_net) as u8
}

fn format_positive_stat(magnitude: u8, rng: &mut (impl Rng + ?Sized)) -> String {
    if rng.random_bool(ITEM_STAT_FLOOR_CHANCE) {
        magnitude.to_string()
    } else {
        format!("+{magnitude}")
    }
}

fn try_allocate_with_curse(target_net: u8, cap: u8, rng: &mut (impl Rng + ?Sized)) -> Option<ItemStats> {
    let lo = target_net as u32 + 1;
    let hi = 2 * cap as u32;
    if lo > hi {
        return None;
    }
    let sum_ab = rng.random_range(lo..=hi);
    let a_min = sum_ab.saturating_sub(cap as u32).max(1);
    let a_max = (sum_ab - 1).min(cap as u32);
    if a_min > a_max {
        return None;
    }
    let a = rng.random_range(a_min..=a_max) as u8;
    let b = (sum_ab - a as u32) as u8;
    let curse = (sum_ab - target_net as u32) as u8;
    if curse == 0 || curse > cap {
        return None;
    }

    let (pos_a, pos_b, curse_idx) = match rng.random_range(0..6) {
        0 => (0, 1, 2),
        1 => (0, 2, 1),
        2 => (1, 0, 2),
        3 => (1, 2, 0),
        4 => (2, 0, 1),
        _ => (2, 1, 0),
    };
    let mut values = ["-".to_string(), "-".to_string(), "-".to_string()];
    values[pos_a] = format_positive_stat(a, rng);
    values[pos_b] = format_positive_stat(b, rng);
    values[curse_idx] = format!("-{curse}");

    Some(ItemStats {
        strength: values[0].clone(),
        smarts: values[1].clone(),
        stealth: values[2].clone(),
    })
}

fn allocate_item_stats(target_net: u8, cap: u8, rng: &mut (impl Rng + ?Sized)) -> ItemStats {
    let curse_possible = (target_net as u32 + 1) <= 2 * cap as u32;
    if curse_possible && rng.random_bool(ITEM_CURSE_CHANCE) {
        if let Some(stats) = try_allocate_with_curse(target_net, cap, rng) {
            return stats;
        }
    }

    let mut slots = [0u8; 3];
    let mut remaining = target_net;
    while remaining > 0 {
        let available: Vec<usize> = (0..3).filter(|&i| slots[i] < cap).collect();
        if available.is_empty() {
            break;
        }
        let unused: Vec<usize> = available
            .iter()
            .copied()
            .filter(|&i| slots[i] == 0)
            .collect();
        let idx = if !unused.is_empty() {
            unused[rng.random_range(0..unused.len())]
        } else {
            available[rng.random_range(0..available.len())]
        };
        let room = (cap - slots[idx]).min(remaining);
        let chunk = if room == 1 {
            1
        } else {
            rng.random_range(1..=room)
        };
        slots[idx] += chunk;
        remaining -= chunk;
    }

    let mut values = ["-".to_string(), "-".to_string(), "-".to_string()];
    for (i, &mag) in slots.iter().enumerate() {
        if mag > 0 {
            values[i] = format_positive_stat(mag, rng);
        }
    }

    ItemStats {
        strength: values[0].clone(),
        smarts: values[1].clone(),
        stealth: values[2].clone(),
    }
}

fn roll_item_stats(
    difficulty: u8,
    rarity_floor: Option<&str>,
    rng: &mut (impl Rng + ?Sized),
) -> (ItemStats, String) {
    let mut cap = roll_item_cap(difficulty, rng);
    let min_net = rarity_floor
        .map(rarity_min_net)
        .unwrap_or(ITEM_RARITY_MIN_NET_COMMON);
    // Ensure cap can fit the minimum net budget across three stat slots.
    cap = cap.max(min_net.div_ceil(3));
    let target_net = roll_target_net(difficulty, cap, min_net, rng);
    let stats = allocate_item_stats(target_net, cap, rng);
    let rarity = rarity_floor
        .map(str::to_string)
        .unwrap_or_else(|| rarity_from_net(target_net));
    (stats, rarity)
}

/// Roll gold value from stats and rarity: exponential base from net stat value × rarity bonus × normal jitter.
pub fn roll_item_value(stats: &ItemStats, rarity: &str, rng: &mut (impl Rng + ?Sized)) -> u32 {
    let net = stats.net_stat_value() as f64;
    let rate = (ITEM_VALUE_ANCHOR_GOLD_HIGH / ITEM_VALUE_ANCHOR_GOLD_LOW)
        .powf(1.0 / (ITEM_VALUE_ANCHOR_STAT_HIGH - ITEM_VALUE_ANCHOR_STAT_LOW));
    let base_coeff = ITEM_VALUE_ANCHOR_GOLD_LOW / rate.powf(ITEM_VALUE_ANCHOR_STAT_LOW);
    let base = base_coeff * rate.powf(net);
    let rarity_mult = 1.0 + rarity_price_bonus(rarity);
    let multiplier = sample_unit_jitter(rng, ITEM_VALUE_JITTER_STDDEV);
    (base * rarity_mult * multiplier).round() as u32
}

// ── Hospital pricing rolls ────────────────────────────────────────────────────

pub fn roll_heal_price(ticks: u32, rng: &mut (impl Rng + ?Sized)) -> u32 {
    let base_price = HEAL_PRICE_PER_TICK_BASE * ticks as f64;
    let multiplier = sample_unit_jitter(rng, HEAL_PRICE_JITTER_STDDEV);
    (base_price * multiplier).round() as u32
}

pub fn injury_chance(margin: i16) -> f64 {
    if margin >= 0 {
        0.0
    } else {
        (margin.unsigned_abs() as f64 * INJURY_CHANCE_PER_MARGIN).min(1.0)
    }
}

pub fn roll_injury(margin: i16, rng: &mut (impl Rng + ?Sized)) -> bool {
    rng.random_bool(injury_chance(margin))
}

pub fn roll_hospital_ticks(margin: i16, rng: &mut (impl Rng + ?Sized)) -> u32 {
    let mean = margin.unsigned_abs() as f64;
    sample_normal_rounded(rng, mean, INJURY_HOSPITAL_STDDEV, 1) as u32
}

/// Returns `Some(ticks)` if the injury roll succeeds, else `None`.
pub fn roll_injury_outcome(margin: i16, rng: &mut (impl Rng + ?Sized)) -> Option<u32> {
    if margin >= 0 || !roll_injury(margin, rng) {
        None
    } else {
        Some(roll_hospital_ticks(margin, rng))
    }
}

pub fn death_chance(margin: i16) -> f64 {
    if margin > DEATH_MARGIN_THRESHOLD {
        0.0
    } else if margin <= DEATH_MARGIN_GUARANTEED {
        1.0
    } else {
        match margin {
            -6 => DEATH_CHANCE_AT_MARGIN_6,
            -7 => DEATH_CHANCE_AT_MARGIN_7,
            -8 => DEATH_CHANCE_AT_MARGIN_8,
            -9 => DEATH_CHANCE_AT_MARGIN_9,
            _ => 1.0,
        }
    }
}

pub fn roll_death(margin: i16, rng: &mut (impl Rng + ?Sized)) -> bool {
    let chance = death_chance(margin);
    chance > 0.0 && rng.random_bool(chance)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureConsequences {
    pub died: bool,
    pub hospital_ticks: Option<u32>,
}

/// Roll death then injury for a failed quest's last-trial margin. Death takes priority.
pub fn roll_failure_consequences(margin: i16, rng: &mut (impl Rng + ?Sized)) -> FailureConsequences {
    if margin >= 0 {
        return FailureConsequences {
            died: false,
            hospital_ticks: None,
        };
    }
    if roll_death(margin, rng) {
        return FailureConsequences {
            died: true,
            hospital_ticks: None,
        };
    }
    FailureConsequences {
        died: false,
        hospital_ticks: roll_injury_outcome(margin, rng),
    }
}
