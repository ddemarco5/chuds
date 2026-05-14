use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::quest_generator::{GeneratedQuest, QuestData};
use crate::quest_result::QuestResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum QuestState {
    Active {
        discord_user_id: u64,
        /// Ticks remaining until this quest resolves. Decremented once per tick.
        ticks_remaining: u32,
    },
    Scouting {
        discord_user_id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardQuest {
    pub id: u32,
    pub quest_data: QuestData,
    pub generated: GeneratedQuest,
    /// Empty = open/available. One or more entries = actively being run by those players.
    #[serde(default)]
    pub states: Vec<QuestState>,
}

impl BoardQuest {

    pub fn has_active(&self) -> bool {
        self.states.iter().any(|s| matches!(s, QuestState::Active { .. }))
    }

    /// Returns the Discord user ID of the first (or only) active player, if any.
    pub fn assigned_to(&self) -> Option<u64> {
        self.states.iter().find_map(|s| match s {
            QuestState::Active { discord_user_id, .. } => Some(*discord_user_id),
            QuestState::Scouting { .. } => None,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Board {
    pub quests: Vec<BoardQuest>,
    pub next_id: u32,
    /// Discord message ID of the persistent chudlerboard post.
    #[serde(default)]
    pub chudlerboard_message_id: Option<u64>,
    /// Completed quest results written by the background worker, keyed by quest id.
    /// Tick applies a result only when ticks_remaining reaches zero AND an entry is present here.
    #[serde(default)]
    pub completed_results: HashMap<u32, QuestResult>,
}

impl Board {
    /// Add a fully-generated quest to the board and return its assigned id.
    pub fn add_quest(&mut self, quest_data: QuestData, generated: GeneratedQuest) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.quests.push(BoardQuest {
            id,
            quest_data,
            generated,
            states: Vec::new(),
        });
        id
    }


    /// Assign an open quest to a player with a tick countdown.
    /// Returns `false` if the quest doesn't exist or is already taken.
    pub fn assign(&mut self, quest_id: u32, discord_user_id: u64, ticks_remaining: u32) -> bool {
        match self.quests.iter_mut().find(|q| q.id == quest_id && !q.has_active()) {
            Some(q) => {
                q.states.push(QuestState::Active { discord_user_id, ticks_remaining });
                true
            }
            None => false,
        }
    }

    /// Add a Scouting state for a player on any quest.
    /// Returns `false` if the quest doesn't exist.
    pub fn scout(&mut self, quest_id: u32, discord_user_id: u64) -> bool {
        match self.quests.iter_mut().find(|q| q.id == quest_id) {
            Some(q) => {
                q.states.push(QuestState::Scouting { discord_user_id });
                true
            }
            None => false,
        }
    }

    /// The active quest for a given player, if any.
    pub fn active_quest_for(&self, discord_user_id: u64) -> Option<&BoardQuest> {
        self.quests.iter().find(|q| {
            q.states.iter().any(|s| matches!(s, QuestState::Active { discord_user_id: uid, .. } if *uid == discord_user_id))
        })
    }

    /// True if the player has any Active or Scouting state on any quest.
    pub fn is_player_busy(&self, discord_user_id: u64) -> bool {
        self.quests.iter().any(|q| {
            q.states.iter().any(|s| match s {
                QuestState::Active { discord_user_id: uid, .. } => *uid == discord_user_id,
                QuestState::Scouting { discord_user_id: uid } => *uid == discord_user_id,
            })
        })
    }

    /// Remove a quest by id regardless of state. Returns `false` if not found.
    pub fn remove_quest(&mut self, quest_id: u32) -> bool {
        let before = self.quests.len();
        self.quests.retain(|q| q.id != quest_id);
        self.quests.len() < before
    }

    /// Decrement `ticks_remaining` on every active quest, then remove and return
    /// quests that are ready to resolve this tick. A quest is due when its result
    /// is present in `completed_results` AND either:
    ///   - `ticks_remaining` has reached zero (all trials elapsed), OR
    ///   - the current trial's result is a failure (early resolution).
    ///
    /// The current trial index (0-based) after decrement is:
    ///   `total_trials - ticks_remaining - 1`
    /// i.e. `total_trials - ticks_remaining` gives the 1-based trial number.
    ///
    /// Quests whose generation result isn't ready yet are left on the board
    /// and rechecked next tick.
    pub fn tick_and_take_due(&mut self) -> Vec<BoardQuest> {
        // Phase 1: decrement every active quest's counter.
        for q in &mut self.quests {
            for s in &mut q.states {
                if let QuestState::Active { ticks_remaining, .. } = s {
                    *ticks_remaining = ticks_remaining.saturating_sub(1);
                }
            }
        }

        // Phase 2: build the set of quest IDs that should resolve this tick.
        // We do this as a separate pass so the drain closure below doesn't need
        // to borrow self.completed_results at the same time as self.quests.
        let mut due_ids = std::collections::HashSet::new();
        for q in &self.quests {
            if let Some(QuestState::Active { ticks_remaining, .. }) = q.states.first() {
                if let Some(result) = self.completed_results.get(&q.id) {
                    let total_trials = q.quest_data.trials.len() as u32;
                    if *ticks_remaining == 0 {
                        // Final tick: resolve regardless of pass/fail.
                        due_ids.insert(q.id);
                    } else {
                        // Intermediate tick: resolve early only if the trial
                        // we just attempted is a failure.
                        let trial_index = total_trials.saturating_sub(*ticks_remaining + 1) as usize;
                        if result.trials.get(trial_index).map_or(false, |t| !t.passed) {
                            due_ids.insert(q.id);
                        }
                    }
                }
            }
        }

        // Phase 3: partition quests into due and remaining.
        let (due, remaining): (Vec<_>, Vec<_>) = self.quests.drain(..).partition(|q| {
            due_ids.contains(&q.id)
        });
        self.quests = remaining;
        due
    }
}
