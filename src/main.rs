mod player;
mod quest_builder;
mod quest_generator;
mod quest_result;

use quest_generator::{QuestGenerator, QuestResults, TrialResult};
use quest_result::QuestResult;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set in environment or .env file"))?;

    let generator = QuestGenerator::new(&api_key)?;

    let mut player = player::prompt_player()?;
    println!("Chud: {} - {}", player.name, player.description);
    player.print_stats();

    loop {
        let (description, difficulty) = quest_builder::prompt_user()?;
        let quest = quest_builder::build_quest(description, difficulty);

        let generated = generator.submit(&quest).await?;

        println!("Rolled {} trial(s) for difficulty {}:", quest.trials.len(), quest.quest_difficulty);
        for (i, stats) in quest.trials.iter().enumerate() {
            println!("  Trial {}: str={} smt={} sth={}", i + 1, stats.strength, stats.smarts, stats.stealth);
        }

        let played = quest_builder::play_quest(&quest, &generated, &player)?;

        let trial_results: Vec<TrialResult> = played.outcomes.iter()
            .zip(generated.trials.iter())
            .map(|(o, situation)| TrialResult {
                situation: situation.clone(),
                stat_used: o.stat_used.label().to_string(),
                margin: o.player_roll as i16 - o.trial_roll as i16,
            })
            .collect();

        let QuestResults { trials, summary } = generator
            .generate_results(&quest, &generated, &trial_results, &player.name, &player.description)
            .await?;

        let result = QuestResult::build(&generated, &played, &player, trials, summary);

        result.print();
        player.record_quest(&result);
    }
}
