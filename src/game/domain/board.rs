use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::game::domain::quest_result::QuestResult;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData};

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
    /// Catalog index when this is a story job; None for regular jobs.
    #[serde(default)]
    pub story_index: Option<usize>,
    /// Ticks until an idle job is recycled to the queue. Only decremented while idle.
    #[serde(default)]
    pub timeout: u32,
}

impl BoardQuest {
    pub fn is_story(&self) -> bool {
        self.story_index.is_some()
    }
    pub fn has_active(&self) -> bool {
        self.states
            .iter()
            .any(|s| matches!(s, QuestState::Active { .. }))
    }

    /// Returns the Discord user ID of the first (or only) active player, if any.
    pub fn assigned_to(&self) -> Option<u64> {
        self.states.iter().find_map(|s| match s {
            QuestState::Active { discord_user_id, .. } => Some(*discord_user_id),
            QuestState::Scouting { .. } => None,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Board {
    pub quests: Vec<BoardQuest>,
    pub next_id: u32,
    /// Completed quest results written by the background worker, keyed by quest id.
    /// Tick applies a result only when ticks_remaining reaches zero AND an entry is present here.
    #[serde(default)]
    pub completed_results: HashMap<u32, QuestResult>,
    /// Index of the next story job to post from the catalog.
    #[serde(default)]
    pub story_next_index: usize,
    /// Catalog length last used for story progress checks. Updated on resume when
    /// `data/story_jobs.yaml` is reloaded so a mid-game catalog swap cannot leave
    /// completion checks stuck on a stale max index.
    #[serde(default)]
    pub story_catalog_len: usize,
}

impl Board {
    pub fn has_story_quest(&self) -> bool {
        self.quests.iter().any(|q| q.is_story())
    }

    /// True once every catalog story mission has been completed.
    pub fn story_series_complete(&self) -> bool {
        let catalog_len = self.effective_story_catalog_len();
        catalog_len > 0 && self.story_next_index >= catalog_len
    }

    /// Catalog length used for completion checks. Falls back to the live catalog when
    /// unset (fresh reset before reconcile); generation always uses the live catalog.
    pub fn effective_story_catalog_len(&self) -> usize {
        if self.story_catalog_len > 0 {
            self.story_catalog_len
        } else {
            crate::story_jobs::story_count()
        }
    }

    /// Sync story progress bounds to the reloaded catalog. Returns true if board state changed.
    pub fn reconcile_story_catalog(&mut self) -> bool {
        let catalog_len = crate::story_jobs::story_count();
        let mut changed = false;

        if self.story_catalog_len != catalog_len {
            if self.story_catalog_len > 0 {
                tracing::info!(
                    old_len = self.story_catalog_len,
                    new_len = catalog_len,
                    story_next_index = self.story_next_index,
                    "story catalog length changed on resume"
                );
            }
            self.story_catalog_len = catalog_len;
            changed = true;
        }

        for quest in &self.quests {
            if let Some(index) = quest.story_index {
                if index >= catalog_len {
                    tracing::warn!(
                        quest_id = quest.id,
                        index,
                        catalog_len,
                        "story quest index outside reloaded catalog"
                    );
                }
            }
        }

        changed
    }

    /// Add a fully-generated quest to the board and return its assigned id.
    pub fn add_quest(
        &mut self,
        quest_data: QuestData,
        generated: GeneratedQuest,
        story_index: Option<usize>,
        job_timeout_tick: u32,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.quests.push(BoardQuest {
            id,
            quest_data,
            generated,
            states: Vec::new(),
            story_index,
            timeout: if story_index.is_some() {
                0
            } else {
                job_timeout_tick
            },
        });
        id
    }

    /// Assign an open quest to a player with a tick countdown.
    /// Returns `false` if the quest doesn't exist or is already taken.
    pub fn assign(
        &mut self,
        quest_id: u32,
        discord_user_id: u64,
        ticks_remaining: u32,
        job_timeout_tick: u32,
    ) -> bool {
        match self
            .quests
            .iter_mut()
            .find(|q| q.id == quest_id && !q.has_active())
        {
            Some(q) => {
                q.timeout = job_timeout_tick;
                q.states
                    .push(QuestState::Active { discord_user_id, ticks_remaining });
                true
            }
            None => false,
        }
    }

    /// Add a Scouting state for a player on any quest.
    /// Returns `false` if the quest doesn't exist.
    pub fn scout(&mut self, quest_id: u32, discord_user_id: u64, job_timeout_tick: u32) -> bool {
        match self.quests.iter_mut().find(|q| q.id == quest_id) {
            Some(q) => {
                q.timeout = job_timeout_tick;
                q.states
                    .push(QuestState::Scouting { discord_user_id });
                true
            }
            None => false,
        }
    }

    /// Set timeout on open non-story quests that lack one (legacy saves).
    pub fn backfill_job_timeouts(&mut self, job_timeout_tick: u32) {
        for q in &mut self.quests {
            if q.states.is_empty() && !q.is_story() && q.timeout == 0 {
                q.timeout = job_timeout_tick;
            }
        }
    }

    /// The active quest for a given player, if any.
    pub fn active_quest_for(&self, discord_user_id: u64) -> Option<&BoardQuest> {
        self.quests.iter().find(|q| {
            q.states.iter().any(|s| {
                matches!(
                    s,
                    QuestState::Active { discord_user_id: uid, .. } if *uid == discord_user_id
                )
            })
        })
    }

    /// Number of quests with any player state (active job or scouting).
    pub fn active_quest_count(&self) -> usize {
        self.quests.iter().filter(|q| !q.states.is_empty()).count()
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
    pub fn tick_and_take_due(&mut self) -> Vec<BoardQuest> {
        for q in &mut self.quests {
            for s in &mut q.states {
                if let QuestState::Active { ticks_remaining, .. } = s {
                    *ticks_remaining = ticks_remaining.saturating_sub(1);
                }
            }
        }

        let mut due_ids = std::collections::HashSet::new();
        for q in &self.quests {
            if let Some(QuestState::Active { ticks_remaining, .. }) = q.states.first() {
                if let Some(result) = self.completed_results.get(&q.id) {
                    let total_trials = q.quest_data.trials.len() as u32;
                    if *ticks_remaining == 0 {
                        due_ids.insert(q.id);
                    } else {
                        let trial_index =
                            total_trials.saturating_sub(*ticks_remaining + 1) as usize;
                        if result
                            .trials
                            .get(trial_index)
                            .map_or(false, |t| !t.passed)
                        {
                            due_ids.insert(q.id);
                        }
                    }
                }
            }
        }

        let (due, remaining): (Vec<_>, Vec<_>) = self
            .quests
            .drain(..)
            .partition(|q| due_ids.contains(&q.id));
        self.quests = remaining;
        due
    }
}
