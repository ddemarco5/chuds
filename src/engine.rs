use crate::board::{Board, BoardQuest, QuestStatus};
use crate::player::{create_chud, Player};
use crate::quest_builder::{roll_trials, play_quest};
use crate::quest_generator::{QuestData, QuestGenerator, QuestResults, TrialResult};
use crate::quest_result::QuestResult;
use crate::storage;

/// Info returned after a quest resolves during a tick.
pub struct QuestResolved {
    pub discord_user_id: u64,
    pub player_name: String,
    pub quest_title: String,
    pub summary: String,
    pub result: QuestResult,
    pub player: crate::player::Player,
    pub level_up: crate::player::LevelUp,
}

/// Info returned after a chud accepts a quest.
pub struct AssignInfo {
    pub player: Player,
    pub quest_title: String,
}

/// A pending generation job sent to the background worker.
///
/// Cloned from the board immediately after assignment so the command handler
/// can release its board lock without waiting for the LLM.
pub struct GenerationJob {
    pub board_quest: BoardQuest,
    /// Snapshot of the player at assignment time, used as the input to the
    /// LLM prompt. Stats are re-loaded from disk when tick applies the result.
    pub player: Player,
}

// ---------------------------------------------------------------------------
// Board operations
// ---------------------------------------------------------------------------

/// Fully generate a quest via the LLM and add it to the board. Returns the new quest id.
pub async fn generate_job(
    generator: &QuestGenerator,
    board: &mut Board,
    description: String,
    difficulty: u8,
) -> anyhow::Result<u32> {
    let quest_data = QuestData{
        quest_description: description,
        quest_goal: None,
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty)
    };
    let generated = generator.generate_from_description(&quest_data).await?;
    tracing::info!(title = %generated.quest_title, giver = %generated.quest_giver, "quest generated");
    let id = board.add_quest(quest_data, generated);
    storage::save_board(board)?;
    tracing::info!(id, "quest added to board");
    Ok(id)
}

/// Add a user-authored quest; only trial situations are LLM-generated. Returns the new quest id.
pub async fn write_job(
    generator: &QuestGenerator,
    board: &mut Board,
    title: String,
    giver: String,
    description: String,
    goal: String,
    difficulty: u8,
) -> anyhow::Result<u32> {
    let quest_data = QuestData{
        quest_description: description,
        quest_goal: Some(goal),
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty)
    };
    let generated = generator.generate_from_explicit(&quest_data, title, giver).await?;
    tracing::info!(title = %generated.quest_title, giver = %generated.quest_giver, "quest written");
    let id = board.add_quest(quest_data, generated);
    storage::save_board(board)?;
    tracing::info!(id, "quest added to board");
    Ok(id)
}

/// Remove a quest from the board by id (any state). Errors if not found.
pub fn delete_quest(board: &mut Board, quest_id: u32) -> anyhow::Result<()> {
    if !board.remove_quest(quest_id) {
        anyhow::bail!("quest {} not found on board", quest_id);
    }
    storage::save_board(board)?;
    tracing::info!(id = quest_id, "quest removed from board");
    Ok(())
}

// ---------------------------------------------------------------------------
// Chud operations
// ---------------------------------------------------------------------------

/// Create a chud for a Discord user and save it. Errors if one already exists.
pub fn add_chud(discord_user_id: u64, name: String, description: String) -> anyhow::Result<Player> {
    if storage::load_player(discord_user_id)?.is_some() {
        anyhow::bail!("chud already exists for user {}", discord_user_id);
    }
    let player = create_chud(discord_user_id, name, description);
    storage::save_player(&player)?;
    tracing::info!(name = %player.name, discord_user_id, "chud created");
    Ok(player)
}

/// Delete a chud's save file. Errors if not found.
pub fn delete_chud(discord_user_id: u64) -> anyhow::Result<()> {
    storage::delete_player(discord_user_id)?;
    tracing::info!(discord_user_id, "chud deleted");
    Ok(())
}

// ---------------------------------------------------------------------------
// Quest assignment
// ---------------------------------------------------------------------------

/// Assign a chud to an open board quest.
///
/// Guards:
/// - The player file must exist.
/// - The player must not already have an active quest.
/// - The quest must exist and be open.
///
/// Tick duration = one tick per trial (tune via TICK_TIME_S in .env).
pub fn assign_chud_to_quest(
    board: &mut Board,
    discord_user_id: u64,
    quest_id: u32,
) -> anyhow::Result<AssignInfo> {
    let player = storage::load_player(discord_user_id)?
        .ok_or_else(|| anyhow::anyhow!("no chud found for user {}", discord_user_id))?;

    if board.active_quest_for(discord_user_id).is_some() {
        anyhow::bail!("user {} already has an active quest", discord_user_id);
    }

    let quest = board
        .quests
        .iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found", quest_id))?;

    let trial_count = quest.quest_data.trials.len() as u32;
    let quest_title = quest.generated.quest_title.clone();

    let ticks_remaining = trial_count;

    if !board.assign(quest_id, discord_user_id, ticks_remaining) {
        anyhow::bail!("quest {} is not available for assignment", quest_id);
    }

    storage::save_board(board)?;
    tracing::info!(discord_user_id, quest_id, ticks_remaining, "chud assigned to quest");
    Ok(AssignInfo { player, quest_title })
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Run the full LLM pipeline for a single quest and return the result.
///
/// Called exclusively by the background generation worker. Does **not** touch
/// player stats or disk — the worker writes the result into
/// `board.completed_results` and saves the board itself. Separating generation
/// from stat application means a bot restart can survive a mid-flight LLM call:
/// if the bot dies before generation completes, the quest simply stays Active
/// and the worker retries nothing (operator can /delete_quest to unblock).
pub async fn generate_result(
    generator: &QuestGenerator,
    board_quest: &BoardQuest,
    player: &Player,
) -> anyhow::Result<QuestResult> {
    let played = play_quest(&board_quest.quest_data, &board_quest.generated, player)?;

    let trial_results: Vec<TrialResult> = played
        .outcomes
        .iter()
        .zip(board_quest.generated.trials.iter())
        .map(|(o, situation)| TrialResult {
            situation: situation.clone(),
            stat_used: o.stat_used.label().to_string(),
            margin: o.player_roll as i16 - o.trial_roll as i16,
        })
        .collect();

    let QuestResults { trials, summary } = generator
        .generate_results(
            &board_quest.quest_data,
            &board_quest.generated,
            &trial_results,
            &player.name,
            &player.description,
        )
        .await?;

    let result = QuestResult::build(&board_quest.generated, &played, player, trials, summary);
    result.log();
    Ok(result)
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
pub async fn tick(board: &mut Board) -> anyhow::Result<Vec<QuestResolved>> {
    let due = board.tick_and_take_due();
    tracing::info!(count = due.len(), "tick fired");

    let mut resolved = Vec::new();

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
        storage::save_player(&player)?;

        tracing::info!(
            discord_user_id,
            quest = %quest_title,
            passed,
            "quest resolved and player saved"
        );

        resolved.push(QuestResolved {
            discord_user_id,
            player_name: player.name.clone(),
            quest_title,
            summary,
            result,
            player,
            level_up,
        });

        if !passed {
            board.quests.push(BoardQuest {
                id: board_quest.id,
                quest_data: board_quest.quest_data,
                generated: board_quest.generated,
                status: QuestStatus::Open,
            });
        }
    }

    storage::save_board(board)?;
    Ok(resolved)
}

/// Run a single game tick.
pub async fn run_tick(board: &mut Board) -> anyhow::Result<Vec<QuestResolved>> {
    tick(board).await
}

// ---------------------------------------------------------------------------
// Persistence helpers (admin save / load)
// ---------------------------------------------------------------------------

pub fn save_all(board: &Board) -> anyhow::Result<()> {
    storage::save_board(board)?;
    let ids = storage::list_player_ids()?;
    let count = ids.len();
    for id in ids {
        if let Some(player) = storage::load_player(id)? {
            storage::save_player(&player)?;
        }
    }
    tracing::info!(players = count, "save_all complete");
    Ok(())
}

pub fn load_all() -> anyhow::Result<Board> {
    storage::load_board()
}
