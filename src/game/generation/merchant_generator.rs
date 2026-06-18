use rig::memory::ConversationMemory;
use rig::providers::openrouter;
use serde::{Deserialize, Serialize};

use crate::game::domain::item::ItemSeed;
use crate::game::generation::generators::{build_agent, prompt_parse_retry, OpenRouterAgent};
use crate::game::generation::memory::{make_memory_from_store, GameConversationMemory};
use crate::game::merchant::{MerchantCatalog, MerchantDefinition, MERCHANT_ROSTER_SIZE};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::llm_memory::LlmMemoryBundle;
use crate::game::tuneable_rolls::roll_item_value;

const MERCHANT_PROFILE_SYSTEM_CONTEXT: &str = r#"You are inventing traveling merchants who visit a Chud guild hall to sell gear.

Your task:
- Create distinct merchant shop identities for a satirical, crude, lighthearted fantasy world
- Each merchant needs a plain 'name' (2-5 words, like "Old Ropey Pete" or "The Scrap Witch")
- Each merchant needs a 'theme': 1-2 sentences describing their shop vibe, specialty, and aesthetic (used later to name their inventory)

RULES:
- Merchants should feel varied: junk peddlers, suspicious alchemists, ex-adventurer flippers, etc.
- Do NOT use legendary proper-noun titles
- Avoid emdash use
- Output ONLY valid YAML
- Reply with ONLY valid YAML (no preamble). You may wrap in ```yaml fences."#;

const MERCHANT_STOCK_SYSTEM_CONTEXT: &str = r#"You are naming and describing items sold by a traveling merchant.

You will receive:
- The merchant's name and theme
- Partial item data (type, optional subtype, stat bonuses, rarity) already rolled by the game

RARITY:
- Use the item's rarity to shape name adjectives and description tone
- common: mundane, worn, unremarkable
- uncommon: slightly special craftsmanship or materials
- rare: prized, ominous, or conspicuously fine
- exceptional: unmistakably remarkable without becoming a proper-noun title

Your task:
- For each item, write a plain 'name': a simple 2-4 word descriptor
  - Draw from the merchant's theme for materials and flavor
  - Invent the item type from the theme when subtype is not present
  - Take into account stat modifiers when naming
  - Do NOT invent unique proper-noun titles
- For each item, write a 'description': 1-2 sentences about how it looks or feels
  - Describe the object only; do NOT mention the merchant by name or explain the sale

RULES:
- Match the satirical, crude, lighthearted tone of the world
- Do NOT change item type, subtype, or stats
- Return exactly as many items as provided in the input, in the same order
- Avoid emdash use
- Output ONLY valid YAML with an 'items' list of {name, description} objects
- Reply with ONLY valid YAML (no preamble). You may wrap in ```yaml fences."#;

#[derive(Serialize)]
struct MerchantProfilePrompt {
    count: usize,
}

#[derive(Deserialize)]
struct MerchantProfileEntry {
    name: String,
    theme: String,
}

#[derive(Deserialize)]
struct MerchantProfilesResponse {
    merchants: Vec<MerchantProfileEntry>,
}

#[derive(Serialize)]
struct MerchantStockPrompt<'a> {
    merchant_name: &'a str,
    merchant_theme: &'a str,
    items: &'a [ItemSeed],
}

#[derive(Deserialize)]
struct StockFlavorEntry {
    name: String,
    description: String,
}

#[derive(Deserialize)]
struct MerchantStockResponse {
    items: Vec<StockFlavorEntry>,
}

pub struct MerchantGenerator {
    profile_agent: OpenRouterAgent,
    stock_agent: OpenRouterAgent,
    memory: GameConversationMemory,
}

impl MerchantGenerator {
    pub fn new(api_key: &str, memory: &LlmMemoryBundle) -> anyhow::Result<Self> {
        let client = openrouter::Client::new(api_key)?;
        Ok(Self {
            profile_agent: build_agent(&client, MERCHANT_PROFILE_SYSTEM_CONTEXT),
            stock_agent: build_agent(&client, MERCHANT_STOCK_SYSTEM_CONTEXT),
            memory: make_memory_from_store(memory.merchant.clone()),
        })
    }

    pub(crate) fn memory(&self) -> &GameConversationMemory {
        &self.memory
    }

    async fn generate_profiles(&self, count: usize) -> anyhow::Result<Vec<MerchantProfileEntry>> {
        let scaffold = "\nThe response should only contain a yaml of this structure:\n```yaml\nmerchants:\n  - name: REPLACE_NAME\n    theme: REPLACE_THEME\n```";
        let prompt = format!(
            "{}{scaffold}\nGenerate exactly {count} merchants.",
            serde_yaml::to_string(&MerchantProfilePrompt { count })?,
        );
        tracing::info!(count, "generating merchant profiles");
        let response = prompt_parse_retry::<MerchantProfilesResponse>(
            &self.profile_agent,
            &self.memory,
            &prompt,
            None,
            "merchant_profiles",
        )
        .await?;
        if response.merchants.len() != count {
            anyhow::bail!(
                "expected {count} merchant profiles, got {}",
                response.merchants.len()
            );
        }
        tracing::info!(count, "merchant profiles received");
        Ok(response.merchants)
    }

    async fn generate_stock_flavor(
        &self,
        merchant_name: &str,
        merchant_theme: &str,
        seeds: &[ItemSeed],
    ) -> anyhow::Result<Vec<StockFlavorEntry>> {
        let expected = seeds.len();
        let scaffold = "\nThe response should only contain a yaml of this structure:\n```yaml\nitems:\n  - name: REPLACE_NAME\n    description: REPLACE_DESCRIPTION\n```";
        let prompt = format!(
            "{}{scaffold}\nGenerate flavor for exactly {expected} items.",
            serde_yaml::to_string(&MerchantStockPrompt {
                merchant_name,
                merchant_theme,
                items: seeds,
            })?,
        );
        tracing::info!(merchant = %merchant_name, items = expected, "generating merchant stock flavor");
        let response = prompt_parse_retry_with_list_count::<MerchantStockResponse>(
            &self.stock_agent,
            &self.memory,
            &prompt,
            "items",
            expected,
            &format!("merchant_stock_{}", merchant_name.replace(' ', "_")),
        )
        .await?;
        tracing::info!(merchant = %merchant_name, items = response.items.len(), "merchant stock flavor received");
        Ok(response.items)
    }

    pub async fn build_catalog(
        &self,
        registry: &mut ItemRegistry,
        roll_seeds_for_merchant: impl Fn(usize) -> Vec<ItemSeed>,
    ) -> anyhow::Result<MerchantCatalog> {
        let profiles = self.generate_profiles(MERCHANT_ROSTER_SIZE).await?;
        let mut merchants = Vec::with_capacity(profiles.len());

        for (index, profile) in profiles.into_iter().enumerate() {
            let seeds = roll_seeds_for_merchant(index);
            let flavors = self
                .generate_stock_flavor(&profile.name, &profile.theme, &seeds)
                .await?;

            let mut stock_pool = Vec::with_capacity(seeds.len());
            let mut rng = rand::rng();
            for (seed, flavor) in seeds.into_iter().zip(flavors) {
                let mut item = seed.into_item(flavor.name, flavor.description);
                item.value = roll_item_value(&item.stats, &item.rarity, &mut rng);
                let id = registry.add_item(item);
                stock_pool.push(id);
            }

            tracing::info!(
                merchant = %profile.name,
                items = stock_pool.len(),
                "merchant stock items generated"
            );

            merchants.push(MerchantDefinition {
                name: profile.name,
                theme: profile.theme,
                stock_pool,
            });
        }

        let item_count: usize = merchants.iter().map(|m| m.stock_pool.len()).sum();
        tracing::info!(
            merchants = merchants.len(),
            items = item_count,
            "merchant item generation complete"
        );

        Ok(MerchantCatalog { merchants })
    }
}

/// Like `prompt_parse_retry` but validates a named YAML list has the expected length.
pub async fn prompt_parse_retry_with_list_count<T: serde::de::DeserializeOwned>(
    agent: &OpenRouterAgent,
    memory: &GameConversationMemory,
    prompt: &str,
    list_key: &str,
    expected_count: usize,
    conversation_id: &str,
) -> anyhow::Result<T> {
    use rig::completion::message::Message;

    use crate::game::generation::generators::{
        normalize_yaml_string_values, prompt_with_retry, truncate_for_log, yaml_correction,
    };

    const MAX_RETRIES: u32 = 5;
    let mut retries = 0u32;
    let mut correction = String::new();
    loop {
        let history = memory.load(conversation_id).await.unwrap_or_default();
        let effective_prompt = if correction.is_empty() {
            prompt.to_string()
        } else {
            format!("{prompt}\n\n{correction}")
        };
        let raw = prompt_with_retry(agent, &effective_prompt, &history).await?;
        let parsed = serde_yaml::from_str::<serde_yaml::Value>(&raw);
        match parsed {
            Err(e) => {
                if retries < MAX_RETRIES {
                    retries += 1;
                    let preview = truncate_for_log(&raw, 500);
                    tracing::warn!(
                        error = %e,
                        retries,
                        MAX_RETRIES,
                        "malformed YAML from LLM, retrying"
                    );
                    correction = yaml_correction("malformed YAML", &e.to_string(), &preview);
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "LLM returned malformed YAML after {} retries: {}",
                    MAX_RETRIES,
                    e
                ));
            }
            Ok(mut value) => {
                normalize_yaml_string_values(&mut value);
                let actual = value
                    .get(list_key)
                    .and_then(|t| t.as_sequence())
                    .map(|s| s.len());
                if actual != Some(expected_count) {
                    if retries < MAX_RETRIES {
                        retries += 1;
                        let preview = truncate_for_log(&raw, 500);
                        tracing::warn!(
                            expected_count,
                            actual = ?actual,
                            list_key,
                            retries,
                            MAX_RETRIES,
                            "wrong list count from LLM, retrying"
                        );
                        let msg =
                            format!("expected {expected_count} {list_key} entries, got {:?}", actual);
                        correction = yaml_correction("wrong list count", &msg, &preview);
                        continue;
                    }
                    return Err(anyhow::anyhow!(
                        "LLM returned wrong {list_key} count after {} retries: expected {}, got {:?}",
                        MAX_RETRIES,
                        expected_count,
                        actual
                    ));
                }
                match serde_yaml::from_value(value) {
                    Ok(result) => {
                        let _ = memory
                            .append(
                                conversation_id,
                                vec![Message::user(prompt), Message::assistant(&raw)],
                            )
                            .await;
                        tracing::info!(conversation_id, "committed to memory");
                        return Ok(result);
                    }
                    Err(e) => {
                        if retries < MAX_RETRIES {
                            retries += 1;
                            let preview = truncate_for_log(&raw, 500);
                            correction =
                                yaml_correction("unexpected YAML structure", &e.to_string(), &preview);
                            continue;
                        }
                        return Err(anyhow::anyhow!(
                            "LLM returned unexpected YAML structure after {} retries: {}",
                            MAX_RETRIES,
                            e
                        ));
                    }
                }
            }
        }
    }
}
