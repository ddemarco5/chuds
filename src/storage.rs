use std::path::Path;

use anyhow::Context;

use crate::board::Board;
use crate::chudmasters::Chudmasters;
use crate::message_cache::MessageCache;
use crate::player::Player;

const PLAYERS_DIR: &str = "data/players";
const BOARD_PATH: &str = "data/board.yaml";
const MESSAGE_CACHE_PATH: &str = "data/message_cache.yaml";
const CHUDMASTERS_PATH: &str = "data/chudmasters.yaml";

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

/// Return the Discord user IDs of all players that have a save file.
pub fn list_player_ids() -> anyhow::Result<Vec<u64>> {
    if !Path::new(PLAYERS_DIR).exists() {
        return Ok(vec![]);
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(PLAYERS_DIR).context("reading players dir")? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            if let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()) {
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

/// Load the quest board, returning an empty board if no file exists yet.
pub fn load_board() -> anyhow::Result<Board> {
    if !Path::new(BOARD_PATH).exists() {
        return Ok(Board::default());
    }
    let yaml = std::fs::read_to_string(BOARD_PATH).context("reading board")?;
    serde_yaml::from_str(&yaml).context("parsing board")
}

