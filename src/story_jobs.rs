use std::sync::LazyLock;

use serde::Deserialize;

use crate::game::domain::item::ItemStats;

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

static CATALOG: LazyLock<StoryCatalog> = LazyLock::new(|| {
    let file: StoryJobsFile =
        serde_yaml::from_str(include_str!("story_jobs.yaml")).expect("invalid story_jobs.yaml");
    StoryCatalog {
        story_line_name: file.story_line_name,
        stories: file
            .stories
            .into_iter()
            .map(|entry| {
                assert!(
                    (1..=10).contains(&entry.difficulty),
                    "story job difficulty must be between 1 and 10"
                );
                StoryEntry {
                    description: entry.description,
                    goal: entry.goal,
                    difficulty: entry.difficulty,
                    reward: entry.reward.map(|reward| StoryReward {
                        rarity: validate_rarity(&reward.rarity),
                        stats: reward.stats.as_deref().map(parse_stats),
                    }),
                }
            })
            .collect(),
    }
});

pub fn catalog() -> &'static StoryCatalog {
    &CATALOG
}
