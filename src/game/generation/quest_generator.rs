use rand_distr::{Distribution, Normal};
use rig::memory::InMemoryConversationMemory;
use rig::providers::openrouter;
use serde::{Deserialize, Serialize};

use crate::game::generation::generators::{build_agent, make_memory, prompt_parse_retry, OpenRouterAgent};

const DESCRIPTION_SYSTEM_CONTEXT: &str = r#"You will be the be the person described in the prompts that follow writing a job for the town job board.

You are writing 2 things:
   - A note that will be posted to a town job board (first person)
   - A description of what will be accomplished at the end of this quest (narrator voice)

RULES:
- Ensure use of recurring characters even if specified by only first name
- Keep recurring character stories consistent
- Don't introduce yourself or start off with a hook ("Listen up!", "Hey!", "I ain't gonna sugar coat it,", etc) this is a job posting.
- Describe your problem and why you need help
- Keep between 20 and 100 words
- Avoid emdash use
- Do not include quest names, difficulty levels, or promise rewards
- Include your character name in the 'quest_giver' field - Create one if not provided. Use a first and last name
- Create a 'quest_title' field: 1-4 words that will title the job posting paper to be posted on a wall.
- Output in YAML format with 'quest_title', 'quest_giver' (your name), 'description' (your letter), and 'goal' (the quest goal) fields
- If quest_goal is present in the input YAML, use it verbatim as the 'goal' field and shape the job posting so the described problem leads to that outcome
- If quest_goal is absent, invent an appropriate goal

You will receive quest_description and quest_difficulty in YAML format. quest_goal may also be included.

YAML OUTPUT:
- Reply with ONLY valid YAML (no preamble or commentary). You may wrap the YAML in ```yaml fences.
- Use block scalars (|) or double-quoted strings for 'description' and 'goal' when they contain colons, quotes, or multiple lines."#;

#[derive(Serialize)]
struct DescPrompt<'a> {
    quest_description: &'a str,
    quest_difficulty: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    quest_goal: Option<&'a str>,
}

#[derive(Deserialize)]
struct DescResponse {
    quest_title: String,
    quest_giver: String,
    description: String,
    goal: String,
}

const TRIAL_SYSTEM_CONTEXT: &str = r#"You are a fantasy quest generator. Write in THIRD PERSON/OBJECTIVE narrator point of view.

RULES:
- Write in THIRD PERSON as an objective narrator describing scenes
- use 'quest_difficulty' (value of 1-10) to set the overall tone of danger
- the yaml provided has a list of 'trials' that contain strength,smarts, and stealth numbers. These are the difficulty (1-10) of each trial you will generate.
- Describe the SITUATION the adventurer faces, do NOT say what needs to be done. DO describe what needs to be overcome.
   - BAD: "Scour the woods for clues" (tells player to search)
   - GOOD: "The forest floor is scattered with discarded merchant trinkets" (describes the scene)
- The trials are given in order. Make them follow a logical progression: earlier trials should naturally set up later ones (e.g., discovery before confrontation, setup before payoff). Treat the final trial as the most consequential moment.
- Keep each trial under 80 words
- Avoid emdash use. 
- Do not promise rewards

You will receive quest data and the quest description in YAML format. Keep the tone of the trials consistent with the description.
If there is only one trial, ensure it encapsulates the whole adventure.
Respond with the exact YAML template shown in the user message. Replace each REPLACE_TRIAL_N placeholder (e.g. REPLACE_TRIAL_1) with one trial description string.
The input 'trials' list contains stat numbers only; your output 'trials' must be a list of description strings (same count), not stat objects.

YAML OUTPUT:
- Reply with ONLY valid YAML (no preamble or commentary). You may wrap the YAML in ```yaml fences.
- Each trial entry must be a single YAML string; use a block scalar (|) or double-quoted string if needed."#;

#[derive(Serialize)]
struct TrialPrompt<'a> {
    quest_description: &'a str,
    quest_goal: &'a str,
    quest_difficulty: u8,
    trials: &'a [TrialStats],
    quest_giver_description: &'a str,
}

#[derive(Deserialize)]
struct TrialsResponse {
    trials: Vec<String>,
}

const RESULTS_SYSTEM_CONTEXT: &str = r#"You will be narrating a Chud's attempt to overcome this job and its trials. 

You will receive a job's context, the adventurer's name and description, and an ordered list of trials.
Rolls have already been made to determine the success rate, these are stored in 'margin'
Each trial has the situation the adventurer faced, the ability they relied on, and a margin score.

Your task is TWO things:
1. Rewrite each trial situation as a short narrative sentence or two that incorporates how the adventurer performed.
   Use their description to narrate interesting attempts at each trial.
   Show the outcome through action and consequence, not by stating pass or fail.
   Use the 'passed: <bool>' value in the template to detemine if the trail was passed or failed.
   Use 'stat_used' to describe how the adventurer attempted this trial
   Use this margin scale to gauge the severity of the narrative.
   MARGIN SCALE:
     margin >= 1   : success with varying ease (higher = more effortless, up to 10)
     margin = 0    : barely scraped through by luck or desperation
     margin = -1   : fell just short, a near miss
     margin <= -2  : clear or disastrous failure

2. Write a short final summary (1-3 sentences) about the overall outcome of the job and the adventurer's return
   If ANY trial has passed: false, the entire job is FAILED; if not, the job has resulted in success.
   Ensure the summary generally follows (a passed or failed version) of the goal provided in 'quest goal:'

RULES:
- Write in THIRD PERSON
- Never name the ability directly - show it through the character's actions
- Ensure diverse word use, don't often repeat verbs used previously, especially pertaining to character actions
- Keep each rewritten trial under 80 words
- Keep the final summary under 30 words
- Avoid emdash use
- Match the tone set by the job giver's description

You will receive the job context and trial roll outcomes in YAML format.
Respond with the exact YAML template shown in the user message. Replace each REPLACE_TRIAL_N placeholder and REPLACE_QUEST_SUMMARY with your narrative.

YAML OUTPUT:
- Reply with ONLY valid YAML (no preamble or commentary). You may wrap the YAML in ```yaml fences.
- Each trial entry and 'summary' must be YAML strings; use block scalars (|) or double-quoted strings when needed."#;

#[derive(Serialize)]
struct TrialResultPrompt<'a> {
    situation: &'a str,
    stat_used: &'a str,
    margin: i16,
    passed: bool,
}

#[derive(Serialize)]
struct ResultsPrompt<'a> {
    adventurer_name: &'a str,
    adventurer_description: &'a str,
    quest_description: &'a str,
    quest_giver: &'a str,
    quest_giver_description: &'a str,
    quest_goal: &'a str,
    trials: Vec<TrialResultPrompt<'a>>,
}

#[derive(Deserialize)]
struct ResultsResponse {
    trials: Vec<String>,
    summary: String,
}

const REWARD_QUADRATIC: f64 = 0.3;
const REWARD_LINEAR: f64 = 3.0;
const REWARD_JITTER_STDDEV: f64 = 0.10;

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
    pub passed: bool,
}

pub struct QuestResults {
    pub trials: Vec<String>,
    pub summary: String,
}

pub struct QuestGenerator {
    description_agent: OpenRouterAgent,
    trial_agent: OpenRouterAgent,
    results_agent: OpenRouterAgent,
    description_memory: InMemoryConversationMemory,
    trial_memory: InMemoryConversationMemory,
    results_memory: InMemoryConversationMemory,
}

impl QuestGenerator {
    pub fn new(api_key: &str) -> anyhow::Result<Self> {
        let client = openrouter::Client::new(api_key)?;
        Ok(Self {
            description_agent: build_agent(&client, DESCRIPTION_SYSTEM_CONTEXT),
            description_memory: make_memory(),
            trial_agent: build_agent(&client, TRIAL_SYSTEM_CONTEXT),
            trial_memory: make_memory(),
            results_agent: build_agent(&client, RESULTS_SYSTEM_CONTEXT),
            results_memory: make_memory(),
        })
    }

    pub async fn generate_from_description(&self, quest: &QuestData) -> anyhow::Result<GeneratedQuest> {
        let desc_yaml = serde_yaml::to_string(&DescPrompt {
            quest_description: &quest.quest_description,
            quest_difficulty: quest.quest_difficulty,
            quest_goal: quest.quest_goal.as_deref(),
        })?;
        if quest.quest_goal.is_some() {
            tracing::info!("generating quest description with user-provided goal");
        } else {
            tracing::info!("generating quest description");
        }
        let desc_response = prompt_parse_retry::<DescResponse>(
            &self.description_agent,
            &self.description_memory,
            &desc_yaml,
            None,
            "description",
        )
        .await?;
        tracing::info!("quest description received");
        let quest_title = desc_response.quest_title;
        let quest_giver = desc_response.quest_giver;
        let description = desc_response.description;
        let quest_goal = quest.quest_goal.clone().unwrap_or(desc_response.goal);
        tracing::info!("quest goal is {quest_goal}");

        let trial_scaffold = {
            let slots = quest
                .trials
                .iter()
                .enumerate()
                .map(|(i, _)| format!("  - REPLACE_TRIAL_{}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\nThe response should only contain a yaml of this structure:\n```yaml\ntrials:\n{slots}\n```"
            )
        };
        let trial_yaml = format!(
            "{}{trial_scaffold}",
            serde_yaml::to_string(&TrialPrompt {
                quest_description: &quest.quest_description,
                quest_goal: &quest_goal,
                quest_difficulty: quest.quest_difficulty,
                quest_giver_description: &description,
                trials: &quest.trials,
            })?
        );
        tracing::info!("generating quest trials");
        let trials = prompt_parse_retry::<TrialsResponse>(
            &self.trial_agent,
            &self.trial_memory,
            &trial_yaml,
            Some(quest.trials.len()),
            "trials",
        )
        .await?
        .trials;
        tracing::info!(count = trials.len(), expected = quest.trials.len(), "quest trials received");

        let reward = Self::calculate_reward(&quest.trials);
        Ok(GeneratedQuest {
            quest_title,
            quest_giver,
            description,
            trials,
            quest_goal,
            reward,
        })
    }

    pub async fn generate_from_explicit(
        &self,
        quest: &QuestData,
        title: String,
        giver: String,
    ) -> anyhow::Result<GeneratedQuest> {
        let trial_scaffold = {
            let slots = quest
                .trials
                .iter()
                .enumerate()
                .map(|(i, _)| format!("  - REPLACE_TRIAL_{}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\nThe response should only contain a yaml of this structure:\n```yaml\ntrials:\n{slots}\n```"
            )
        };
        let trial_yaml = format!(
            "{}{trial_scaffold}",
            serde_yaml::to_string(&TrialPrompt {
                quest_description: &quest.quest_description,
                quest_goal: quest.quest_goal.as_ref().unwrap(),
                quest_difficulty: quest.quest_difficulty,
                quest_giver_description: &quest.quest_description,
                trials: &quest.trials,
            })?
        );
        tracing::info!("generating quest trials (description provided)");
        let trials = prompt_parse_retry::<TrialsResponse>(
            &self.trial_agent,
            &self.trial_memory,
            &trial_yaml,
            Some(quest.trials.len()),
            "trials",
        )
        .await?
        .trials;
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
        let cumulative: f64 = trials
            .iter()
            .map(|t| {
                let vals = [t.strength, t.smarts, t.stealth];
                let nonzero: Vec<f64> = vals
                    .iter()
                    .filter(|&&v| v != 0)
                    .map(|&v| v as f64)
                    .collect();
                if nonzero.is_empty() {
                    0.0
                } else {
                    nonzero.iter().sum::<f64>() / nonzero.len() as f64
                }
            })
            .sum();
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
            let slots = outcomes
                .iter()
                .enumerate()
                .map(|(i, _)| format!("  - REPLACE_TRIAL_{}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\nThe response should only contain a yaml of this structure:\n```yaml\ntrials:\n{slots}\nsummary: REPLACE_QUEST_SUMMARY\n```"
            )
        };
        let results_yaml = serde_yaml::to_string(&ResultsPrompt {
            adventurer_name: chud_name,
            adventurer_description: chud_description,
            quest_description: &quest.quest_description,
            quest_giver: &generated.quest_giver,
            quest_giver_description: &generated.description,
            quest_goal: &generated.quest_goal,
            trials: outcomes
                .iter()
                .map(|o| TrialResultPrompt {
                    situation: &o.situation,
                    stat_used: &o.stat_used,
                    margin: o.margin,
                    passed: o.passed,
                })
                .collect(),
        })?;
        let results_prompt = format!("{results_yaml}{scaffold}");

        tracing::info!("generating quest results");
        let r = prompt_parse_retry::<ResultsResponse>(
            &self.results_agent,
            &self.results_memory,
            &results_prompt,
            Some(outcomes.len()),
            "results",
        )
        .await?;
        tracing::info!(count = r.trials.len(), "quest results received");

        Ok(QuestResults {
            trials: r.trials,
            summary: r.summary,
        })
    }
}
