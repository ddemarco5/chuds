use rand::Rng;

use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::player::Player;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData, TrialStats};
use crate::game::mechanics::quest_builder::{PlayedQuest, StatChoice, TrialOutcome};

const MIN_STAT: u8 = 1;
const MAX_STAT: u8 = 10;

/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at maximum experience (10).
const EXP_DROPOUT_MAX: f64 = 0.70;
/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at minimum experience (1).
const EXP_DROPOUT_MIN: f64 = 0.05;

pub fn effective_stats(player: &Player, registry: &ItemRegistry) -> (u8, u8, u8) {
    let mut strength = player.strength as i16;
    let mut smarts = player.smarts as i16;
    let mut stealth = player.stealth as i16;

    for id in player.chud.equipment.all_ids() {
        if let Some(item) = registry.get(id) {
            let (a, b, c) = item.stats.total_modifier();
            strength += a;
            smarts += b;
            stealth += c;
        }
    }

    (
        strength.clamp(MIN_STAT as i16, MAX_STAT as i16) as u8,
        smarts.clamp(MIN_STAT as i16, MAX_STAT as i16) as u8,
        stealth.clamp(MIN_STAT as i16, MAX_STAT as i16) as u8,
    )
}

fn roll_aids(player: &Player, registry: &ItemRegistry, stat: StatChoice) -> (u8, i16) {
    let mut floor = 0u8;
    let mut modifier = 0i16;

    for id in player.chud.equipment.all_ids() {
        if let Some(item) = registry.get(id) {
            let (f_str, f_smt, f_sth) = item.stats.total_floor();
            let (m_str, m_smt, m_sth) = item.stats.total_modifier();
            match stat {
                StatChoice::Strength => {
                    floor = floor.saturating_add(f_str);
                    modifier += m_str;
                }
                StatChoice::Smarts => {
                    floor = floor.saturating_add(f_smt);
                    modifier += m_smt;
                }
                StatChoice::Stealth => {
                    floor = floor.saturating_add(f_sth);
                    modifier += m_sth;
                }
            }
        }
    }

    (floor, modifier)
}

fn effective_stat(player: &Player, registry: &ItemRegistry, stat: StatChoice) -> u8 {
    let (str, smt, sth) = effective_stats(player, registry);
    match stat {
        StatChoice::Strength => str,
        StatChoice::Smarts => smt,
        StatChoice::Stealth => sth,
    }
}

fn choose_stat(
    stats: &TrialStats,
    player: &Player,
    registry: &ItemRegistry,
    rng: &mut impl Rng,
) -> StatChoice {
    let candidates = [
        (StatChoice::Strength, stats.strength, effective_stat(player, registry, StatChoice::Strength)),
        (StatChoice::Smarts, stats.smarts, effective_stat(player, registry, StatChoice::Smarts)),
        (StatChoice::Stealth, stats.stealth, effective_stat(player, registry, StatChoice::Stealth)),
    ];

    let valid: Vec<(StatChoice, i16)> = candidates
        .iter()
        .filter(|&&(_, req, _)| req > 0)
        .map(|&(s, req, ps)| (s, ps as i16 - req as i16))
        .collect();

    let best_margin = valid.iter().map(|&(_, m)| m).max().unwrap_or(0);

    let t = (player.experience - 1) as f64 / 9.0;
    let dropout_prob = EXP_DROPOUT_MIN + t * (EXP_DROPOUT_MAX - EXP_DROPOUT_MIN);

    let considered: Vec<StatChoice> = valid
        .iter()
        .filter(|&&(_, m)| m == best_margin || !rng.gen_bool(dropout_prob))
        .map(|&(s, _)| s)
        .collect();

    let pool = if considered.is_empty() {
        valid.iter().map(|&(s, _)| s).collect()
    } else {
        considered
    };
    pool[rng.gen_range(0..pool.len())]
}

pub fn try_quest(
    quest: &QuestData,
    generated: &GeneratedQuest,
    player: &Player,
    registry: &ItemRegistry,
) -> anyhow::Result<PlayedQuest> {
    let mut rng = rand::thread_rng();
    let mut outcomes = Vec::new();

    for (stats, _situation) in quest.trials.iter().zip(generated.trials.iter()) {
        let stat_used = choose_stat(stats, player, registry, &mut rng);
        let effective = effective_stat(player, registry, stat_used);
        let required = stat_used.trial_required(stats);
        let (floor, modifier) = roll_aids(player, registry, stat_used);
        let base = stat_used.player_stat(player);
        let raw_roll = rng.gen_range(1..=base);
        let after_modifier = (raw_roll as i16 + modifier).max(1) as u8;
        let player_roll = after_modifier.max(floor);
        let floor_applied = modifier == 0 && player_roll > after_modifier;
        let trial_roll: u8 = rng.gen_range(1..=required + 1);
        let passed = player_roll >= trial_roll;

        outcomes.push(TrialOutcome {
            stats: stats.clone(),
            stat_used,
            raw_roll,
            roll_modifier: modifier,
            floor_applied,
            player_roll,
            trial_roll,
            player_stat: effective,
            required,
            passed,
        });

        if !passed {
            break;
        }
    }

    Ok(PlayedQuest { outcomes })
}

pub fn play_quest(
    quest: &QuestData,
    generated: &GeneratedQuest,
    player: &Player,
    registry: &ItemRegistry,
) -> anyhow::Result<PlayedQuest> {
    try_quest(quest, generated, player, registry)
}

/// Run `trials` simulations of the quest for `player` and return the fraction
/// of runs in which the player succeeded (0.0 = never, 1.0 = always).
pub fn check_job(
    quest: &QuestData,
    generated: &GeneratedQuest,
    player: &Player,
    registry: &ItemRegistry,
    trials: u32,
) -> anyhow::Result<f64> {
    let count = trials.max(1);
    let mut successes = 0u32;
    for _ in 0..count {
        let played = try_quest(quest, generated, player, registry)?;
        if played.outcomes.last().map_or(false, |o| o.passed) {
            successes += 1;
        }
    }
    Ok(successes as f64 / count as f64)
}
