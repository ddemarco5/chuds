use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::game::domain::item::ItemSeed;
use crate::game::generation::generators::{
    build_agent, build_client, prompt_parse_retry, OpenRouterAgent,
};
use crate::game::generation::memory::{make_memory_from_store, GameConversationMemory};
use crate::game::merchant::{MerchantCatalog, MerchantDefinition, MERCHANT_ROSTER_SIZE};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::llm_memory::LlmMemoryBundle;
use crate::game::tuneable_rolls::roll_item_value;

fn merchant_profile_system_context() -> String {
    format!(
        r#"You are inventing traveling merchants who visit a Chud guild hall to sell gear.

Your task:
- Create distinct merchant shop identities for this world: {}
- Each merchant needs a plain 'name' (2-5 words, like "Old Ropey Pete" or "The Scrap Witch")
- Each merchant needs a 'theme': 1-2 sentences describing their shop vibe, specialty, and aesthetic (used later to name their inventory)

RULES:
- Merchants should feel varied: junk peddlers, suspicious alchemists, ex-adventurer flippers, etc.
- Do NOT use legendary proper-noun titles
- Avoid emdash use
- Output ONLY valid YAML
- Reply with ONLY valid YAML (no preamble). You may wrap in ```yaml fences."#,
        crate::story_jobs::world_setting()
    )
}

fn merchant_stock_system_context() -> String {
    format!(
        r#"You are naming and describing items sold by a traveling merchant.

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
- Match this world setting: {}
- Do NOT change item type, subtype, or stats
- Return exactly as many items as provided in the input, in the same order
- Avoid emdash use
- Output ONLY valid YAML with an 'items' list of {{name, description}} objects
- Reply with ONLY valid YAML (no preamble). You may wrap in ```yaml fences."#,
        crate::story_jobs::world_setting()
    )
}

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
        let client = build_client(api_key)?;
        Ok(Self {
            profile_agent: build_agent(&client, &merchant_profile_system_context()),
            stock_agent: build_agent(&client, &merchant_stock_system_context()),
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
        let started = Instant::now();
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
        tracing::debug!(
            count,
            elapsed_ms = started.elapsed().as_millis(),
            "merchant profiles received"
        );
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
        tracing::info!(merchant = %merchant_name, items = expected, "generating merchant stock");
        let started = Instant::now();
        let response = prompt_parse_retry::<MerchantStockResponse>(
            &self.stock_agent,
            &self.memory,
            &prompt,
            Some(("items", expected)),
            &format!("merchant_stock_{}", merchant_name.replace(' ', "_")),
        )
        .await?;
        tracing::info!(
            merchant = %merchant_name,
            items = response.items.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "merchant stock done"
        );
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

            merchants.push(MerchantDefinition {
                name: profile.name,
                theme: profile.theme,
                stock_pool,
            });
        }

        Ok(MerchantCatalog { merchants })
    }
}
