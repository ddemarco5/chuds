use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::item::Item;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::{LevelUp, Player};
use crate::game::domain::quest_result::QuestResult;
use crate::game::tick::phases;

/// Info returned after a scouting action resolves during a tick.
pub struct ScoutResult {
    pub discord_user_id: u64,
    pub player_name: String,
    pub quest_title: String,
    /// How many ticks the job would take if taken (one per trial).
    pub quest_days: u32,
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
    pub player: Player,
    pub level_up: LevelUp,
    pub reward: u32,
    pub hospitalized: bool,
    pub died: bool,
    pub death_ctx: Option<crate::game::engine::DeathContext>,
    pub item_awarded: Option<Item>,
    pub item_award_disposition: Option<crate::game::engine::ItemAwardDisposition>,
}

pub struct TickContext<'a> {
    pub board: &'a mut Board,
    pub hospital: &'a mut Hospital,
    pub queue: &'a mut JobQueue,
    pub item_registry: &'a mut ItemRegistry,
    pub max_jobs: usize,
    pub generation_queue: Option<&'a tokio::sync::mpsc::UnboundedSender<crate::game::engine::GenerationJob>>,
    pub pending_quests: Option<&'a std::sync::atomic::AtomicUsize>,
    /// Messages currently in the quest description LLM memory; gates auto job generation.
    pub description_history_msgs: usize,
    /// Backlog fill at/below which auto job generation triggers.
    pub job_gen_low_threshold: usize,
    /// Backlog fill auto job generation tops the queue up to when triggered.
    pub job_gen_high_threshold: usize,
    /// Minimum `description_history_msgs` required before auto generation runs.
    pub job_gen_min_history_msgs: usize,
}

#[derive(Default)]
pub struct TickOutcome {
    pub hospital_releases: Vec<String>,
    pub scout_results: Vec<ScoutResult>,
    pub quest_resolved: Vec<QuestResolved>,
    pub slots_filled: usize,
    /// Number of jobs the create_jobs phase requested for generation this tick.
    pub jobs_requested: usize,
    pub story_series_complete: bool,
    /// Discord user ID of the chud that finished the final story mission (when complete).
    pub final_completer_user_id: Option<u64>,
    /// Name of the chud that finished the final story mission (when complete).
    pub final_completer_chud_name: Option<String>,
}

/// Advance the game by one tick through the phase pipeline.
pub fn run_tick(ctx: &mut TickContext) -> anyhow::Result<TickOutcome> {
    let mut outcome = TickOutcome::default();
    phases::hospital::hospital_phase(ctx, &mut outcome)?;
    phases::scouting::scouting_phase(ctx, &mut outcome)?;
    phases::quests::quest_phase(ctx, &mut outcome)?;
    phases::refill::refill_phase(ctx, &mut outcome)?;
    phases::create_jobs::create_jobs_phase(ctx, &mut outcome)?;
    Ok(outcome)
}
