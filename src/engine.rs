use crate::board::Board;
use crate::player::{create_chud, Player};
use crate::quest_builder::{build_quest, play_quest};
use crate::quest_generator::{QuestGenerator, QuestResults, TrialResult};
use crate::quest_result::QuestResult;
use crate::storage;

/// Number of trials resolved per tick. Tune for game balance.
pub const TRIALS_PER_TICK: u32 = 1;

/// Info returned after a quest resolves during a tick.
pub struct QuestResolved {
    pub discord_user_id: u64,
    pub player_name: String,
    pub quest_title: String,
    pub passed: bool,
    pub summary: String,
    pub result: QuestResult,
    pub player: crate::player::Player,
}

/// Info returned after a chud accepts a quest.
pub struct AssignInfo {
    pub player_name: String,
    pub quest_title: String,
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
    let quest_data = build_quest(description, difficulty);
    let generated = generator.submit(&quest_data).await?;
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
    difficulty: u8,
) -> anyhow::Result<u32> {
    let quest_data = build_quest(description.clone(), difficulty);
    let generated = generator.submit_with_description(&quest_data, title, giver, description).await?;
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
/// Tick duration = ceil(trial_count / TRIALS_PER_TICK).
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

    let ticks_remaining = trial_count.div_ceil(TRIALS_PER_TICK);

    if !board.assign(quest_id, discord_user_id, ticks_remaining) {
        anyhow::bail!("quest {} is not available for assignment", quest_id);
    }

    storage::save_board(board)?;
    tracing::info!(discord_user_id, quest_id, ticks_remaining, "chud assigned to quest");
    Ok(AssignInfo { player_name: player.name, quest_title })
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// Advance the board by one tick: decrement all active quest counters, then
/// resolve any that have reached zero via the LLM, update player stats, and
/// persist state.
pub async fn tick(generator: &QuestGenerator, board: &mut Board) -> anyhow::Result<Vec<QuestResolved>> {
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

        tracing::info!(
            quest_id = board_quest.id,
            title = %board_quest.generated.quest_title,
            discord_user_id,
            "resolving quest"
        );

        let mut player = match storage::load_player(discord_user_id)? {
            Some(p) => p,
            None => {
                tracing::warn!(discord_user_id, quest_id = board_quest.id, "player not found, skipping quest resolution");
                continue;
            }
        };

        let played = play_quest(&board_quest.quest_data, &board_quest.generated, &player)?;

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

        let result = QuestResult::build(&board_quest.generated, &played, &player, trials, summary);

        result.log();
        player.record_quest(&result);
        storage::save_player(&player)?;

        tracing::info!(
            discord_user_id,
            quest = %board_quest.generated.quest_title,
            passed = result.passed,
            "quest resolved and player saved"
        );

        resolved.push(QuestResolved {
            discord_user_id,
            player_name: player.name.clone(),
            quest_title: board_quest.generated.quest_title.clone(),
            passed: result.passed,
            summary: result.summary.clone(),
            result,
            player,
        });
    }

    storage::save_board(board)?;
    Ok(resolved)
}

/// Run a single game tick.
pub async fn run_tick(generator: &QuestGenerator, board: &mut Board) -> anyhow::Result<Vec<QuestResolved>> {
    tick(generator, board).await
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
