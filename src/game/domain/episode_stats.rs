use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::game::domain::quest_result::CompletedTrial;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EpisodeStats {
    #[serde(default)]
    pub story: StoryEpisodeStats,
    #[serde(default)]
    pub records: RecordEpisodeStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryBeat {
    pub discord_user_id: u64,
    pub chud_name: String,
    pub job_title: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StoryEpisodeStats {
    /// Catalog index -> first chud to clear that story mission this episode.
    #[serde(default)]
    pub beats: HashMap<usize, StoryBeat>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RecordEpisodeStats {
    #[serde(default)]
    pub highest_difficulty_beaten: u8,
    #[serde(default)]
    pub worst_roll: Option<WorstRollRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorstRollRecord {
    pub discord_user_id: u64,
    pub chud_name: String,
    pub player_roll: u8,
    pub trial_roll: u8,
    pub margin: i16,
}

impl EpisodeStats {
    /// Record the first chud to beat a story mission. Ignores later attempts.
    pub fn record_story_beat(
        &mut self,
        index: usize,
        discord_user_id: u64,
        chud_name: String,
        job_title: String,
    ) {
        self.story.beats.entry(index).or_insert(StoryBeat {
            discord_user_id,
            chud_name,
            job_title,
        });
    }

    /// Update the highest difficulty beaten if the passed value is higher.
    pub fn update_highest_difficulty(&mut self, difficulty: u8) {
        if difficulty > self.records.highest_difficulty_beaten {
            self.records.highest_difficulty_beaten = difficulty;
        }
    }

    /// Consider each trial margin; keep the most negative roll seen this episode.
    pub fn consider_worst_rolls(
        &mut self,
        discord_user_id: u64,
        chud_name: &str,
        trials: &[CompletedTrial],
    ) {
        for trial in trials {
            self.try_update_worst_roll(
                discord_user_id,
                chud_name,
                trial.player_roll,
                trial.trial_roll,
                trial.margin,
            );
        }
    }

    fn try_update_worst_roll(
        &mut self,
        discord_user_id: u64,
        chud_name: &str,
        player_roll: u8,
        trial_roll: u8,
        margin: i16,
    ) {
        let is_new_record = self
            .records
            .worst_roll
            .as_ref()
            .is_none_or(|current| margin < current.margin);
        if !is_new_record {
            return;
        }
        self.records.worst_roll = Some(WorstRollRecord {
            discord_user_id,
            chud_name: chud_name.to_string(),
            player_roll,
            trial_roll,
            margin,
        });
    }
}
