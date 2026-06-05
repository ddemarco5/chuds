use rand::Rng;

use crate::game::domain::player::Player;
use crate::game::domain::quest::TrialStats;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData};
use crate::game::mechanics::quest_builder::{PlayedQuest, StatChoice, TrialOutcome};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::tuneable_rolls::{EXP_DROPOUT_MAX, EXP_DROPOUT_MIN};

const MIN_STAT: u8 = 1;
const MAX_STAT: u8 = 10;

// P(optimal stat chosen) by experience level.
// dropout_prob = EXP_DROPOUT_MIN + ((exp - 1) / 9) * (EXP_DROPOUT_MAX - EXP_DROPOUT_MIN)
// Non-optimal valid stats are each dropped independently with `dropout_prob`; the optimal stat
// is always kept. The surviving pool is then sampled uniformly at random.
// Closed forms (d = dropout_prob): 2 valid → (1 + d) / 2,  3 valid → (1 + d + d²) / 3
//
// | Exp | dropout | 2 valid stats | 3 valid stats |
// |-----|---------|---------------|---------------|
// |   1 |   5.0%  |       52.5%   |       35.1%   |
// |   2 |  12.2%  |       56.1%   |       37.9%   |
// |   3 |  19.4%  |       59.7%   |       41.1%   |
// |   4 |  26.7%  |       63.3%   |       44.6%   |
// |   5 |  33.9%  |       66.9%   |       48.5%   |
// |   6 |  41.1%  |       70.6%   |       52.7%   |
// |   7 |  48.3%  |       74.2%   |       57.2%   |
// |   8 |  55.6%  |       77.8%   |       62.1%   |
// |   9 |  62.8%  |       81.4%   |       67.4%   |
// |  10 |  70.0%  |       85.0%   |       73.0%   |

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
