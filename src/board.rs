use serde::{Deserialize, Serialize};

use crate::quest_generator::{GeneratedQuest, QuestData};

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

#[derive(Debug, Serialize, Deserialize)]
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
    /// Discord message ID of the live job-board post in the configured channel.
    #[serde(default)]
    pub board_message_id: Option<u64>,
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
    /// those that have reached zero.
    pub fn tick_and_take_due(&mut self) -> Vec<BoardQuest> {
        for q in &mut self.quests {
            if let QuestStatus::Active { ticks_remaining, .. } = &mut q.status {
                *ticks_remaining = ticks_remaining.saturating_sub(1);
            }
        }
        let (due, remaining): (Vec<_>, Vec<_>) = self.quests.drain(..).partition(|q| {
            matches!(&q.status, QuestStatus::Active { ticks_remaining, .. } if *ticks_remaining == 0)
        });
        self.quests = remaining;
        due
    }
}
