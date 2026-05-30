use serde::{Deserialize, Serialize};

use crate::game::domain::player::Player;
use crate::game::generation::quest_generator::GeneratedQuest;
use crate::game::mechanics::quest_builder::{PlayedQuest, StatChoice, TrialOutcome};

fn is_optimal(outcome: &TrialOutcome, player: &Player) -> bool {
    let chosen_score = outcome.player_stat as i16 - outcome.required as i16;
    let best_score = [
        (outcome.stats.strength, player.strength),
        (outcome.stats.smarts,   player.smarts),
        (outcome.stats.stealth,  player.stealth),
    ]
    .iter()
    .filter(|&&(req, _)| req > 0)
    .map(|&(req, ps)| ps as i16 - req as i16)
    .max()
    .unwrap_or(chosen_score);
    chosen_score >= best_score
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletedTrial {
    pub situation: String,
    pub stat_used: StatChoice,
    pub player_roll: u8,
    pub trial_roll: u8,
    pub margin: i16,
    pub passed: bool,
    pub chose_optimal: bool,
    pub narrative: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuestResult {
    pub quest_title: String,
    pub quest_giver: String,
    pub quest_description: String,
    pub trials: Vec<CompletedTrial>,
    pub passed: bool,
    pub summary: String,
}

impl QuestResult {
    pub fn build(
        generated: &GeneratedQuest,
        played: &PlayedQuest,
        player: &Player,
        narratives: Vec<String>,
        summary: String,
    ) -> Self {
        let passed = played.outcomes.iter().all(|o| o.passed);

        let trials = played.outcomes.iter()
            .zip(generated.trials.iter())
            .zip(narratives.into_iter())
            .map(|((outcome, situation), narrative)| {
                let margin = outcome.player_roll as i16 - outcome.trial_roll as i16;
                let chose_optimal = is_optimal(outcome, player);
                CompletedTrial {
                    situation: situation.clone(),
                    stat_used: outcome.stat_used,
                    player_roll: outcome.player_roll,
                    trial_roll: outcome.trial_roll,
                    margin,
                    passed: outcome.passed,
                    chose_optimal,
                    narrative,
                }
            })
            .collect();

        QuestResult {
            quest_title: generated.quest_title.clone(),
            quest_giver: generated.quest_giver.clone(),
            quest_description: generated.description.clone(),
            trials,
            passed,
            summary,
        }
    }

    pub fn stat_wins(&self) -> (u32, u32, u32) {
        let str_w = self.trials.iter().filter(|t| t.stat_used == StatChoice::Strength && t.passed).count() as u32;
        let smt_w = self.trials.iter().filter(|t| t.stat_used == StatChoice::Smarts   && t.passed).count() as u32;
        let sth_w = self.trials.iter().filter(|t| t.stat_used == StatChoice::Stealth  && t.passed).count() as u32;
        (str_w, smt_w, sth_w)
    }

    pub fn log(&self) {
        tracing::info!(title = %self.quest_title, giver = %self.quest_giver, description = %self.quest_description, "quest result");

        for (i, trial) in self.trials.iter().enumerate() {
            tracing::info!(
                trial = i + 1,
                situation = %trial.situation,
                stat = trial.stat_used.label(),
                player_roll = trial.player_roll,
                trial_roll = trial.trial_roll,
                passed = trial.passed,
                optimal = trial.chose_optimal,
                narrative = %trial.narrative,
                "trial outcome"
            );
        }

        tracing::info!(passed = self.passed, summary = %self.summary, "quest complete");
    }
}
