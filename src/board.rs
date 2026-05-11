use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::quest_generator::{GeneratedQuest, QuestData};
use crate::quest_result::QuestResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum QuestStatus {
    Open,
    Active {
        discord_user_id: u64,
        /// Ticks remaining until this quest resolves. Decremented once per tick.
        ticks_remaining: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardQuest {
    pub id: u32,
    pub quest_data: QuestData,
    pub generated: GeneratedQuest,
    pub status: QuestStatus,
}

impl BoardQuest {
    /// Returns the Discord user ID assigned to this quest, if any.
    pub fn assigned_to(&self) -> Option<u64> {
        match &self.status {
            QuestStatus::Active { discord_user_id, .. } => Some(*discord_user_id),
            QuestStatus::Open => None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Board {
    pub quests: Vec<BoardQuest>,
    pub next_id: u32,
    /// Discord message ID of the persistent chudlerboard post.
    #[serde(default)]
    pub chudlerboard_message_id: Option<u64>,
    /// Discord message IDs for each job slot (index == slot position, length ≤ MAX_JOBS).
    #[serde(default)]
    pub job_slot_message_ids: Vec<u64>,
    /// Discord message ID of the persistent divider posted between job slots and buffered messages.
    #[serde(default)]
    pub divider_message_id: Option<u64>,
    /// Discord message IDs to delete at the start of the next tick.
    #[serde(default)]
    pub pending_deletes: Vec<u64>,
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
            status: QuestStatus::Open,
        });
        id
    }

    /// All quests currently available for assignment.
    pub fn open_quests(&self) -> impl Iterator<Item = &BoardQuest> {
        self.quests.iter().filter(|q| matches!(q.status, QuestStatus::Open))
    }

    /// Assign an open quest to a player with a tick countdown.
    /// Returns `false` if the quest doesn't exist or is already taken.
    pub fn assign(&mut self, quest_id: u32, discord_user_id: u64, ticks_remaining: u32) -> bool {
        match self.quests.iter_mut().find(|q| q.id == quest_id && matches!(q.status, QuestStatus::Open)) {
            Some(q) => {
                q.status = QuestStatus::Active { discord_user_id, ticks_remaining };
                true
            }
            None => false,
        }
    }

    /// The active quest for a given player, if any.
    pub fn active_quest_for(&self, discord_user_id: u64) -> Option<&BoardQuest> {
        self.quests.iter().find(|q| {
            matches!(&q.status, QuestStatus::Active { discord_user_id: uid, .. } if *uid == discord_user_id)
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
            if let QuestStatus::Active { ticks_remaining, .. } = &mut q.status {
                *ticks_remaining = ticks_remaining.saturating_sub(1);
            }
        }

        // Phase 2: build the set of quest IDs that should resolve this tick.
        // We do this as a separate pass so the drain closure below doesn't need
        // to borrow self.completed_results at the same time as self.quests.
        let mut due_ids = std::collections::HashSet::new();
        for q in &self.quests {
            if let QuestStatus::Active { ticks_remaining, .. } = &q.status {
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
