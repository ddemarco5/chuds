use std::sync::{LazyLock, RwLock};

use anyhow::Context;
use serde::Deserialize;

use crate::game::domain::item::ItemStats;

const STORY_JOBS_PATH: &str = "data/story_jobs.yaml";
const VALID_RARITIES: &[&str] = &["common", "uncommon", "rare", "exceptional"];

#[derive(Debug, Deserialize)]
struct StoryJobsFile {
    story_line_name: String,
    stories: Vec<StoryEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryEntryRaw {
    description: String,
    goal: String,
    difficulty: u8,
    #[serde(default)]
    reward: Option<StoryRewardRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryRewardRaw {
    rarity: String,
    stats: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct StoryCatalog {
    pub story_line_name: String,
    pub stories: Vec<StoryEntry>,
}

#[derive(Debug, Clone)]
pub struct StoryEntry {
    pub description: String,
    pub goal: String,
    pub difficulty: u8,
    /// Omitted for the final story mission (game ends on completion).
    pub reward: Option<StoryReward>,
}

#[derive(Debug, Clone)]
pub struct StoryReward {
    pub rarity: String,
    pub stats: Option<ItemStats>,
}

fn parse_stats(raw: &[String]) -> ItemStats {
    assert!(
        raw.len() == 3,
        "story job reward stats must be exactly 3 values [strength, smarts, stealth]"
    );
    ItemStats {
        strength: raw[0].clone(),
        smarts: raw[1].clone(),
        stealth: raw[2].clone(),
    }
}

fn validate_rarity(rarity: &str) -> String {
    let normalized = rarity.to_ascii_lowercase();
    assert!(
        VALID_RARITIES.contains(&normalized.as_str()),
        "invalid story reward rarity '{rarity}': expected one of common, uncommon, rare, exceptional"
    );
    normalized
}

fn parse_catalog(yaml: &str) -> anyhow::Result<StoryCatalog> {
    let file: StoryJobsFile = serde_yaml::from_str(yaml).context("parsing story_jobs.yaml")?;
    Ok(StoryCatalog {
        story_line_name: file.story_line_name,
        stories: file
            .stories
            .into_iter()
            .map(|entry| {
                anyhow::ensure!(
                    (1..=10).contains(&entry.difficulty),
                    "story job difficulty must be between 1 and 10"
                );
                Ok(StoryEntry {
                    description: entry.description,
                    goal: entry.goal,
                    difficulty: entry.difficulty,
                    reward: entry.reward.map(|reward| StoryReward {
                        rarity: validate_rarity(&reward.rarity),
                        stats: reward.stats.as_deref().map(parse_stats),
                    }),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    })
}

fn load_from_disk() -> anyhow::Result<StoryCatalog> {
    let yaml = std::fs::read_to_string(STORY_JOBS_PATH)
        .with_context(|| format!("reading {STORY_JOBS_PATH}"))?;
    parse_catalog(&yaml)
}

static CATALOG: LazyLock<RwLock<StoryCatalog>> = LazyLock::new(|| {
    RwLock::new(
        load_from_disk().unwrap_or_else(|e| {
            panic!("failed to load {STORY_JOBS_PATH}: {e}");
        }),
    )
});

/// Re-read `data/story_jobs.yaml` from disk so story progress checks use the current catalog length.
pub fn reload() -> anyhow::Result<()> {
    let fresh = load_from_disk()?;
    *CATALOG
        .write()
        .expect("story catalog lock poisoned") = fresh;
    Ok(())
}

pub fn story_line_name() -> String {
    CATALOG
        .read()
        .expect("story catalog lock poisoned")
        .story_line_name
        .clone()
}

pub fn story_count() -> usize {
    CATALOG
        .read()
        .expect("story catalog lock poisoned")
        .stories
        .len()
}

pub fn story_entry(index: usize) -> Option<StoryEntry> {
    CATALOG
        .read()
        .expect("story catalog lock poisoned")
        .stories
        .get(index)
        .cloned()
}

pub fn story_reward(index: usize) -> Option<StoryReward> {
    story_entry(index).and_then(|entry| entry.reward)
}
