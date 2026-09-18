use serde::{Deserialize, Serialize};

use crate::game::domain::board::BoardQuest;
use crate::game::domain::item::ItemSeed;
use crate::game::domain::quest_result::QuestResult;
use crate::game::generation::generators::{
    build_agent, build_client, prompt_parse_retry, OpenRouterAgent,
};
use crate::game::generation::memory::{make_memory_from_store, GameConversationMemory};
use crate::game::persistence::llm_memory::LlmMemoryBundle;

fn item_system_context() -> String {
    format!(
        r#"You are naming and describing loot found after a Chud completes a job.

You will receive:
- Partial item data (type, optional subtype, stat bonuses, rarity) already rolled by the game
- Mission context for thematic inspiration only (setting, materials, tone)

RARITY:
- Use the item's rarity to shape name adjectives and description tone
- common: mundane, worn, unremarkable
- uncommon: slightly special craftsmanship or materials
- rare: prized, ominous, or conspicuously fine
- exceptional: unmistakably remarkable without becoming a proper-noun title

Your task:
- Write a plain 'name': a simple 2-4 word descriptor, like "Mushroom Boots" or "Rusty Dagger"
  - Combine a thematic adjective/material from the job setting with the item subtype when present
  - Invent the item type from the job setting when the item subtype is not present. A weapon can be anything from a plank of wood to a haunted rapier.
  - Take into account the item's stat modifiers when creating a description
  - Do NOT invent a unique proper-noun title
  - Be creative
- Write a 'description': 1-2 sentences describing the item itself — how it looks, feels, or what it is
  - Describe the object only; do NOT recap the quest, mention the chud, or explain how it was obtained

RULES:
- Match this world setting: {}
- Do NOT change item type, subtype, or stats
- Avoid emdash use
- Output ONLY valid YAML with 'name' and 'description' fields
- Reply with ONLY valid YAML (no preamble). You may wrap in ```yaml fences.
- Use block scalars (|) or double-quoted strings when needed"#,
        crate::story_jobs::world_setting()
    )
}

#[derive(Serialize)]
struct MissionContext<'a> {
    quest_title: &'a str,
    quest_giver: &'a str,
    quest_description: &'a str,
    trials: Vec<&'a str>,
}

#[derive(Serialize)]
struct ItemPrompt<'a> {
    item: &'a ItemSeed,
    mission: MissionContext<'a>,
}

#[derive(Deserialize)]
struct ItemResponse {
    name: String,
    description: String,
}

pub struct ItemGenerator {
    agent: OpenRouterAgent,
    memory: GameConversationMemory,
}

impl ItemGenerator {
    pub fn new(api_key: &str, memory: &LlmMemoryBundle) -> anyhow::Result<Self> {
        let client = build_client(api_key)?;
        Ok(Self {
            agent: build_agent(&client, &item_system_context()),
            memory: make_memory_from_store(memory.item.clone()),
        })
    }

    pub(crate) fn memory(&self) -> &GameConversationMemory {
        &self.memory
    }

    pub async fn generate_from_quest(
        &self,
        seed: &ItemSeed,
        result: &QuestResult,
        _board_quest: &BoardQuest,
    ) -> anyhow::Result<(String, String)> {
        let scaffold = "\nThe response should only contain a yaml of this structure:\n```yaml\nname: REPLACE_NAME\ndescription: REPLACE_DESCRIPTION\n```";
        let mission = MissionContext {
            quest_title: &result.quest_title,
            quest_giver: &result.quest_giver,
            quest_description: &result.quest_description,
            trials: result.trials.iter().map(|t| t.situation.as_str()).collect(),
        };
        let prompt = format!(
            "{}{scaffold}\nThis item is {}.",
            serde_yaml::to_string(&ItemPrompt {
                item: seed,
                mission,
            })?,
            seed.rarity,
        );
        tracing::info!("generating item flavor");
        let response = prompt_parse_retry::<ItemResponse>(
            &self.agent,
            &self.memory,
            &prompt,
            None,
            "item",
        )
        .await?;
        tracing::debug!(name = %response.name, "item flavor received");
        Ok((response.name, response.description))
    }
}
