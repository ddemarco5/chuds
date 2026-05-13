use rand_distr::{Distribution, Normal};
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::openrouter;
use serde::{Deserialize, Serialize};

const DESCRIPTION_SYSTEM_CONTEXT: &str = r#"You are a fantasy quest writer for a lighthearted RPG. Write in FIRST PERSON point of view.

You are an NPC quest giver describing your predicament to an adventurer.
You are writing 2 things:
   - A note that will be posted to a town job board (first person)
   - A description of what will be accomplished at the end of this quest (narrator voice)

RULES:
0. DON'T start the description with an introduction.
1. Write in FIRST PERSON as the person giving the quest
2. MATCH YOUR VOICE to who you are, use contractions and slang when appropriate. Use vocabulary and speech patterns appropriate to your social class and occupation
3. Describe your problem and why you need help
4. Keep between 20 and 80 words
5. Do not rely too heavily on adjectives
6. Avoid emdash use
7. Make sure each response is varied
10. Do not include quest names, difficulty levels, or promise rewards
11. Create and include your character name in the 'quest_giver' field - use a fitting name for your race/class/occupation if not specified. Don't pick just pick "Barnaby" each time.
12. Create a 'quest_title' field: 1-4 words that capture the writer's request (e.g. "Help with Missing Shipment", "Rats in the Cellar", "A Beggar in Need")
13. Output in YAML format with 'quest_title', 'quest_giver' (your name), 'description' (your letter), and 'goal' (the quest goal) fields

You will receive quest_description and quest_difficulty in YAML format. Infer who you are from the quest description and speak in their voice."#;
#[derive(Serialize)]
struct DescPrompt<'a> {
    quest_description: &'a str,
    quest_difficulty: u8,
}
#[derive(Deserialize)]
struct DescResponse { quest_title: String, quest_giver: String, description: String, goal: String }

const TRIAL_SYSTEM_CONTEXT: &str = r#"You are a fantasy quest narrator for a lighthearted RPG. Write in THIRD PERSON/OBJECTIVE narrator point of view.

Given quest data, a quest-giver's description, a quest goal, and a list of trials (each defined ONLY by stat requirements), invent the situation the adventurer faces in each trial.
The trials should follow a natural logical progression towards the quest's goal, taking into account the stat requirements of each trial (high number means harder)

RULES:
1. Write in THIRD PERSON as an objective narrator describing scenes
2. INVENT each trial's situation so it fits the given stat requirements. The stat values represent DIFFICULTY (the required roll to succeed) - higher numbers mean a harder challenge requiring that stat, while 0 means that stat is irrelevant and should not be hinted at.
   10 is maximum or legendary difficulty
3. Describe the SITUATION the adventurer faces, do NOT tell them what to do, do NOT give instructions for success
   - BAD: "Scour the woods for clues" (tells player to search)
   - GOOD: "The forest floor is scattered with discarded merchant trinkets" (describes the scene)
4. The trials are given in order. Make them follow a logical progression: earlier trials should naturally set up later ones (e.g., discovery before confrontation, setup before payoff). Treat the final trial as the most consequential moment.
5. Keep each trial under 40 words
6. Do not rely too heavily on adjectives
7. Avoid emdash use
8. Output in YAML format with a 'trials' field containing a list of strings (one per trial, in the same order as the input)
9. Do not promise rewards

You will receive quest data and the quest giver's description in YAML format. Match your tone to their description for consistency.
Generate output in yaml following the exact format below
"#;
#[derive(Serialize)]
struct TrialPrompt<'a> {
    quest_description: &'a str,
    quest_goal: &'a str,
    quest_difficulty: u8,
    trials: &'a [TrialStats],
    quest_giver_description: &'a str,
}
#[derive(Deserialize)]
struct TrialsResponse { trials: Vec<String> }

const RESULTS_SYSTEM_CONTEXT: &str = r#"You are a narrator for a lighthearted fantasy RPG.

You will receive a quest's context, the adventurer's name and description, and an ordered list of played trials. Each trial has the situation the adventurer faced, the ability they relied on, and a margin score.

Use the adventurer's name and description when referring to them throughout the narrative.

Your task is TWO things:
1. Rewrite each trial situation as a short narrative sentence or two that incorporates how the adventurer performed.
   Show the outcome through action and consequence, not by stating pass or fail.
   Use this margin scale to gauge the severity of the narrative.
   MARGIN SCALE:
     margin >= 1   : success with varying ease (higher = more effortless, up to 10)
     margin = 0    : barely scraped through by luck or desperation
     margin = -1   : fell just short, a near miss
     margin <= -2  : clear or disastrous failure
2. Write a short final summary (1-3 sentences) about the overall outcome of the quest. Note it's success or failure. The adventurer fleeing upon failure, or returning from the quest successful.

RULES:
1. Write in THIRD PERSON
2. Never name the ability directly — show it through the character's actions
3. Avoid excess adjective use and cliche description. Use effective words, but less is more.
4. Keep each rewritten trial under 50 words
5. Keep the final summary under 30 words
6. Avoid emdash use
7. If events seem to conflict, fudge details to make the narrative flow
8. Match the tone set by the quest giver's description

You will receive the quest context and trial outcomes in YAML format.
Generate output in yaml following the exact format below
"#;
#[derive(Serialize)]
struct TrialResultPrompt<'a> {
    situation: &'a str,
    stat_used: &'a str,
    margin: i16,
}
#[derive(Serialize)]
struct ResultsPrompt<'a> {
    quest_description: &'a str,
    quest_giver: &'a str,
    quest_giver_description: &'a str,
    adventurer_name: &'a str,
    adventurer_description: &'a str,
    trials: Vec<TrialResultPrompt<'a>>,
}
#[derive(Deserialize)]
struct ResultsResponse { trials: Vec<String>, summary: String }

// Reward formula: REWARD_QUADRATIC * cumulative^2 + REWARD_LINEAR * cumulative
// Increasing REWARD_QUADRATIC steepens the curve so high-difficulty quests pay out much more relative to easy ones.
// Increasing REWARD_LINEAR raises the baseline payout across all difficulties.
const REWARD_QUADRATIC: f64 = 0.3;
const REWARD_LINEAR: f64 = 3.0;
// Reward jitter: multiplier sampled from Normal(mean=1.0, stddev=REWARD_JITTER_STDDEV).
// 0.10 means ~68% of jobs pay within ±10% of base, ~95% within ±20%. Raise to widen the spread.
const REWARD_JITTER_STDDEV: f64 = 0.10;

type OpenRouterAgent = rig::agent::Agent<openrouter::completion::CompletionModel, ()>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TrialStats {
    pub strength: u8,
    pub smarts: u8,
    pub stealth: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestData {
    pub quest_description: String,
    pub quest_goal: Option<String>,
    pub quest_difficulty: u8,
    pub trials: Vec<TrialStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedQuest {
    pub quest_title: String,
    pub quest_giver: String,
    pub description: String,
    pub trials: Vec<String>,
    pub quest_goal: String,
    #[serde(default)]
    pub reward: u32,
}

pub struct TrialResult {
    pub situation: String,
    pub stat_used: String,
    pub margin: i16,
}

pub struct QuestResults {
    pub trials: Vec<String>,
    pub summary: String,
}

pub struct QuestGenerator {
    description_agent: OpenRouterAgent,
    trial_agent: OpenRouterAgent,
    results_agent: OpenRouterAgent,
}

impl QuestGenerator {
    pub fn new(api_key: &str) -> anyhow::Result<Self> {
        let client = openrouter::Client::new(api_key)?;
        let description_agent = client
            // .agent("openrouter/free:arcee-ai")
            // .agent("openrouter/free:Nous Research")
            .agent("openrouter/free:")
            .preamble(DESCRIPTION_SYSTEM_CONTEXT)
            .build();
        let trial_agent = client
            // .agent("openrouter/free:arcee-ai")
            // .agent("openrouter/free:Nous Research")
            .agent("openrouter/free")
            .preamble(TRIAL_SYSTEM_CONTEXT)
            .build();
        let results_agent = client
            // .agent("openrouter/free:arcee-ai")
            // .agent("openrouter/free:Nous Research")
            .agent("openrouter/free")
            .preamble(RESULTS_SYSTEM_CONTEXT)
            .build();
        Ok(Self { description_agent, trial_agent, results_agent })
    }

    fn sanitize(s: &str) -> String {
        let s = s.trim();
        let inner = s.strip_prefix("```yaml").or_else(|| s.strip_prefix("```")).unwrap_or(s);
        let stripped = if inner != s {
            if let Some(end) = inner.rfind("```") {
                inner[..end].trim()
            } else {
                s
            }
        } else {
            s
        };
        stripped.replace('\u{2019}', "'")
    }

    fn parse_retry_delay(msg: &str) -> Option<u64> {
        let json_str = &msg[msg.find("with message: ")? + "with message: ".len()..];
        let body: serde_json::Value = serde_json::from_str(json_str).ok()?;
        // Gemini
        if let Some(details) = body["error"]["details"].as_array() {
            for detail in details {
                if let Some(delay) = detail["retryDelay"].as_str() {
                    return delay.trim_end_matches('s').parse::<f64>().ok().map(|s| s.ceil() as u64);
                }
            }
        }
        // Meta llama
        if let Some(secs) = body["error"]["metadata"]["retry_after_seconds"].as_u64() {
            return Some(secs);
        }
        None
    }

    async fn prompt_parse_retry<T: serde::de::DeserializeOwned>(agent: &OpenRouterAgent, prompt: &str, expected_trials: Option<usize>) -> anyhow::Result<T> {
        const MAX_RETRIES: u32 = 10;
        let mut retries = 0;
        loop {
            let raw = Self::prompt_with_retry(agent, prompt).await?;
            let parsed = serde_yaml::from_str::<serde_yaml::Value>(&raw);
            match parsed {
                Err(e) => {
                    if retries < MAX_RETRIES {
                        retries += 1;
                        tracing::warn!(error = %e, retries, MAX_RETRIES, "malformed YAML from LLM, retrying");
                        continue;
                    }
                    return Err(anyhow::anyhow!("LLM returned malformed YAML after {} retries: {}", MAX_RETRIES, e));
                }
                Ok(value) => {
                    if let Some(expected) = expected_trials {
                        let actual = value.get("trials").and_then(|t| t.as_sequence()).map(|s| s.len());
                        if actual != Some(expected) {
                            if retries < MAX_RETRIES {
                                retries += 1;
                                tracing::warn!(expected, actual = ?actual, retries, MAX_RETRIES, "wrong trial count from LLM, retrying");
                                continue;
                            }
                            return Err(anyhow::anyhow!("LLM returned wrong trial count after {} retries: expected {}, got {:?}", MAX_RETRIES, expected, actual));
                        }
                    }
                    match serde_yaml::from_value(value) {
                        Ok(result) => return Ok(result),
                        Err(e) => {
                            if retries < MAX_RETRIES {
                                retries += 1;
                                tracing::warn!(error = %e, retries, MAX_RETRIES, "unexpected YAML structure from LLM, retrying");
                                continue;
                            }
                            return Err(anyhow::anyhow!("LLM returned unexpected YAML structure after {} retries: {}", MAX_RETRIES, e));
                        }
                    }
                }
            }
        }
    }

    async fn prompt_with_retry(agent: &OpenRouterAgent, prompt: &str) -> anyhow::Result<String> {
        const MAX_RETRIES: u32 = 6;
        let mut retries = 0;
        loop {
            match agent.prompt(prompt).await {
                Ok(response) => return Ok(Self::sanitize(&response)),
                Err(e) if retries < MAX_RETRIES => {
                    let msg = e.to_string();
                    let (wait_secs, label) = if msg.contains("503") {
                        (10, "503 model overloaded")
                    } else if msg.contains("500") {
                        (15, "500 internal server error")
                    } else if msg.contains("429") {
                        (Self::parse_retry_delay(&msg).unwrap_or(5) * 2, "429 quota exceeded")
                    } else {
                        return Err(e.into());
                    };
                    tracing::warn!(msg);
                    retries += 1;
                    tracing::warn!(error = label, wait_secs, retries, MAX_RETRIES, "retryable error, waiting before retry");
                    tokio::time::sleep(tokio::time::Duration::from_secs(wait_secs)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub async fn generate_from_description(&self, quest: &QuestData) -> anyhow::Result<GeneratedQuest> {

        let desc_yaml = serde_yaml::to_string(&DescPrompt {
            quest_description: &quest.quest_description,
            quest_difficulty: quest.quest_difficulty,
        })?;
        tracing::info!("generating quest description");
        let desc_response = Self::prompt_parse_retry::<DescResponse>(&self.description_agent, &desc_yaml, None).await?;
        tracing::info!("quest description received");
        let quest_title = desc_response.quest_title;
        let quest_giver = desc_response.quest_giver;
        let description = desc_response.description;
        let quest_goal = desc_response.goal;
        tracing::info!("quest goal is {quest_goal}");

        let trial_scaffold = {
            let slots = quest.trials.iter().enumerate().map(|(i, _)| format!("  - \"<Trial {} Description>\"", i + 1)).collect::<Vec<_>>().join("\n");
            format!("\ntrials:\n{}", slots)
        };
        let trial_yaml = format!("{}{trial_scaffold}", serde_yaml::to_string(&TrialPrompt {
            quest_description: &quest.quest_description,
            quest_goal: &quest_goal,
            quest_difficulty: quest.quest_difficulty,
            trials: &quest.trials,
            quest_giver_description: &description,
        })?);
        tracing::info!("generating quest trials");
        let trials = Self::prompt_parse_retry::<TrialsResponse>(&self.trial_agent, &trial_yaml, Some(quest.trials.len())).await?.trials;
        tracing::info!(count = trials.len(), expected = quest.trials.len(), "quest trials received");

        let reward = Self::calculate_reward(&quest.trials);
        Ok(GeneratedQuest { quest_title, quest_giver, description, trials, quest_goal, reward })
    }

    pub async fn generate_from_explicit(
        &self,
        quest: &QuestData,
        title: String,
        giver: String,
    ) -> anyhow::Result<GeneratedQuest> {

        let trial_scaffold = {
            let slots = quest.trials.iter().enumerate().map(|(i, _)| format!("  - \"<Trial {} Description>\"", i + 1)).collect::<Vec<_>>().join("\n");
            format!("\ntrials:\n{}", slots)
        };
        let trial_yaml = format!("{}{trial_scaffold}", serde_yaml::to_string(&TrialPrompt {
            quest_description: &quest.quest_description,
            quest_goal: &quest.quest_goal.as_ref().unwrap(),
            quest_difficulty: quest.quest_difficulty,
            trials: &quest.trials,
            quest_giver_description: &quest.quest_description,
        })?);
        tracing::info!("generating quest trials (description provided)");
        let trials = Self::prompt_parse_retry::<TrialsResponse>(&self.trial_agent, &trial_yaml, Some(quest.trials.len())).await?.trials;
        tracing::info!(count = trials.len(), expected = quest.trials.len(), "quest trials received");

        let reward = Self::calculate_reward(&quest.trials);
        Ok(GeneratedQuest {
            quest_title: title,
            quest_giver: giver,
            description: quest.quest_description.clone(),
            trials,
            quest_goal: quest.quest_goal.as_ref().unwrap().clone(),
            reward,
        })
    }

    fn calculate_reward(trials: &[TrialStats]) -> u32 {
        let cumulative: f64 = trials.iter().map(|t| {
            let vals = [t.strength, t.smarts, t.stealth];
            let nonzero: Vec<f64> = vals.iter().filter(|&&v| v != 0).map(|&v| v as f64).collect();
            if nonzero.is_empty() { 0.0 } else { nonzero.iter().sum::<f64>() / nonzero.len() as f64 }
        }).sum();
        let base = REWARD_QUADRATIC * cumulative * cumulative + REWARD_LINEAR * cumulative;
        let mut rng = rand::thread_rng();
        let normal = Normal::new(1.0, REWARD_JITTER_STDDEV).expect("valid normal distribution");
        let multiplier = normal.sample(&mut rng).max(0.0);
        (base * multiplier).round() as u32
    }

    pub async fn generate_results(
        &self,
        quest: &QuestData,
        generated: &GeneratedQuest,
        outcomes: &[TrialResult],
        chud_name: &str,
        chud_description: &str,
    ) -> anyhow::Result<QuestResults> {

        let scaffold = {
            let slots = outcomes.iter().enumerate().map(|(i, _)| format!("  - \"<Trial {} Narrative>\"", i + 1)).collect::<Vec<_>>().join("\n");
            format!("\nRespond using exactly this structure:\ntrials:\n{}\nsummary: \"<Quest Summary>\"", slots)
        };
        let results_yaml = serde_yaml::to_string(&ResultsPrompt {
            quest_description: &quest.quest_description,
            quest_giver: &generated.quest_giver,
            quest_giver_description: &generated.description,
            adventurer_name: chud_name,
            adventurer_description: chud_description,
            trials: outcomes.iter().map(|o| TrialResultPrompt {
                situation: &o.situation,
                stat_used: &o.stat_used,
                margin: o.margin,
            }).collect(),
        })?;
        let results_prompt = format!("{results_yaml}{scaffold}");

        tracing::info!("generating quest results");
        let r = Self::prompt_parse_retry::<ResultsResponse>(&self.results_agent, &results_prompt, Some(outcomes.len())).await?;
        tracing::info!(count = r.trials.len(), "quest results received");

        Ok(QuestResults { trials: r.trials, summary: r.summary })
    }
}
