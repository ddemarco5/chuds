use crate::board::{Board, BoardQuest};
use crate::player::{create_chud, Player};
use crate::quest_builder::roll_trials;
use crate::simulation::play_quest;
use crate::quest_generator::{QuestData, QuestGenerator, QuestResults, TrialResult};
use crate::quest_result::QuestResult;
use crate::storage;

/// Info returned after a chud accepts a quest.
pub struct AssignInfo {
    pub player: Player,
    pub quest_title: String,
}

/// A pending generation job sent to the background worker.
pub enum GenerationJob {
    /// Generate the LLM result narrative for an already-assigned quest.
    ///
    /// Cloned from the board immediately after assignment so the command handler
    /// can release its board lock without waiting for the LLM.
    QuestResult {
        board_quest: BoardQuest,
        /// Snapshot of the player at assignment time, used as the input to the
        /// LLM prompt. Stats are re-loaded from disk when tick applies the result.
        player: Player,
    },
    /// Fully generate a new quest from a description and add it to the board.
    QuestCreation {
        quest_data: QuestData,
    },
}

// ---------------------------------------------------------------------------
// Board operations
// ---------------------------------------------------------------------------

/// Build a [`GenerationJob::QuestCreation`] from a raw description and difficulty.
pub fn make_quest_creation_job(description: String, difficulty: u8) -> GenerationJob {
    let quest_data = QuestData {
        quest_description: description,
        quest_goal: None,
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

    if board.is_player_busy(discord_user_id) {
        anyhow::bail!("user {} is already on or scouting a quest", discord_user_id);
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

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

pub fn format_dm_completion_report(
    player_name: &str,
    result: &crate::quest_result::QuestResult,
    player: &crate::player::Player,
    level_up: &crate::player::LevelUp,
    reward: u32,
) -> String {
    let mut out = String::new();
    let outcome = if result.passed { "PASSED" } else { "FAILED" };

    out.push_str(&format!("{}\n\n", player.format_stats()));

    out.push_str(&format!(
        "**{}** - {}\n{}\n",
        result.quest_title, result.quest_giver, result.quest_description
    ));

    for (i, trial) in result.trials.iter().enumerate() {
        let pass_str = if trial.passed { "\u{2705}" } else { "\u{274c}" };
        let brain = if trial.chose_optimal { " \u{1F9E0}" } else { "" };
        out.push_str(&format!(
            "\n**Trial {}** - {}\n{} {} | {} rolled {} vs {}{}\n*{}*\n",
            i + 1,
            trial.situation,
            pass_str,
            trial.stat_used.label(),
            player_name,
            trial.player_roll,
            trial.trial_roll,
            brain,
            trial.narrative,
        ));
    }
    out.push_str(&format!("──────────\n**{}**\n{}", outcome, result.summary));
    if result.passed && reward > 0 {
        out.push_str(&format!("\nYou're ${} richer!", reward));
    }

    if level_up.any() {
        fn fmt_stat(levelled: bool, val: u8) -> String {
            if levelled {
                format!("{} -> **{}**", val - 1, val)
            } else {
                val.to_string()
            }
        }
        out.push_str(&format!(
            "\n{} has improved! - Strength {}, Smarts {}, Stealth {}, Experience {}",
            player_name,
            fmt_stat(level_up.str_up, player.strength),
            fmt_stat(level_up.smt_up, player.smarts),
            fmt_stat(level_up.sth_up, player.stealth),
            fmt_stat(level_up.exp_up, player.experience),
        ));
    }
    out
}

fn oxford_join(names: &[&str]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => {
            let (last, rest) = names.split_last().unwrap();
            format!("{}, and {}", rest.join(", "), last)
        }
    }
}

pub fn format_dm_scouting_report(
    player_name: &str,
    quest_title: &str,
    chance: f64,
    active_player_name: Option<&str>,
    scouting_player_names: &[&str],
) -> String {
    let feeling = if chance == 0.0 {
        "don't want to talk about"
    } else if chance <= 0.25 {
        "are scared of"
    } else if chance <= 0.50 {
        "feel apprehensive about"
    } else if chance <= 0.75 {
        "think they can do"
    } else {
        "say they'll fuckin demolish"
    };

    let mut out = format!("**{}** checked \"{}\", they {} it.", player_name, quest_title, feeling);

    if let Some(name) = active_player_name {
        out.push_str(&format!("\nOh, and they also saw {} on the job there.", name));
    }

    if !scouting_player_names.is_empty() {
        let names_str = oxford_join(scouting_player_names);
        out.push_str(&format!("\nThey also spotted {} checking it out.", names_str));
    }

    out
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
