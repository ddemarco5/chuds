use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::player::{create_chud, Player};
use crate::game::domain::quest_result::QuestResult;
use crate::game::generation::quest_generator::{QuestData, QuestGenerator, QuestResults, TrialResult};
use crate::game::mechanics::quest_builder::roll_trials;
use crate::game::mechanics::simulation::play_quest;
use crate::game::persistence::storage;

/// Info returned after a chud accepts a quest.
pub struct AssignInfo {
    pub player: Player,
    pub quest_title: String,
}

/// A pending generation job sent to the background worker.
pub enum GenerationJob {
    /// Generate the LLM result narrative for an already-assigned quest.
    QuestResult {
        board_quest: BoardQuest,
        player: Player,
    },
    /// Fully generate a new quest from a description and add it to the board.
    QuestCreation {
        quest_data: QuestData,
    },
}

/// Build a [`GenerationJob::QuestCreation`] from a raw description and difficulty.
pub fn make_quest_creation_job(
    description: String,
    difficulty: u8,
    quest_goal: Option<String>,
) -> GenerationJob {
    let quest_data = QuestData {
        quest_description: description,
        quest_goal,
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty),
    };
    GenerationJob::QuestCreation { quest_data }
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
    let quest_data = QuestData {
        quest_description: description,
        quest_goal: Some(goal),
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty),
    };
    let generated = generator
        .generate_from_explicit(&quest_data, title, giver)
        .await?;
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

/// Assign a chud to an open board quest.
pub fn assign_chud_to_quest(
    board: &mut Board,
    hospital: &crate::game::domain::hospital::Hospital,
    discord_user_id: u64,
    quest_id: u32,
) -> anyhow::Result<AssignInfo> {
    let player = storage::load_player(discord_user_id)?
        .ok_or_else(|| anyhow::anyhow!("no chud found for user {}", discord_user_id))?;

    if crate::game::busy::is_player_busy(board, hospital, discord_user_id).is_some() {
        anyhow::bail!("user {} is busy", discord_user_id);
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
    Ok(AssignInfo {
        player,
        quest_title,
    })
}

/// Run the full LLM pipeline for a single quest and return the result.
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
            passed: o.passed,
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
