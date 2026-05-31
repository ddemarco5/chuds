use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::hospital::Hospital;
use crate::game::domain::item::{equip_item, ItemRegistry};
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::{create_chud, Player};
use crate::game::domain::quest_result::QuestResult;
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData, QuestGenerator, QuestResults, TrialResult};
use crate::game::mechanics::item_builder::{roll_item, roll_item_drop};
use crate::game::mechanics::quest_builder::roll_trials;
use crate::game::mechanics::simulation::{effective_stats, play_quest};
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
        force_item_drop: bool,
    },
    /// Fully generate a new quest from a description and add it to the board.
    QuestCreation {
        quest_data: QuestData,
    },
}

/// Enqueue background quest result generation after assignment.
pub fn enqueue_quest_result(
    generation_queue: &tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    board_quest: BoardQuest,
    player: Player,
    force_item_drop: bool,
) -> anyhow::Result<()> {
    generation_queue
        .send(GenerationJob::QuestResult {
            board_quest,
            player,
            force_item_drop,
        })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))
}

/// Assign a chud to a quest and enqueue result generation.
pub fn take_and_enqueue_quest(
    board: &mut Board,
    hospital: &Hospital,
    discord_user_id: u64,
    quest_id: u32,
    generation_queue: &tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    force_item_drop: bool,
) -> anyhow::Result<AssignInfo> {
    let info = assign_chud_to_quest(board, hospital, discord_user_id, quest_id)?;
    let board_quest = board
        .quests
        .iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();
    enqueue_quest_result(generation_queue, board_quest, info.player.clone(), force_item_drop)?;
    Ok(info)
}

/// Move quests from the front of the queue onto the board until full or queue empty.
pub fn refill_board_from_queue(
    board: &mut Board,
    queue: &mut JobQueue,
    max_jobs: usize,
) -> usize {
    let mut added = 0usize;
    while board.quests.len() < max_jobs && !queue.entries.is_empty() {
        let entry = queue.entries.remove(0);
        board.add_quest(entry.quest_data, entry.generated);
        added += 1;
    }
    if added > 0 {
        tracing::info!(added, "board refilled from queue");
    }
    added
}

/// Push a fully generated quest onto the job queue and persist.
pub fn enqueue_quest(
    queue: &mut JobQueue,
    quest_data: QuestData,
    generated: GeneratedQuest,
) -> anyhow::Result<()> {
    queue.push(quest_data, generated);
    storage::save_job_queue(queue)
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

/// Generate trial flavor via LLM and enqueue the quest (does not add directly to board).
pub async fn write_job(
    generator: &QuestGenerator,
    queue: &mut JobQueue,
    title: String,
    giver: String,
    description: String,
    goal: String,
    difficulty: u8,
) -> anyhow::Result<()> {
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
    enqueue_quest(queue, quest_data, generated)?;
    tracing::info!(queued = queue.entries.len(), "quest added to queue");
    Ok(())
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
    hospital: &Hospital,
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

/// Register a pending item, equip it on the player, and remove any replaced item from the registry.
pub fn award_pending_item(
    registry: &mut ItemRegistry,
    player: &mut Player,
    item: crate::game::domain::item::Item,
) -> anyhow::Result<crate::game::domain::item::Item> {
    let id = registry.add_item(item);
    let awarded = registry
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("item {} missing after add", id))?
        .clone();
    if let Some(old_id) = equip_item(player, awarded.clone()) {
        registry.remove(old_id);
        tracing::info!(replaced = old_id, "old item removed from registry");
    }
    storage::save_item_registry(registry)?;
    Ok(awarded)
}

/// Run the full LLM pipeline for a single quest and return the result.
pub async fn generate_result(
    generator: &QuestGenerator,
    item_generator: &ItemGenerator,
    item_registry: &ItemRegistry,
    board_quest: &BoardQuest,
    player: &Player,
    force_item_drop: bool,
) -> anyhow::Result<QuestResult> {
    let played = play_quest(
        &board_quest.quest_data,
        &board_quest.generated,
        player,
        item_registry,
    )?;

    let effective = effective_stats(player, item_registry);

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

    let mut result = QuestResult::build(
        &board_quest.generated,
        &played,
        effective,
        trials,
        summary,
    );

    if result.passed && (force_item_drop || roll_item_drop(&mut rand::thread_rng())) {
        let seed = roll_item(board_quest.quest_data.quest_difficulty, &mut rand::thread_rng());
        let (name, description) = item_generator
            .generate_from_quest(&seed, &result, board_quest)
            .await?;
        result.pending_item = Some(seed.into_item(name, description));
        tracing::info!(
            name = %result.pending_item.as_ref().unwrap().name,
            item_type = ?result.pending_item.as_ref().unwrap().item_type,
            "item generated for quest result"
        );
    }

    result.log();
    Ok(result)
}

pub fn save_all(board: &Board, item_registry: &ItemRegistry) -> anyhow::Result<()> {
    storage::save_board(board)?;
    storage::save_item_registry(item_registry)?;
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
