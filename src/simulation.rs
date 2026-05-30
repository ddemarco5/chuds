use rand::Rng;

use crate::board::{Board, BoardQuest, QuestState};
use crate::player::Player;
use crate::quest_builder::{StatChoice, TrialOutcome, PlayedQuest};
use crate::quest_generator::{GeneratedQuest, QuestData, TrialStats};
use crate::quest_result::QuestResult;
use crate::storage;

/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at maximum experience (10).
const EXP_DROPOUT_MAX: f64 = 0.70;
/// Probability (0.0-1.0) that a non-optimal valid stat is dropped at minimum experience (1).
const EXP_DROPOUT_MIN: f64 = 0.05;

/// Info returned after a scouting action resolves during a tick.
pub struct ScoutResult {
    pub discord_user_id: u64,
    pub player_name: String,
    pub quest_title: String,
    pub chance: f64,
    /// Discord user ID of the player currently active on this quest, if any.
    pub active_discord_user_id: Option<u64>,
    /// Discord user IDs of other players who were also scouting this quest this tick.
    pub other_scouting_discord_user_ids: Vec<u64>,
}

/// Info returned after a quest resolves during a tick.
pub struct QuestResolved {
    pub discord_user_id: u64,
    pub player_name: String,
    pub quest_title: String,
    pub summary: String,
    pub result: QuestResult,
    pub player: crate::player::Player,
    pub level_up: crate::player::LevelUp,
    pub reward: u32,
    pub hospitalized: bool,
}

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
fn choose_stat(stats: &TrialStats, player: &Player, rng: &mut impl Rng) -> StatChoice {
    // Pair each stat choice with the trial's requirement and the player's value for it.
    let candidates = [
        (StatChoice::Strength, stats.strength, player.strength),
        (StatChoice::Smarts,   stats.smarts,   player.smarts),
        (StatChoice::Stealth,  stats.stealth,  player.stealth),
    ];

    // Drop any stat the trial doesn't require (requirement = 0), then compute
    // each remaining stat's margin: positive means the player exceeds the requirement.
    let valid: Vec<(StatChoice, i16)> = candidates.iter()
        .filter(|&&(_, req, _)| req > 0)
        .map(|&(s, req, ps)| (s, ps as i16 - req as i16))
        .collect();

    // Find the highest margin among valid stats — this is the "optimal" choice.
    let best_margin = valid.iter().map(|&(_, m)| m).max().unwrap_or(0);

    // Dropout probability scales linearly with experience:
    // low exp → near EXP_DROPOUT_MIN (mostly random), high exp → near EXP_DROPOUT_MAX (focused).
    let t = (player.experience - 1) as f64 / 9.0;
    let dropout_prob = EXP_DROPOUT_MIN + t * (EXP_DROPOUT_MAX - EXP_DROPOUT_MIN);

    // Keep the best-margin stat(s) unconditionally; randomly drop sub-optimal stats
    // based on dropout_prob — higher experience makes sub-optimal stats less likely to survive.
    let considered: Vec<StatChoice> = valid.iter()
        .filter(|&&(_, m)| m == best_margin || !rng.gen_bool(dropout_prob))
        .map(|&(s, _)| s)
        .collect();

    // Fall back to all valid stats if dropout eliminated everything, then pick randomly.
    let pool = if considered.is_empty() { valid.iter().map(|&(s, _)| s).collect() } else { considered };
    pool[rng.gen_range(0..pool.len())]
}

pub fn try_quest(quest: &QuestData, generated: &GeneratedQuest, player: &Player) -> anyhow::Result<PlayedQuest> {
    let mut rng = rand::thread_rng();
    let mut outcomes = Vec::new();

    for (stats, _situation) in quest.trials.iter().zip(generated.trials.iter()) {
        let stat_used = choose_stat(stats, player, &mut rng);
        let player_stat = stat_used.player_stat(player);
        let required = stat_used.trial_required(stats);
        let player_roll: u8 = rng.gen_range(1..=player_stat);
        let trial_roll: u8  = rng.gen_range(1..=required + 1);
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

pub fn play_quest(quest: &QuestData, generated: &GeneratedQuest, player: &Player) -> anyhow::Result<PlayedQuest> {
    try_quest(quest, generated, player)
}

/// Run `trials` simulations of the quest for `player` and return the fraction
/// of runs in which the player succeeded (0.0 = never, 1.0 = always).
pub fn check_job(quest: &QuestData, generated: &GeneratedQuest, player: &Player, trials: u32) -> anyhow::Result<f64> {
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

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// Advance the board by one tick.
///
/// Phase 1 — decrement `ticks_remaining` on every Active quest.
/// Phase 2 — collect quests whose result is ready AND either their last tick
///           has fired (`ticks_remaining == 0`) OR the current trial failed
///           (early resolution). Quests still waiting on the LLM are left on
///           the board and rechecked next tick.
/// Phase 3 — for each due quest: remove its result, reload the player from
///           disk, apply stat changes, save player, push to the return vec.
///           Failed quests are re-opened on the board.
///
/// No LLM calls happen here.
pub async fn tick(board: &mut Board) -> anyhow::Result<(Vec<QuestResolved>, Vec<ScoutResult>)> {
    // ---------------------------------------------------------------------------
    // Scouting pass — single-tick action, resolved before Active quest processing.
    // Collect every (quest_id, discord_user_id) pair that is currently Scouting,
    // strip those states from the board, then handle each one.
    // ---------------------------------------------------------------------------
    let mut scout_results: Vec<ScoutResult> = Vec::new();
    let scouting: Vec<(u32, u64)> = board.quests.iter_mut().flat_map(|q| {
        let quest_id = q.id;
        let scouts: Vec<(u32, u64)> = q.states.iter().filter_map(|s| {
            if let QuestState::Scouting { discord_user_id } = s {
                Some((quest_id, *discord_user_id))
            } else {
                None
            }
        }).collect();
        q.states.retain(|s| !matches!(s, QuestState::Scouting { .. }));
        scouts
    }).collect();

    for &(quest_id, discord_user_id) in &scouting {
        let player = match storage::load_player(discord_user_id)? {
            Some(p) => p,
            None => {
                tracing::warn!(discord_user_id, quest_id, "scouting player not found, skipping");
                continue;
            }
        };
        let quest = match board.quests.iter().find(|q| q.id == quest_id) {
            Some(q) => q,
            None => {
                tracing::warn!(quest_id, "scouted quest not found on board, skipping");
                continue;
            }
        };
        let chance = check_job(&quest.quest_data, &quest.generated, &player, 20)?;
        tracing::info!("{} checked job {} and sees a {:.2}% chance of success.", player.name, quest_id, chance*100.0);
        let active_discord_user_id = quest.assigned_to();
        let quest_title = quest.generated.quest_title.clone();
        let other_scouting_discord_user_ids: Vec<u64> = scouting.iter()
            .filter(|&&(qid, uid)| qid == quest_id && uid != discord_user_id)
            .map(|&(_, uid)| uid)
            .collect();
        scout_results.push(ScoutResult {
            discord_user_id,
            player_name: player.name.clone(),
            quest_title,
            chance,
            active_discord_user_id,
            other_scouting_discord_user_ids,
        });
    }

    let due = board.tick_and_take_due();
    tracing::info!(count = due.len(), "tick fired");

    let mut resolved = Vec::new();
    let mut hospital = crate::hospital::Hospital::load()?;

    for board_quest in due {
        let discord_user_id = match board_quest.assigned_to() {
            Some(id) => id,
            None => {
                tracing::warn!(quest_id = board_quest.id, "due quest has no assigned player, skipping");
                continue;
            }
        };

        let result = match board.completed_results.remove(&board_quest.id) {
            Some(r) => r,
            None => {
                tracing::warn!(quest_id = board_quest.id, "no completed result found, skipping");
                continue;
            }
        };

        let mut player = match storage::load_player(discord_user_id)? {
            Some(p) => p,
            None => {
                tracing::warn!(discord_user_id, quest_id = board_quest.id, "player not found, skipping quest resolution");
                continue;
            }
        };

        let passed = result.passed;
        let summary = result.summary.clone();
        let quest_title = board_quest.generated.quest_title.clone();

        let level_up = player.record_quest(&result);
        let player_name = player.name.clone(); // Clone name before player is moved
        if passed {
            tracing::info!("{} made ${}", player_name, board_quest.generated.reward);
            player.cash += board_quest.generated.reward;
        }
        storage::save_player(&player)?;

        tracing::info!(
            discord_user_id,
            quest = %quest_title,
            passed,
            "quest resolved and player saved"
        );

        // Hospitalize on severe failures (margin < -2)
        let hospitalized = !passed
            && result.trials.last().map_or(false, |t| t.margin < -2)
            && {
                let ticks = result.trials.last().unwrap().margin.abs() as u32;
                let admitted = hospital.admit(discord_user_id, player_name.clone(), ticks).is_some();
                if admitted {
                    tracing::info!(discord_user_id, ticks, "chud admitted to hospital");
                }
                admitted
            };

        resolved.push(QuestResolved {
            discord_user_id,
            player_name,
            quest_title,
            summary,
            result,
            player,
            level_up,
            reward: board_quest.generated.reward,
            hospitalized,
        });

        if !passed {
            board.quests.push(BoardQuest {
                id: board_quest.id,
                quest_data: board_quest.quest_data,
                generated: board_quest.generated,
                states: Vec::new(),
            });
        }
    }

    storage::save_board(board)?;
    hospital.save()?;
    Ok((resolved, scout_results))
}

/// Run a single game tick.
pub async fn run_tick(board: &mut Board) -> anyhow::Result<(Vec<QuestResolved>, Vec<ScoutResult>)> {
    tick(board).await
}
