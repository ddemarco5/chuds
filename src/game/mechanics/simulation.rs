use rand::Rng;

use crate::game::domain::player::Player;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData, TrialStats};
use crate::game::mechanics::quest_builder::{PlayedQuest, StatChoice, TrialOutcome};

/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at maximum experience (10).
const EXP_DROPOUT_MAX: f64 = 0.70;
/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at minimum experience (1).
const EXP_DROPOUT_MIN: f64 = 0.05;

fn choose_stat(stats: &TrialStats, player: &Player, rng: &mut impl Rng) -> StatChoice {
    let candidates = [
        (StatChoice::Strength, stats.strength, player.strength),
        (StatChoice::Smarts, stats.smarts, player.smarts),
        (StatChoice::Stealth, stats.stealth, player.stealth),
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
) -> anyhow::Result<PlayedQuest> {
    let mut rng = rand::thread_rng();
    let mut outcomes = Vec::new();

    for (stats, _situation) in quest.trials.iter().zip(generated.trials.iter()) {
        let stat_used = choose_stat(stats, player, &mut rng);
        let player_stat = stat_used.player_stat(player);
        let required = stat_used.trial_required(stats);
        let player_roll: u8 = rng.gen_range(1..=player_stat);
        let trial_roll: u8 = rng.gen_range(1..=required + 1);
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

pub fn play_quest(
    quest: &QuestData,
    generated: &GeneratedQuest,
    player: &Player,
) -> anyhow::Result<PlayedQuest> {
    try_quest(quest, generated, player)
}

/// Run `trials` simulations of the quest for `player` and return the fraction
/// of runs in which the player succeeded (0.0 = never, 1.0 = always).
pub fn check_job(
    quest: &QuestData,
    generated: &GeneratedQuest,
    player: &Player,
    trials: u32,
) -> anyhow::Result<f64> {
    let count = trials.max(1);
    let mut successes = 0u32;
    for _ in 0..count {
        let played = try_quest(quest, generated, player)?;
        if played.outcomes.last().map_or(false, |o| o.passed) {
            successes += 1;
        }
    }
    Ok(successes as f64 / count as f64)
}
