use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::game::domain::quest_result::CompletedTrial;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EpisodeStats {
    #[serde(default)]
    pub story: StoryEpisodeStats,
    #[serde(default)]
    pub records: RecordEpisodeStats,
    #[serde(default)]
    pub money: MoneyEpisodeStats,
    #[serde(default)]
    pub gambling: GamblingEpisodeStats,
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
    pub highest_difficulty: Option<HighestDifficultyRecord>,
    #[serde(default)]
    pub worst_roll: Option<WorstRollRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordChud {
    pub discord_user_id: u64,
    pub chud_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighestDifficultyRecord {
    pub difficulty: u8,
    #[serde(default)]
    pub chuds: Vec<RecordChud>,
    #[serde(default, skip_serializing, rename = "discord_user_id")]
    legacy_discord_user_id: Option<u64>,
    #[serde(default, skip_serializing, rename = "chud_name")]
    legacy_chud_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorstRollRecord {
    pub player_roll: u8,
    pub trial_roll: u8,
    pub margin: i16,
    #[serde(default)]
    pub chuds: Vec<RecordChud>,
    #[serde(default, skip_serializing, rename = "discord_user_id")]
    legacy_discord_user_id: Option<u64>,
    #[serde(default, skip_serializing, rename = "chud_name")]
    legacy_chud_name: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MoneyEpisodeStats {
    #[serde(default)]
    pub by_chud: HashMap<u64, ChudMoneyStats>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChudMoneyStats {
    pub chud_name: String,
    #[serde(default)]
    pub earned: u32,
    #[serde(default)]
    pub spent: u32,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GamblingEpisodeStats {
    #[serde(default)]
    pub by_chud: HashMap<u64, ChudGamblingStats>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChudGamblingStats {
    pub chud_name: String,
    #[serde(default)]
    pub gambling_won: u32,
    #[serde(default)]
    pub gambling_lost: u32,
}

impl HighestDifficultyRecord {
    fn normalize(&mut self) {
        if self.chuds.is_empty() {
            if let (Some(discord_user_id), Some(chud_name)) = (
                self.legacy_discord_user_id.take(),
                self.legacy_chud_name.take(),
            ) {
                self.chuds.push(RecordChud {
                    discord_user_id,
                    chud_name,
                });
            }
        }
    }
}

impl WorstRollRecord {
    fn normalize(&mut self) {
        if self.chuds.is_empty() {
            if let (Some(discord_user_id), Some(chud_name)) = (
                self.legacy_discord_user_id.take(),
                self.legacy_chud_name.take(),
            ) {
                self.chuds.push(RecordChud {
                    discord_user_id,
                    chud_name,
                });
            }
        }
    }
}

fn push_record_chud(chuds: &mut Vec<RecordChud>, discord_user_id: u64, chud_name: &str) {
    if let Some(entry) = chuds.iter_mut().find(|c| c.discord_user_id == discord_user_id) {
        entry.chud_name = chud_name.to_string();
    } else {
        chuds.push(RecordChud {
            discord_user_id,
            chud_name: chud_name.to_string(),
        });
    }
}

fn format_chud_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} and {b}"),
        rest => {
            let (head, tail) = rest.split_at(rest.len() - 1);
            format!("{}, and {}", head.join(", "), tail[0])
        }
    }
}

impl EpisodeStats {
    /// Migrate legacy single-chud record fields into `chuds` vectors.
    pub fn normalize(&mut self) {
        if let Some(hd) = &mut self.records.highest_difficulty {
            hd.normalize();
        }
        if let Some(wr) = &mut self.records.worst_roll {
            wr.normalize();
        }
    }

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

    /// Update the highest difficulty beaten, accumulating chuds that tie.
    pub fn update_highest_difficulty(
        &mut self,
        difficulty: u8,
        discord_user_id: u64,
        chud_name: &str,
    ) {
        match &mut self.records.highest_difficulty {
            None => {
                self.records.highest_difficulty = Some(HighestDifficultyRecord {
                    difficulty,
                    chuds: vec![RecordChud {
                        discord_user_id,
                        chud_name: chud_name.to_string(),
                    }],
                    legacy_discord_user_id: None,
                    legacy_chud_name: None,
                });
            }
            Some(current) if difficulty > current.difficulty => {
                current.difficulty = difficulty;
                current.chuds = vec![RecordChud {
                    discord_user_id,
                    chud_name: chud_name.to_string(),
                }];
            }
            Some(current) if difficulty == current.difficulty => {
                push_record_chud(&mut current.chuds, discord_user_id, chud_name);
            }
            Some(_) => {}
        }
    }

    pub fn record_money_earned(&mut self, discord_user_id: u64, chud_name: &str, amount: u32) {
        if amount == 0 {
            return;
        }
        let entry = self
            .money
            .by_chud
            .entry(discord_user_id)
            .or_insert_with(|| ChudMoneyStats {
                chud_name: chud_name.to_string(),
                ..Default::default()
            });
        entry.chud_name = chud_name.to_string();
        entry.earned = entry.earned.saturating_add(amount);
    }

    pub fn record_money_spent(&mut self, discord_user_id: u64, chud_name: &str, amount: u32) {
        if amount == 0 {
            return;
        }
        let entry = self
            .money
            .by_chud
            .entry(discord_user_id)
            .or_insert_with(|| ChudMoneyStats {
                chud_name: chud_name.to_string(),
                ..Default::default()
            });
        entry.chud_name = chud_name.to_string();
        entry.spent = entry.spent.saturating_add(amount);
    }

    pub fn record_gambling_spin(
        &mut self,
        discord_user_id: u64,
        chud_name: &str,
        pay_in: u32,
        payout: i64,
    ) {
        let net = payout - pay_in as i64;
        if net == 0 {
            return;
        }
        let entry = self
            .gambling
            .by_chud
            .entry(discord_user_id)
            .or_insert_with(|| ChudGamblingStats {
                chud_name: chud_name.to_string(),
                ..Default::default()
            });
        entry.chud_name = chud_name.to_string();
        if net > 0 {
            entry.gambling_won = entry.gambling_won.saturating_add(net as u32);
        } else {
            entry.gambling_lost = entry.gambling_lost.saturating_add((-net) as u32);
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
        match &mut self.records.worst_roll {
            None => {
                self.records.worst_roll = Some(WorstRollRecord {
                    player_roll,
                    trial_roll,
                    margin,
                    chuds: vec![RecordChud {
                        discord_user_id,
                        chud_name: chud_name.to_string(),
                    }],
                    legacy_discord_user_id: None,
                    legacy_chud_name: None,
                });
            }
            Some(current) if margin < current.margin => {
                current.player_roll = player_roll;
                current.trial_roll = trial_roll;
                current.margin = margin;
                current.chuds = vec![RecordChud {
                    discord_user_id,
                    chud_name: chud_name.to_string(),
                }];
            }
            Some(current) if margin == current.margin => {
                push_record_chud(&mut current.chuds, discord_user_id, chud_name);
            }
            Some(_) => {}
        }
    }

    /// Story mission completions in catalog order for the end-game screen.
    pub fn format_story_recap(&self, story_count: usize) -> String {
        if story_count == 0 {
            return String::new();
        }

        let mut lines = Vec::new();
        let last = story_count - 1;

        for index in 0..last {
            if let Some(beat) = self.story.beats.get(&index) {
                lines.push(format!(
                    "- {} completed by: **{}**",
                    beat.job_title, beat.chud_name
                ));
            }
        }

        if let Some(beat) = self.story.beats.get(&last) {
            lines.push(format!(
                "Final job {} completed by: **{}**",
                beat.job_title, beat.chud_name
            ));
        }

        lines.join("\n")
    }

    /// Post-game record lines (worst roll, etc.) for the end-game screen.
    pub fn format_post_game_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        if let Some(hd) = &self.records.highest_difficulty {
            if !hd.chuds.is_empty() {
                let names: Vec<&str> = hd.chuds.iter().map(|c| c.chud_name.as_str()).collect();
                lines.push(format!(
                    "{} completed the hardest job; difficulty {}",
                    format_chud_names(&names),
                    hd.difficulty
                ));
            }
        }

        if let Some(wr) = &self.records.worst_roll {
            if !wr.chuds.is_empty() {
                let names: Vec<&str> = wr.chuds.iter().map(|c| c.chud_name.as_str()).collect();
                lines.push(format!(
                    "{} had the worst roll of {} vs {}",
                    format_chud_names(&names),
                    wr.player_roll,
                    wr.trial_roll
                ));
            }
        }

        if let Some((amount, names)) = self.chuds_at_top_money(|stats| stats.earned) {
            lines.push(format!(
                "{} made the most money; ${amount}",
                format_chud_names(&names)
            ));
        }

        if let Some((amount, names)) = self.chuds_at_top_money(|stats| stats.spent) {
            lines.push(format!(
                "{} spent the most money; ${amount}",
                format_chud_names(&names)
            ));
        }

        if let Some((amount, names)) = self.chuds_at_top_gambling(|stats| stats.gambling_won) {
            lines.push(format!(
                "{} was the best gambler, they won ${amount}",
                format_chud_names(&names)
            ));
        }

        if let Some((amount, names)) = self.chuds_at_top_gambling(|stats| stats.gambling_lost) {
            lines.push(format!(
                "{} was the worst gambler, they lost ${amount}",
                format_chud_names(&names)
            ));
        }

        lines
    }

    fn chuds_at_top_money(
        &self,
        amount_for: impl Fn(&ChudMoneyStats) -> u32,
    ) -> Option<(u32, Vec<&str>)> {
        let max = self
            .money
            .by_chud
            .values()
            .map(&amount_for)
            .max()
            .filter(|&amount| amount > 0)?;

        let names: Vec<&str> = self
            .money
            .by_chud
            .values()
            .filter(|stats| amount_for(stats) == max)
            .map(|stats| stats.chud_name.as_str())
            .collect();

        if names.is_empty() {
            return None;
        }

        Some((max, names))
    }

    fn chuds_at_top_gambling(
        &self,
        amount_for: impl Fn(&ChudGamblingStats) -> u32,
    ) -> Option<(u32, Vec<&str>)> {
        let max = self
            .gambling
            .by_chud
            .values()
            .map(&amount_for)
            .max()
            .filter(|&amount| amount > 0)?;

        let names: Vec<&str> = self
            .gambling
            .by_chud
            .values()
            .filter(|stats| amount_for(stats) == max)
            .map(|stats| stats.chud_name.as_str())
            .collect();

        if names.is_empty() {
            return None;
        }

        Some((max, names))
    }
}
