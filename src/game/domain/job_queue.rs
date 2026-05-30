use serde::{Deserialize, Serialize};

use crate::game::generation::quest_generator::{GeneratedQuest, QuestData};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedQuest {
    pub quest_data: QuestData,
    pub generated: GeneratedQuest,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct JobQueue {
    pub entries: Vec<QueuedQuest>,
}

impl JobQueue {
    pub fn push(&mut self, quest_data: QuestData, generated: GeneratedQuest) {
        self.entries.push(QueuedQuest {
            quest_data,
            generated,
        });
    }
}
