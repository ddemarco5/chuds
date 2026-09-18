use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::guild_status;

/// Reason why a player is busy and cannot take new quests.
#[derive(Debug, Clone)]
pub enum BusyReason {
    ActiveQuest { quest_title: String },
    Scouting,
    Hospitalized,
    ReturningFromJob,
}

/// Returns why the player is busy, or None if available.
/// Checks: Active quest → Scouting → Hospitalized → Returning from job
pub fn is_player_busy(
    board: &Board,
    hospital: &Hospital,
    discord_user_id: u64,
) -> Option<BusyReason> {
    guild_status::compute_guild_hall_status(board, hospital)
        .ok()
        .and_then(|s| s.busy_reason(discord_user_id))
}

/// Returns why the chud's loadout is locked (active job or scouting).
/// Hospitalized and returning chuds may still change gear.
pub fn is_equipment_locked(
    board: &Board,
    hospital: &Hospital,
    discord_user_id: u64,
) -> Option<BusyReason> {
    match is_player_busy(board, hospital, discord_user_id)? {
        BusyReason::Hospitalized | BusyReason::ReturningFromJob => None,
        reason => Some(reason),
    }
}
