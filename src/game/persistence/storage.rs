use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::{Mutex, MutexGuard};

use crate::game::domain::board::Board;
use crate::game::domain::episode_stats::EpisodeStats;
use crate::game::domain::guild_hall::GuildHall;
use crate::game::merchant::MerchantState;
use crate::game::domain::graveyard::Graveyard;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::starting_benefits::StartingBenefits;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::merchants;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::Player;
use crate::game::domain::session::{GamePhase, GameSession};
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
const EPISODE_STATS_PATH: &str = "data/episode_stats.yaml";

static MESSAGE_CACHE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Serialize all reads/writes of [`MessageCache`] on disk.
pub async fn message_cache_lock() -> MutexGuard<'static, ()> {
    MESSAGE_CACHE_LOCK.lock().await
}

fn save_yaml<T: Serialize>(path: &str, value: &T, name: &str) -> anyhow::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(value).with_context(|| format!("serializing {name}"))?;
    std::fs::write(path, yaml).with_context(|| format!("writing {name}"))
}

fn load_yaml<T: DeserializeOwned>(path: &str, name: &str) -> anyhow::Result<T> {
    let yaml = std::fs::read_to_string(path).with_context(|| format!("reading {name}"))?;
    serde_yaml::from_str(&yaml).with_context(|| format!("parsing {name}"))
}

fn load_yaml_or_else<T: DeserializeOwned>(
    path: &str,
    name: &str,
    default: impl FnOnce() -> T,
) -> anyhow::Result<T> {
    if !Path::new(path).exists() {
        return Ok(default());
    }
    load_yaml(path, name)
}

fn load_yaml_or_default<T: DeserializeOwned + Default>(path: &str, name: &str) -> anyhow::Result<T> {
    load_yaml_or_else(path, name, T::default)
}

/// Persist a player to `data/players/<discord_user_id>.yaml`.
pub fn save_player(player: &Player) -> anyhow::Result<()> {
    let path = format!("{}/{}.yaml", PLAYERS_DIR, player.discord_user_id);
    save_yaml(&path, player, &format!("player {}", player.discord_user_id))
}

/// Load a player by Discord user ID, or `None` if they don't exist yet.
pub fn load_player(discord_user_id: u64) -> anyhow::Result<Option<Player>> {
    let path = format!("{}/{}.yaml", PLAYERS_DIR, discord_user_id);
    if !Path::new(&path).exists() {
        return Ok(None);
    }
    load_yaml(&path, &format!("player {discord_user_id}")).map(Some)
}

/// Persist the quest board to `data/board.yaml`.
pub fn save_board(board: &Board) -> anyhow::Result<()> {
    save_yaml(BOARD_PATH, board, "board")
}

/// Load the quest board, returning an empty board if no file exists yet.
pub fn load_board() -> anyhow::Result<Board> {
    load_yaml_or_else(BOARD_PATH, "board", fresh_board)
}

/// New board with story catalog length initialized from the current catalog.
pub fn fresh_board() -> Board {
    let mut board = Board::default();
    board.reconcile_story_catalog();
    board
}

/// Reconcile persisted story progress with the reloaded catalog and session phase.
pub fn resume_story_state(board: &mut Board, session: &mut GameSession) -> anyhow::Result<bool> {
    crate::story_jobs::reload()?;
    let old_catalog_len = board.story_catalog_len;
    let catalog_changed = board.reconcile_story_catalog();
    let mut changed = catalog_changed;
    if reconcile_session_story(session, board, catalog_changed, old_catalog_len) {
        changed = true;
    }
    if changed {
        save_board(board)?;
        save_session(session)?;
    }
    Ok(changed)
}

/// Repairs known-bad persisted board state after YAML load, before gameplay resumes.
/// Missing quest results are re-enqueued for generation. Add future resume-time fixes here.
pub fn reconcile_resumed_board(
    board: &mut Board,
    generation_queue: &tokio::sync::mpsc::UnboundedSender<crate::game::engine::GenerationJob>,
) -> anyhow::Result<()> {
    let enqueued = crate::game::engine::enqueue_missing_active_quest_results(board, generation_queue)?;
    if enqueued > 0 {
        tracing::info!(enqueued, "re-enqueued quest results on resume");
    }
    save_board(board)?;
    Ok(())
}

/// Undo Complete only when the story catalog grew after the player had already finished it.
/// Admin-complete and normal restarts while Complete are left alone.
pub fn reconcile_session_story(
    session: &mut GameSession,
    board: &Board,
    catalog_changed: bool,
    old_catalog_len: usize,
) -> bool {
    if session.phase != GamePhase::Complete {
        return false;
    }
    if board.story_series_complete() {
        return false;
    }
    let had_finished_old_catalog =
        old_catalog_len > 0 && board.story_next_index >= old_catalog_len;
    if !catalog_changed || !had_finished_old_catalog {
        return false;
    }
    tracing::warn!(
        story_next_index = board.story_next_index,
        old_catalog_len,
        new_catalog_len = board.story_catalog_len,
        "story catalog grew after the episode was complete; resuming Playing"
    );
    session.phase = GamePhase::Playing;
    session.final_completer_user_id = None;
    session.final_completer_chud_name = None;
    true
}

/// Persist the hospital to `data/hospital.yaml`.
pub fn save_hospital(hospital: &Hospital) -> anyhow::Result<()> {
    save_yaml(HOSPITAL_PATH, hospital, "hospital")
}

/// Load the hospital from disk, or return empty if no file exists.
pub fn load_hospital() -> anyhow::Result<Hospital> {
    load_yaml_or_default(HOSPITAL_PATH, "hospital")
}

/// Persist the graveyard to `data/graveyard.yaml`.
pub fn save_graveyard(graveyard: &Graveyard) -> anyhow::Result<()> {
    save_yaml(GRAVEYARD_PATH, graveyard, "graveyard")
}

/// Load the graveyard from disk, or return empty if no file exists.
pub fn load_graveyard() -> anyhow::Result<Graveyard> {
    load_yaml_or_default(GRAVEYARD_PATH, "graveyard")
}

/// Persist starting benefits to `data/starting_benefits.yaml`.
pub fn save_starting_benefits(benefits: &StartingBenefits) -> anyhow::Result<()> {
    save_yaml(STARTING_BENEFITS_PATH, benefits, "starting benefits")
}

/// Load starting benefits from disk, or return empty if no file exists.
pub fn load_starting_benefits() -> anyhow::Result<StartingBenefits> {
    load_yaml_or_default(STARTING_BENEFITS_PATH, "starting benefits")
}

/// Persist the job queue to `data/job_queue.yaml`.
pub fn save_job_queue(queue: &JobQueue) -> anyhow::Result<()> {
    save_yaml(JOB_QUEUE_PATH, queue, "job queue")
}

/// Load the job queue, returning empty if no file exists yet.
pub fn load_job_queue() -> anyhow::Result<JobQueue> {
    load_yaml_or_default(JOB_QUEUE_PATH, "job queue")
}

/// Load every saved player that currently has a chud. Missing files and parse
/// errors are skipped (with a warning) so a single bad save cannot blank the roster.
pub fn load_chuds() -> Vec<Player> {
    let ids = match list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players");
            return Vec::new();
        }
    };
    ids.into_iter()
        .filter_map(|id| match load_player(id) {
            Ok(Some(p)) if p.has_chud() => Some(p),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(discord_user_id = id, err = %e, "failed to load player");
                None
            }
        })
        .collect()
}

/// Load every saved player that currently has a chud, failing on IO/parse errors.
pub fn try_load_chuds() -> anyhow::Result<Vec<Player>> {
    let mut players = Vec::new();
    for id in list_player_ids()? {
        if let Some(player) = load_player(id)?.filter(|p| p.has_chud()) {
            players.push(player);
        }
    }
    Ok(players)
}

/// Load the player whose chud name matches `name` (case-insensitive), if any.
pub fn find_player_by_name(name: &str) -> anyhow::Result<Option<Player>> {
    let needle = name.trim();
    if needle.is_empty() {
        return Ok(None);
    }
    Ok(try_load_chuds()?
        .into_iter()
        .find(|player| player.chud_ref().name.eq_ignore_ascii_case(needle)))
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
    save_yaml(MESSAGE_CACHE_PATH, cache, "message cache")
}

/// Load the message cache, returning an empty cache if no file exists yet.
pub fn load_message_cache() -> anyhow::Result<MessageCache> {
    load_yaml_or_default(MESSAGE_CACHE_PATH, "message cache")
}

/// Persist the chudmasters list to `data/chudmasters.yaml`.
pub fn save_chudmasters(chudmasters: &Chudmasters) -> anyhow::Result<()> {
    save_yaml(CHUDMASTERS_PATH, chudmasters, "chudmasters")
}

/// Load the chudmasters list, returning an empty list if no file exists yet.
pub fn load_chudmasters() -> anyhow::Result<Chudmasters> {
    load_yaml_or_default(CHUDMASTERS_PATH, "chudmasters")
}

/// Load the item registry, returning empty if no file exists yet.
pub fn load_item_registry() -> anyhow::Result<ItemRegistry> {
    load_yaml_or_default(ITEM_REGISTRY_PATH, "item registry")
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
    save_yaml(GUILD_HALL_PATH, hall, "guild hall")
}

/// Load guild-hall state from disk, or return empty if no file exists.
pub fn load_guild_hall() -> anyhow::Result<GuildHall> {
    load_yaml_or_default(GUILD_HALL_PATH, "guild hall")
}

/// Persist the item registry to `data/item_registry.yaml`.
pub fn save_item_registry(registry: &ItemRegistry) -> anyhow::Result<()> {
    save_yaml(ITEM_REGISTRY_PATH, registry, "item registry")
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
    save_board(&fresh_board())?;
    save_job_queue(&JobQueue::default())?;
    save_hospital(&Hospital::default())?;
    save_graveyard(&Graveyard::default())?;
    save_starting_benefits(&StartingBenefits::default())?;
    save_guild_hall_full(&GuildHall::default())?;
    merchants::clear_merchants()?;
    save_item_registry(&ItemRegistry::default())?;
    save_session(&GameSession::default())?;
    save_episode_stats(&EpisodeStats::default())?;
    tracing::info!("game data reset to a fresh game");
    Ok(())
}

/// Persist the game-flow session to `data/session.yaml`.
pub fn save_session(session: &GameSession) -> anyhow::Result<()> {
    save_yaml(SESSION_PATH, session, "session")
}

/// Load the game-flow session, defaulting to `Playing` if no file exists yet.
pub fn load_session() -> anyhow::Result<GameSession> {
    load_yaml_or_default(SESSION_PATH, "session")
}

/// Persist episode stats to `data/episode_stats.yaml`.
pub fn save_episode_stats(stats: &EpisodeStats) -> anyhow::Result<()> {
    save_yaml(EPISODE_STATS_PATH, stats, "episode stats")
}

/// Load episode stats, returning defaults if no file exists yet.
pub fn load_episode_stats() -> anyhow::Result<EpisodeStats> {
    load_yaml_or_default(EPISODE_STATS_PATH, "episode stats")
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
