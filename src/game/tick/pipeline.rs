use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::{LevelUp, Player};
use crate::game::domain::quest_result::QuestResult;
use crate::game::tick::phases;

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
    pub player: Player,
    pub level_up: LevelUp,
    pub reward: u32,
    pub hospitalized: bool,
}

pub struct TickContext<'a> {
    pub board: &'a mut Board,
    pub hospital: &'a mut Hospital,
    pub queue: &'a mut JobQueue,
    pub max_jobs: usize,
}

#[derive(Default)]
pub struct TickOutcome {
    pub hospital_releases: Vec<String>,
    pub scout_results: Vec<ScoutResult>,
    pub quest_resolved: Vec<QuestResolved>,
    pub slots_filled: usize,
}

/// Advance the game by one tick through the phase pipeline.
pub fn run_tick(ctx: &mut TickContext) -> anyhow::Result<TickOutcome> {
    let mut outcome = TickOutcome::default();
    phases::hospital::hospital_phase(ctx, &mut outcome)?;
    phases::scouting::scouting_phase(ctx, &mut outcome)?;
    phases::quests::quest_phase(ctx, &mut outcome)?;
    phases::refill::refill_phase(ctx, &mut outcome)?;
    Ok(outcome)
}
