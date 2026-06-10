use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use tokio::sync::{Mutex, MutexGuard};

use crate::game::domain::board::Board;
use crate::game::domain::guild_hall::GuildHall;
use crate::game::merchant::MerchantState;
use crate::game::domain::graveyard::Graveyard;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::starting_benefits::StartingBenefits;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::merchants;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::Player;
use crate::game::domain::session::GameSession;
use crate::game::persistence::chudmasters::Chudmasters;
use crate::game::persistence::message_cache::MessageCache;

const JOB_QUEUE_PATH: &str = "data/job_queue.yaml";
const PLAYERS_DIR: &str = "data/players";
const BOARD_PATH: &str = "data/board.yaml";
const HOSPITAL_PATH: &str = "data/hospital.yaml";
const GRAVEYARD_PATH: &str = "data/graveyard.yaml";
const STARTING_BENEFITS_PATH: &str = "data/starting_benefits.yaml";
const MESSAGE_CACHE_PATH: &str = "data/message_cache.yaml";
const CHUDMASTERS_PATH: &str = "data/chudmasters.yaml";
const ITEM_REGISTRY_PATH: &str = "data/item_registry.yaml";
const GUILD_HALL_PATH: &str = "data/guild_hall.yaml";
const SESSION_PATH: &str = "data/session.yaml";

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

/// Persist the graveyard to `data/graveyard.yaml`.
pub fn save_graveyard(graveyard: &Graveyard) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(graveyard).context("serializing graveyard")?;
    std::fs::write(GRAVEYARD_PATH, yaml).context("writing graveyard")
}

/// Load the graveyard from disk, or return empty if no file exists.
pub fn load_graveyard() -> anyhow::Result<Graveyard> {
    if !Path::new(GRAVEYARD_PATH).exists() {
        return Ok(Graveyard::default());
    }
    let yaml = std::fs::read_to_string(GRAVEYARD_PATH).context("reading graveyard")?;
    serde_yaml::from_str(&yaml).context("parsing graveyard")
}

/// Persist starting benefits to `data/starting_benefits.yaml`.
pub fn save_starting_benefits(benefits: &StartingBenefits) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(benefits).context("serializing starting benefits")?;
    std::fs::write(STARTING_BENEFITS_PATH, yaml).context("writing starting benefits")
}

/// Load starting benefits from disk, or return empty if no file exists.
pub fn load_starting_benefits() -> anyhow::Result<StartingBenefits> {
    if !Path::new(STARTING_BENEFITS_PATH).exists() {
        return Ok(StartingBenefits::default());
    }
    let yaml = std::fs::read_to_string(STARTING_BENEFITS_PATH).context("reading starting benefits")?;
    serde_yaml::from_str(&yaml).context("parsing starting benefits")
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

/// Load the player whose chud name matches `name` (case-insensitive), if any.
pub fn find_player_by_name(name: &str) -> anyhow::Result<Option<Player>> {
    let needle = name.trim();
    if needle.is_empty() {
        return Ok(None);
    }
    for &id in &list_player_ids()? {
        if let Some(player) = load_player(id)? {
            if player.has_chud()
                && player.chud_ref().name.eq_ignore_ascii_case(needle)
            {
                return Ok(Some(player));
            }
        }
    }
    Ok(None)
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

/// Load merchant visit state and attach the roster from `data/merchants.yaml`.
/// Discards a stale roster or visit when registry IDs are missing (e.g. after reset).
pub fn load_merchant_state(registry: &ItemRegistry) -> anyhow::Result<MerchantState> {
    let mut merchant = load_guild_hall()?.merchant;
    let mut repaired = false;

    merchant.catalog = match merchants::load_merchants()? {
        Some(catalog) if merchants::catalog_matches_registry(&catalog, registry) => {
            Some(merchants::normalize_catalog(catalog))
        }
        Some(_) => {
            tracing::warn!("stale merchant roster (registry IDs missing); clearing");
            merchants::clear_merchants()?;
            repaired = true;
            None
        }
        None => None,
    };

    if let Some(catalog) = &merchant.catalog {
        if let Some(visit) = &mut merchant.visit {
            let visit_name = visit.merchant_name.clone();
            match catalog.merchants.iter().position(|m| m.name == visit_name) {
                Some(new_index) if visit.merchant_index != new_index => {
                    visit.merchant_index = new_index;
                    repaired = true;
                }
                None => {
                    tracing::warn!(merchant = %visit_name, "merchant visit target missing after normalize; clearing");
                    merchant.visit = None;
                    repaired = true;
                }
                _ => {}
            }
        }

        if let Some(visit) = &merchant.visit {
            if !merchants::visit_matches_registry(visit, registry) {
                tracing::warn!("stale merchant visit; clearing");
                merchant.visit = None;
                repaired = true;
            }
        }

        if let Err(e) = merchants::save_merchants(catalog) {
            tracing::warn!(err = %e, "failed to persist normalized merchant catalog");
        }
    }

    if repaired {
        save_guild_hall(&merchant)?;
    }

    Ok(merchant)
}

/// Persist merchant visit state and roster (guild hall + merchants file).
pub fn save_merchant_state(merchant: &MerchantState) -> anyhow::Result<()> {
    save_guild_hall(merchant)?;
    if let Some(catalog) = &merchant.catalog {
        merchants::save_merchants(catalog)?;
    }
    Ok(())
}

/// Persist guild-hall visit state to `data/guild_hall.yaml` (roster excluded).
pub fn save_guild_hall(merchant: &MerchantState) -> anyhow::Result<()> {
    save_guild_hall_full(&GuildHall {
        merchant: merchant.clone(),
    })
}

/// Persist guild-hall state to `data/guild_hall.yaml`.
pub fn save_guild_hall_full(hall: &GuildHall) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(hall).context("serializing guild hall")?;
    std::fs::write(GUILD_HALL_PATH, yaml).context("writing guild hall")
}

/// Load guild-hall state from disk, or return empty if no file exists.
pub fn load_guild_hall() -> anyhow::Result<GuildHall> {
    if !Path::new(GUILD_HALL_PATH).exists() {
        return Ok(GuildHall::default());
    }
    let yaml = std::fs::read_to_string(GUILD_HALL_PATH).context("reading guild hall")?;
    serde_yaml::from_str(&yaml).context("parsing guild hall")
}

/// Persist the item registry to `data/item_registry.yaml`.
pub fn save_item_registry(registry: &ItemRegistry) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(registry).context("serializing item registry")?;
    std::fs::write(ITEM_REGISTRY_PATH, yaml).context("writing item registry")
}

/// Reset all per-game state to a fresh game: deletes every player and resets the board,
/// queue, hospital, graveyard, starting benefits, merchant, item registry, and session.
///
/// Preserves chudmasters and learned LLM memory (those are not per-game state).
pub fn reset_game_data() -> anyhow::Result<()> {
    for id in list_player_ids()? {
        let path = format!("{}/{}.yaml", PLAYERS_DIR, id);
        if Path::new(&path).exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("deleting player {}", id))?;
        }
    }
    save_board(&Board::default())?;
    save_job_queue(&JobQueue::default())?;
    save_hospital(&Hospital::default())?;
    save_graveyard(&Graveyard::default())?;
    save_starting_benefits(&StartingBenefits::default())?;
    save_guild_hall_full(&GuildHall::default())?;
    merchants::clear_merchants()?;
    save_item_registry(&ItemRegistry::default())?;
    save_session(&GameSession::default())?;
    tracing::info!("game data reset to a fresh game");
    Ok(())
}

/// Persist the game-flow session to `data/session.yaml`.
pub fn save_session(session: &GameSession) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let yaml = serde_yaml::to_string(session).context("serializing session")?;
    std::fs::write(SESSION_PATH, yaml).context("writing session")
}

/// Load the game-flow session, defaulting to `Playing` if no file exists yet.
pub fn load_session() -> anyhow::Result<GameSession> {
    if !Path::new(SESSION_PATH).exists() {
        return Ok(GameSession::default());
    }
    let yaml = std::fs::read_to_string(SESSION_PATH).context("reading session")?;
    serde_yaml::from_str(&yaml).context("parsing session")
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
