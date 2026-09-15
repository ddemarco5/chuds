use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use rig::completion::message::Message;
use serde::{Deserialize, Serialize};

use crate::game::generation::gravestone_generator::GravestoneGenerator;
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::memory::LlmMemorySlot;
use crate::game::generation::merchant_generator::MerchantGenerator;
use crate::game::generation::quest_generator::QuestGenerator;

const LLM_MEMORY_PATH: &str = "data/llm_memory.zst";
const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMemoryBundle {
    pub version: u32,
    pub description: HashMap<String, Vec<Message>>,
    pub trials: HashMap<String, Vec<Message>>,
    pub results: HashMap<String, Vec<Message>>,
    pub item: HashMap<String, Vec<Message>>,
    pub gravestone: HashMap<String, Vec<Message>>,
    pub merchant: HashMap<String, Vec<Message>>,
    pub story_description: HashMap<String, Vec<Message>>,
    pub story_trials: HashMap<String, Vec<Message>>,
}

impl Default for LlmMemoryBundle {
    fn default() -> Self {
        Self {
            version: BUNDLE_VERSION,
            description: HashMap::new(),
            trials: HashMap::new(),
            results: HashMap::new(),
            item: HashMap::new(),
            gravestone: HashMap::new(),
            merchant: HashMap::new(),
            story_description: HashMap::new(),
            story_trials: HashMap::new(),
        }
    }
}

/// Load persisted LLM memory from disk, or return an empty bundle if missing or corrupt.
pub fn load_llm_memory_bundle() -> LlmMemoryBundle {
    if !Path::new(LLM_MEMORY_PATH).exists() {
        tracing::info!(path = LLM_MEMORY_PATH, "no LLM memory file, starting fresh");
        return LlmMemoryBundle::default();
    }
    match try_load_llm_memory_bundle() {
        Ok(bundle) => bundle,
        Err(e) => {
            tracing::warn!(err = %e, path = LLM_MEMORY_PATH, "failed to load LLM memory, starting fresh");
            LlmMemoryBundle::default()
        }
    }
}

fn try_load_llm_memory_bundle() -> anyhow::Result<LlmMemoryBundle> {
    let compressed = std::fs::read(LLM_MEMORY_PATH).context("reading LLM memory file")?;
    let compressed_bytes = compressed.len();
    let json = zstd::decode_all(compressed.as_slice()).context("decompressing LLM memory")?;
    let json_bytes = json.len();
    let bundle: LlmMemoryBundle =
        serde_json::from_slice(&json).context("deserializing LLM memory")?;
    if bundle.version != BUNDLE_VERSION {
        tracing::warn!(
            found = bundle.version,
            expected = BUNDLE_VERSION,
            path = LLM_MEMORY_PATH,
            "LLM memory bundle version mismatch, starting fresh"
        );
        return Ok(LlmMemoryBundle::default());
    }
    tracing::info!(
        path = LLM_MEMORY_PATH,
        compressed_bytes,
        json_bytes,
        description_msgs = bundle.description.values().map(|v| v.len()).sum::<usize>(),
        trials_msgs = bundle.trials.values().map(|v| v.len()).sum::<usize>(),
        results_msgs = bundle.results.values().map(|v| v.len()).sum::<usize>(),
        item_msgs = bundle.item.values().map(|v| v.len()).sum::<usize>(),
        gravestone_msgs = bundle.gravestone.values().map(|v| v.len()).sum::<usize>(),
        merchant_msgs = bundle.merchant.values().map(|v| v.len()).sum::<usize>(),
        story_description_msgs = bundle.story_description.values().map(|v| v.len()).sum::<usize>(),
        story_trials_msgs = bundle.story_trials.values().map(|v| v.len()).sum::<usize>(),
        "LLM memory loaded from disk"
    );
    Ok(bundle)
}

/// Persist an LLM memory bundle to disk (zstd-compressed JSON).
pub fn save_llm_memory_bundle(bundle: &LlmMemoryBundle) -> anyhow::Result<()> {
    std::fs::create_dir_all("data").context("creating data directory")?;
    let json = serde_json::to_vec(bundle).context("serializing LLM memory")?;
    let json_bytes = json.len();
    let compressed = zstd::encode_all(json.as_slice(), 3).context("compressing LLM memory")?;
    let compressed_bytes = compressed.len();
    std::fs::write(LLM_MEMORY_PATH, compressed).context("writing LLM memory file")?;
    tracing::info!(
        path = LLM_MEMORY_PATH,
        json_bytes,
        compressed_bytes,
        description_msgs = bundle.description.values().map(|v| v.len()).sum::<usize>(),
        trials_msgs = bundle.trials.values().map(|v| v.len()).sum::<usize>(),
        results_msgs = bundle.results.values().map(|v| v.len()).sum::<usize>(),
        item_msgs = bundle.item.values().map(|v| v.len()).sum::<usize>(),
        gravestone_msgs = bundle.gravestone.values().map(|v| v.len()).sum::<usize>(),
        merchant_msgs = bundle.merchant.values().map(|v| v.len()).sum::<usize>(),
        story_description_msgs = bundle.story_description.values().map(|v| v.len()).sum::<usize>(),
        story_trials_msgs = bundle.story_trials.values().map(|v| v.len()).sum::<usize>(),
        "LLM memory saved to disk"
    );
    Ok(())
}

fn bundle_from_generators(
    quest: &QuestGenerator,
    item: &ItemGenerator,
    gravestone: &GravestoneGenerator,
    merchant: &MerchantGenerator,
) -> LlmMemoryBundle {
    LlmMemoryBundle {
        version: BUNDLE_VERSION,
        description: quest
            .memory_for_slot(LlmMemorySlot::Description)
            .export_filtered_store(),
        trials: quest
            .memory_for_slot(LlmMemorySlot::Trials)
            .export_filtered_store(),
        results: quest
            .memory_for_slot(LlmMemorySlot::Results)
            .export_filtered_store(),
        story_description: quest
            .memory_for_slot(LlmMemorySlot::StoryDescription)
            .export_filtered_store(),
        story_trials: quest
            .memory_for_slot(LlmMemorySlot::StoryTrials)
            .export_filtered_store(),
        item: item.memory().export_filtered_store(),
        gravestone: gravestone.memory().export_filtered_store(),
        merchant: merchant.memory().export_filtered_store(),
    }
}

/// Snapshot all agent memories and write them to disk.
pub fn save_all_llm_memory(
    quest: &QuestGenerator,
    item: &ItemGenerator,
    gravestone: &GravestoneGenerator,
    merchant: &MerchantGenerator,
) -> anyhow::Result<()> {
    let bundle = bundle_from_generators(quest, item, gravestone, merchant);
    save_llm_memory_bundle(&bundle)
}

/// Clear in-memory LLM history for the given slot(s) and persist immediately.
pub fn clear_llm_memory(
    quest: &QuestGenerator,
    item: &ItemGenerator,
    gravestone: &GravestoneGenerator,
    merchant: &MerchantGenerator,
    slot: LlmMemorySlot,
) -> anyhow::Result<()> {
    match slot {
        LlmMemorySlot::Description => {
            quest
                .memory_for_slot(LlmMemorySlot::Description)
                .clear_all();
        }
        LlmMemorySlot::Trials => quest.memory_for_slot(LlmMemorySlot::Trials).clear_all(),
        LlmMemorySlot::StoryDescription => {
            quest
                .memory_for_slot(LlmMemorySlot::StoryDescription)
                .clear_all();
        }
        LlmMemorySlot::StoryTrials => {
            quest
                .memory_for_slot(LlmMemorySlot::StoryTrials)
                .clear_all();
        }
        LlmMemorySlot::Results => quest.memory_for_slot(LlmMemorySlot::Results).clear_all(),
        LlmMemorySlot::Item => item.memory().clear_all(),
        LlmMemorySlot::Gravestone => gravestone.memory().clear_all(),
        LlmMemorySlot::Merchant => merchant.memory().clear_all(),
        LlmMemorySlot::All => {
            quest
                .memory_for_slot(LlmMemorySlot::Description)
                .clear_all();
            quest.memory_for_slot(LlmMemorySlot::Trials).clear_all();
            quest
                .memory_for_slot(LlmMemorySlot::StoryDescription)
                .clear_all();
            quest
                .memory_for_slot(LlmMemorySlot::StoryTrials)
                .clear_all();
            quest.memory_for_slot(LlmMemorySlot::Results).clear_all();
            item.memory().clear_all();
            gravestone.memory().clear_all();
            merchant.memory().clear_all();
        }
    }
    tracing::info!(?slot, "LLM memory cleared");
    save_all_llm_memory(quest, item, gravestone, merchant)
}
