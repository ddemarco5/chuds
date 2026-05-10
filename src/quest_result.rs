use crate::player::Player;
use crate::quest_builder::{PlayedQuest, TrialOutcome};
use crate::quest_generator::GeneratedQuest;

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

pub struct CompletedTrial {
    pub situation: String,
    pub stat_used: String,
    pub player_roll: u8,
    pub trial_roll: u8,
    pub margin: i16,
    pub passed: bool,
    pub chose_optimal: bool,
    pub narrative: String,
}

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
                    stat_used: outcome.stat_used.label().to_string(),
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
        let count = |stat: &str| {
            self.trials.iter().filter(|t| t.stat_used == stat && t.passed).count() as u32
        };
        (count("strength"), count("smarts"), count("stealth"))
    }

    pub fn print(&self) {
        println!("\nQuest: {}", self.quest_title);
        println!("Poster: {}", self.quest_giver);
        println!("{}", self.quest_description);

        for (i, trial) in self.trials.iter().enumerate() {
            println!("\n-- Trial {} --", i + 1);
            println!("{}", trial.situation);
            println!("  {} | rolled {} vs {} -> {} [{}]",
                trial.stat_used, trial.player_roll, trial.trial_roll,
                if trial.passed { "PASS" } else { "FAIL" },
                if trial.chose_optimal { "optimal" } else { "suboptimal" });
            println!("  {}", trial.narrative);
        }

        println!("\n--- {}", self.summary);
        println!("\n{}", if self.passed { "QUEST PASSED" } else { "QUEST FAILED" });
    }
}
