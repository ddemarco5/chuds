use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;

/// Reason why a player is busy and cannot take new quests.
#[derive(Debug, Clone)]
pub enum BusyReason {
    ActiveQuest { quest_title: String },
    Scouting,
    Hospitalized,
}

/// Returns why the player is busy, or None if available.
/// Checks: Active quest → Scouting → Hospitalized
pub fn is_player_busy(
    board: &Board,
    hospital: &Hospital,
    discord_user_id: u64,
) -> Option<BusyReason> {
    if let Some(quest) = board.active_quest_for(discord_user_id) {
        return Some(BusyReason::ActiveQuest {
            quest_title: quest.generated.quest_title.clone(),
        });
    }

    let is_scouting = board.quests.iter().any(|q| {
        q.states.iter().any(|s| {
            matches!(
                s,
                crate::game::domain::board::QuestState::Scouting {
                    discord_user_id: uid
                } if *uid == discord_user_id
            )
        })
    });
    if is_scouting {
        return Some(BusyReason::Scouting);
    }

    if hospital.is_hospitalized(discord_user_id) {
        return Some(BusyReason::Hospitalized);
    }

    None
}
