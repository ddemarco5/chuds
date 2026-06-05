use rig::providers::openrouter;
use serde::{Deserialize, Serialize};

use crate::game::domain::player::Chud;
use crate::game::generation::generators::{build_agent, prompt_parse_retry, OpenRouterAgent};
use crate::game::generation::memory::{make_memory_from_store, GameConversationMemory};
use crate::game::persistence::llm_memory::LlmMemoryBundle;

const GRAVESTONE_SYSTEM_CONTEXT: &str = r#"You are writing epitaph text for a fallen Chud's gravestone.

You will receive:
- The chud's name and description
- The trial situation and failed outcome that killed them

Your task:
- Write an 'epitaph': 1-3 sentences, artistic, solemn, mildly humorous, third person
  - Capture who they were and what killed them
  - Match the satirical, crude, lighthearted tone of the world

RULES:
- Avoid emdash use
- Output ONLY valid YAML with an 'epitaph' field
- Reply with ONLY valid YAML (no preamble). You may wrap in ```yaml fences.
- Use block scalars (|) or double-quoted strings when needed"#;

#[derive(Serialize)]
struct GravestonePrompt<'a> {
    chud_name: &'a str,
    chud_description: &'a str,
    death_trial: &'a str,
    death_outcome: &'a str,
}

#[derive(Deserialize)]
struct GravestoneResponse {
    epitaph: String,
}

pub struct GravestoneGenerator {
    agent: OpenRouterAgent,
    memory: GameConversationMemory,
}

impl GravestoneGenerator {
    pub fn new(api_key: &str, memory: &LlmMemoryBundle) -> anyhow::Result<Self> {
        let client = openrouter::Client::new(api_key)?;
        Ok(Self {
            agent: build_agent(&client, GRAVESTONE_SYSTEM_CONTEXT),
            memory: make_memory_from_store(memory.gravestone.clone()),
        })
    }

    pub(crate) fn memory(&self) -> &GameConversationMemory {
        &self.memory
    }

    pub async fn generate(
        &self,
        chud: &Chud,
        death_trial: &str,
        death_outcome: &str,
    ) -> anyhow::Result<String> {
        let scaffold = "\nThe response should only contain a yaml of this structure:\n```yaml\nepitaph: REPLACE_EPITAPH\n```";
        let prompt = format!(
            "{}{scaffold}",
            serde_yaml::to_string(&GravestonePrompt {
                chud_name: &chud.name,
                chud_description: &chud.description,
                death_trial,
                death_outcome,
            })?,
        );
        tracing::info!(chud = %chud.name, "generating gravestone epitaph");
        let response = prompt_parse_retry::<GravestoneResponse>(
            &self.agent,
            &self.memory,
            &prompt,
            None,
            "gravestone",
        )
        .await?;
        tracing::info!(chud = %chud.name, "gravestone epitaph received");
        Ok(response.epitaph)
    }
}
