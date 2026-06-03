use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use tokio::sync::{Mutex, MutexGuard};

use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::Player;
use crate::game::persistence::chudmasters::Chudmasters;
use crate::game::persistence::message_cache::MessageCache;

const JOB_QUEUE_PATH: &str = "data/job_queue.yaml";
const PLAYERS_DIR: &str = "data/players";
const BOARD_PATH: &str = "data/board.yaml";
const HOSPITAL_PATH: &str = "data/hospital.yaml";
const MESSAGE_CACHE_PATH: &str = "data/message_cache.yaml";
const CHUDMASTERS_PATH: &str = "data/chudmasters.yaml";
const ITEM_REGISTRY_PATH: &str = "data/item_registry.yaml";

static MESSAGE_CACHE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Serialize all reads/writes of [`MessageCache`] on disk.
pub async fn message_cache_lock() -> MutexGuard<'static, ()> {
    MESSAGE_CACHE_LOCK.lock().await
}

/// Persist a player to `data/players/<discord_user_id>.yaml`.
pub fn save_player(player: &Player) -> anyhow::Result<()> {
    std::fs::create_dir_all(PLAYERS_DIR)?;
    let path = format!("{}/{}.yaml", PLAYERS_DIR, player.discord_user_id);
    let yaml = serde_yaml::to_string(player).context("serializing player")?;
    std::fs::write(&path, yaml)
        .with_context(|| format!("writing player {}", player.discord_user_id))
}

/// Load a player by Discord user ID, or `None` if they don't exist yet.
pub fn load_player(discord_user_id: u64) -> anyhow::Result<Option<Player>> {
    let path = format!("{}/{}.yaml", PLAYERS_DIR, discord_user_id);
    if !Path::new(&path).exists() {
        return Ok(None);
    }
    let yaml = std::fs::read_to_string(&path)
        .with_context(|| format!("reading player {}", discord_user_id))?;
    let player = serde_yaml::from_str(&yaml)
        .with_context(|| format!("parsing player {}", discord_user_id))?;
    Ok(Some(player))
}

/// Persist the quest board to `data/board.yaml`.
pub fn save_board(board: &Board) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(board).context("serializing board")?;
    std::fs::write(BOARD_PATH, yaml).context("writing board")
}

/// Load the quest board, returning an empty board if no file exists yet.
pub fn load_board() -> anyhow::Result<Board> {
    if !Path::new(BOARD_PATH).exists() {
        return Ok(Board::default());
    }
    let yaml = std::fs::read_to_string(BOARD_PATH).context("reading board")?;
    serde_yaml::from_str(&yaml).context("parsing board")
}

/// Persist the hospital to `data/hospital.yaml`.
pub fn save_hospital(hospital: &Hospital) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(hospital).context("serializing hospital")?;
    std::fs::write(HOSPITAL_PATH, yaml).context("writing hospital")
}

/// Load the hospital from disk, or return empty if no file exists.
pub fn load_hospital() -> anyhow::Result<Hospital> {
    if !Path::new(HOSPITAL_PATH).exists() {
        return Ok(Hospital::default());
    }
    let yaml = std::fs::read_to_string(HOSPITAL_PATH).context("reading hospital")?;
    serde_yaml::from_str(&yaml).context("parsing hospital")
}

/// Persist the job queue to `data/job_queue.yaml`.
pub fn save_job_queue(queue: &JobQueue) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(queue).context("serializing job queue")?;
    std::fs::write(JOB_QUEUE_PATH, yaml).context("writing job queue")
}

/// Load the job queue, returning empty if no file exists yet.
pub fn load_job_queue() -> anyhow::Result<JobQueue> {
    if !Path::new(JOB_QUEUE_PATH).exists() {
        return Ok(JobQueue::default());
    }
    let yaml = std::fs::read_to_string(JOB_QUEUE_PATH).context("reading job queue")?;
    serde_yaml::from_str(&yaml).context("parsing job queue")
}

/// Return the Discord user IDs of all players that have a save file.
pub fn list_player_ids() -> anyhow::Result<Vec<u64>> {
    if !Path::new(PLAYERS_DIR).exists() {
        return Ok(vec![]);
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(PLAYERS_DIR).context("reading players dir")? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            if let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u64>().ok())
            {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

/// Delete a player's save file. Returns an error if the file does not exist.
pub fn delete_player(discord_user_id: u64) -> anyhow::Result<()> {
    let path = format!("{}/{}.yaml", PLAYERS_DIR, discord_user_id);
    if !Path::new(&path).exists() {
        anyhow::bail!("no chud found for user {}", discord_user_id);
    }
    std::fs::remove_file(&path)
        .with_context(|| format!("deleting player {}", discord_user_id))
}

/// Persist the message cache to `data/message_cache.yaml`.
pub fn save_message_cache(cache: &MessageCache) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(cache).context("serializing message cache")?;
    std::fs::write(MESSAGE_CACHE_PATH, yaml).context("writing message cache")
}

/// Load the message cache, returning an empty cache if no file exists yet.
pub fn load_message_cache() -> anyhow::Result<MessageCache> {
    if !Path::new(MESSAGE_CACHE_PATH).exists() {
        return Ok(MessageCache::default());
    }
    let yaml = std::fs::read_to_string(MESSAGE_CACHE_PATH).context("reading message cache")?;
    serde_yaml::from_str(&yaml).context("parsing message cache")
}

/// Persist the chudmasters list to `data/chudmasters.yaml`.
pub fn save_chudmasters(chudmasters: &Chudmasters) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(chudmasters).context("serializing chudmasters")?;
    std::fs::write(CHUDMASTERS_PATH, yaml).context("writing chudmasters")
}

/// Load the chudmasters list, returning an empty list if no file exists yet.
pub fn load_chudmasters() -> anyhow::Result<Chudmasters> {
    if !Path::new(CHUDMASTERS_PATH).exists() {
        return Ok(Chudmasters::default());
    }
    let yaml = std::fs::read_to_string(CHUDMASTERS_PATH).context("reading chudmasters")?;
    serde_yaml::from_str(&yaml).context("parsing chudmasters")
}

/// Load the item registry, returning empty if no file exists yet.
pub fn load_item_registry() -> anyhow::Result<ItemRegistry> {
    if !Path::new(ITEM_REGISTRY_PATH).exists() {
        return Ok(ItemRegistry::default());
    }
    let yaml = std::fs::read_to_string(ITEM_REGISTRY_PATH).context("reading item registry")?;
    let mut registry: ItemRegistry = serde_yaml::from_str(&yaml).context("parsing item registry")?;
    if registry.backfill_missing_values() {
        save_item_registry(&registry)?;
    }
    Ok(registry)
}

/// Persist the item registry to `data/item_registry.yaml`.
pub fn save_item_registry(registry: &ItemRegistry) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(registry).context("serializing item registry")?;
    std::fs::write(ITEM_REGISTRY_PATH, yaml).context("writing item registry")
}

/// Return `true` if the given Discord user ID is a Chudmaster.
pub fn is_chudmaster(discord_user_id: u64) -> anyhow::Result<bool> {
    let cms = load_chudmasters()?;
    Ok(cms.ids.contains(&discord_user_id))
}

/// Add a Discord user ID to the chudmasters list. Returns `false` if already present.
pub fn add_chudmaster(discord_user_id: u64) -> anyhow::Result<bool> {
    let mut cms = load_chudmasters()?;
    if cms.ids.contains(&discord_user_id) {
        return Ok(false);
    }
    cms.ids.push(discord_user_id);
    save_chudmasters(&cms)?;
    Ok(true)
}

/// Remove a Discord user ID from the chudmasters list. Returns `false` if not found.
pub fn remove_chudmaster(discord_user_id: u64) -> anyhow::Result<bool> {
    let mut cms = load_chudmasters()?;
    let before = cms.ids.len();
    cms.ids.retain(|&id| id != discord_user_id);
    if cms.ids.len() == before {
        return Ok(false);
    }
    save_chudmasters(&cms)?;
    Ok(true)
}
