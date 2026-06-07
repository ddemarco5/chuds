use std::collections::{HashMap, HashSet};

use crate::game::busy::BusyReason;
use crate::game::domain::board::{Board, QuestState};
use crate::game::domain::hospital::Hospital;
use crate::game::persistence::storage;

/// Roster assignment index and guild-hall status message inputs, from one board + hospital scan.
#[derive(Debug, Clone)]
pub struct GuildHallStatus {
    active: HashMap<u64, String>,
    scouting: HashSet<u64>,
    hospitalized: HashSet<u64>,
    pub idle_names: Vec<String>,
    pub hospital_names: Vec<String>,
    pub show_all_busy: bool,
    pub show_no_chuds: bool,
}

impl GuildHallStatus {
    pub fn busy_reason(&self, discord_user_id: u64) -> Option<BusyReason> {
        if let Some(quest_title) = self.active.get(&discord_user_id) {
            return Some(BusyReason::ActiveQuest {
                quest_title: quest_title.clone(),
            });
        }
        if self.scouting.contains(&discord_user_id) {
            return Some(BusyReason::Scouting);
        }
        if self.hospitalized.contains(&discord_user_id) {
            return Some(BusyReason::Hospitalized);
        }
        None
    }

    pub fn is_busy(&self, discord_user_id: u64) -> bool {
        self.busy_reason(discord_user_id).is_some()
    }
}

/// Rebuilds who is busy and who is loitering in the guild hall from current game state.
///
/// Walks every quest on the board once to collect players on active jobs or scouting,
/// merges in hospital admissions, then treats every other saved chud as idle. The result
/// powers the persistent status box under the job board (idle / all-busy / no-chuds / hospital lists)
/// and cheap per-user checks via [`GuildHallStatus::busy_reason`] without rescanning quests.
///
/// Called whenever assignments might change, including: every game tick
/// ([`crate::discord::tick::execute_tick`]), job-board Take and Scout buttons (before and
/// after the mutation), heal and `/chud` registration, admin assign flows, gear UI updates,
/// and any path that calls [`crate::discord::guild_hall::update_board_message`] or
/// [`crate::discord::guild_hall::refresh_board_status`] without an already-fresh snapshot.
pub fn compute_guild_hall_status(
    board: &Board,
    hospital: &Hospital,
) -> anyhow::Result<GuildHallStatus> {
    let mut active = HashMap::new();
    let mut scouting = HashSet::new();

    for quest in &board.quests {
        let title = quest.generated.quest_title.clone();
        for state in &quest.states {
            match state {
                QuestState::Active { discord_user_id, .. } => {
                    active.insert(*discord_user_id, title.clone());
                }
                QuestState::Scouting { discord_user_id } => {
                    scouting.insert(*discord_user_id);
                }
            }
        }
    }

    let mut hospitalized = HashSet::new();
    let mut hospital_names: Vec<String> = hospital
        .entries
        .iter()
        .map(|e| {
            hospitalized.insert(e.discord_user_id);
            e.chud_name.clone()
        })
        .collect();
    hospital_names.sort();

    let player_ids = storage::list_player_ids()?;
    let mut idle_names = Vec::new();
    let mut has_any_chud = false;

    for &id in &player_ids {
        if let Some(player) = storage::load_player(id)? {
            if !player.has_chud() {
                continue;
            }
            has_any_chud = true;
            if active.contains_key(&id) || scouting.contains(&id) || hospitalized.contains(&id) {
                continue;
            }
            idle_names.push(player.chud_ref().name.clone());
        }
    }
    idle_names.sort();

    let show_no_chuds = !has_any_chud;
    let show_all_busy = has_any_chud && idle_names.is_empty();

    Ok(GuildHallStatus {
        active,
        scouting,
        hospitalized,
        idle_names,
        hospital_names,
        show_all_busy,
        show_no_chuds,
    })
}
