use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::graveyard::{Graveyard, GraveyardEntry};
use crate::game::domain::hospital::Hospital;
use crate::game::domain::starting_benefits::StartingBenefits;
use crate::game::guild_status::GuildHallStatus;
use crate::game::domain::item::{EquipmentSlot, ItemType};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::{JobQueue, QueuedQuest};
use crate::game::domain::player::{create_chud, Player};
use crate::game::domain::quest_result::QuestResult;
use crate::game::generation::gravestone_generator::GravestoneGenerator;
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData, QuestGenerator, QuestResults, TrialResult};
use crate::game::merchant::{catalog_needs_generated_roster, MerchantState};
use crate::game::tuneable_rolls::{
    death_chance, injury_chance, item_drop_chance, roll_auto_job_difficulty,
    roll_failure_consequences, roll_item, roll_item_drop, roll_item_value, roll_trials,
};
use crate::story_jobs;
use crate::game::mechanics::quest_builder::PlayedQuest;
use crate::game::mechanics::simulation::{effective_stats, play_quest};
use crate::game::tuneable_rolls::FailureConsequences;
use crate::game::persistence::storage;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Context about what killed a chud, used for epitaph generation.
#[derive(Clone)]
pub struct DeathContext {
    pub trial: String,
    pub outcome: String,
}

pub struct KillResult {
    pub discord_user_id: u64,
    pub chud_name: String,
    pub benefits_awarded: u32,
    pub epitaph: String,
}

/// Info returned after a chud accepts a quest.
pub struct AssignInfo {
    pub player: Player,
    pub quest_title: String,
}

/// Result of creating (or respawning) a chud.
pub struct AddChudResult {
    pub player: Player,
    /// Inherited starting-benefit cash claimed on creation (0 if none pending).
    pub benefits_claimed: u32,
}

/// A pending generation job sent to the background worker.
pub enum GenerationJob {
    /// Generate the LLM result narrative for an already-assigned quest.
    QuestResult {
        board_quest: BoardQuest,
        player: Player,
        force_item_drop: bool,
    },
    /// Fully generate a new quest from a description and add it to the board.
    QuestCreation {
        quest_data: QuestData,
        /// Some(catalog index) for story jobs; None for regular jobs.
        story_index: Option<usize>,
    },
    /// Generate the merchant roster and stock pools for a fresh game.
    MerchantCatalog,
}

/// Enqueue background quest result generation after assignment.
pub fn enqueue_quest_result(
    generation_queue: &tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    board_quest: BoardQuest,
    player: Player,
    force_item_drop: bool,
) -> anyhow::Result<()> {
    generation_queue
        .send(GenerationJob::QuestResult {
            board_quest,
            player,
            force_item_drop,
        })
        .map_err(|e| anyhow::anyhow!("generation queue closed: {e}"))
}

/// Assign a chud to a quest and enqueue result generation.
pub fn take_and_enqueue_quest(
    board: &mut Board,
    hospital: &Hospital,
    discord_user_id: u64,
    quest_id: u32,
    generation_queue: &tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    force_item_drop: bool,
    pre_assign_status: Option<&GuildHallStatus>,
    job_timeout_tick: u32,
) -> anyhow::Result<AssignInfo> {
    let info = assign_chud_to_quest(
        board,
        hospital,
        discord_user_id,
        quest_id,
        pre_assign_status,
        job_timeout_tick,
    )?;
    let board_quest = board
        .quests
        .iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found after assignment", quest_id))?
        .clone();
    enqueue_quest_result(generation_queue, board_quest, info.player.clone(), force_item_drop)?;
    Ok(info)
}

/// Ensure the next story job is being generated. Enqueues a story `QuestCreation`
/// when a story slot is open (none currently on the board and catalog entries remain)
/// and no generation is already in flight. Returns true if a job was enqueued.
pub fn ensure_story_generation(
    board: &Board,
    generation_queue: Option<&tokio::sync::mpsc::UnboundedSender<GenerationJob>>,
    pending_quests: Option<&AtomicUsize>,
) -> bool {
    let catalog_len = story_jobs::story_count();
    if catalog_len == 0
        || board.story_next_index >= catalog_len
        || board.has_story_quest()
    {
        return false;
    }

    let (Some(gen_q), Some(pending)) = (generation_queue, pending_quests) else {
        return false;
    };
    if pending.load(Ordering::SeqCst) != 0 {
        return false;
    }

    let index = board.story_next_index;
    let Some(entry) = story_jobs::story_entry(index) else {
        return false;
    };
    pending.fetch_add(1, Ordering::SeqCst);
    if gen_q
        .send(GenerationJob::QuestCreation {
            quest_data: QuestData {
                quest_description: entry.description.clone(),
                quest_goal: Some(entry.goal.clone()),
                quest_difficulty: entry.difficulty,
                trials: roll_trials(entry.difficulty),
            },
            story_index: Some(index),
        })
        .is_err()
    {
        pending.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

/// Enqueue merchant catalog generation when the generated roster is missing and none is in flight.
pub fn ensure_merchant_catalog_generation(
    merchant: &MerchantState,
    generation_queue: Option<&tokio::sync::mpsc::UnboundedSender<GenerationJob>>,
    pending: Option<&AtomicUsize>,
) -> bool {
    if merchant
        .catalog
        .as_ref()
        .is_some_and(|c| !catalog_needs_generated_roster(c))
    {
        return false;
    }

    let (Some(gen_q), Some(pending)) = (generation_queue, pending) else {
        return false;
    };
    if pending.load(Ordering::SeqCst) != 0 {
        return false;
    }

    pending.fetch_add(1, Ordering::SeqCst);
    if gen_q.send(GenerationJob::MerchantCatalog).is_err() {
        pending.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

/// Decrement idle regular job timeouts, remove expired jobs, return them for re-queue.
pub fn drain_expired_board_jobs(board: &mut Board) -> Vec<QueuedQuest> {
    let mut expired = Vec::new();
    board.quests.retain_mut(|q| {
        if !q.states.is_empty() || q.is_story() {
            return true;
        }
        q.timeout = q.timeout.saturating_sub(1);
        if q.timeout == 0 {
            expired.push(QueuedQuest {
                quest_data: q.quest_data.clone(),
                generated: q.generated.clone(),
            });
            false
        } else {
            true
        }
    });
    expired
}

/// Refill the board: ensure a story job is present (via async generation) before
/// pulling from the regular queue. Returns the number of queue jobs added.
pub fn refill_board(
    board: &mut Board,
    queue: &mut JobQueue,
    max_jobs: usize,
    job_timeout_tick: u32,
    generation_queue: Option<&tokio::sync::mpsc::UnboundedSender<GenerationJob>>,
    pending_quests: Option<&AtomicUsize>,
) -> usize {
    // Story job takes priority: keep its slot reserved while it is generated from the
    // catalog (bootstrap/recovery fallback) before filling from the regular queue.
    let catalog_len = story_jobs::story_count();
    if catalog_len > 0
        && board.story_next_index < catalog_len
        && !board.has_story_quest()
    {
        ensure_story_generation(board, generation_queue, pending_quests);
        return 0;
    }

    // Regular jobs: pull pre-generated quests from the chudmaster queue. Selection is an
    // age-weighted random draw rather than strict FIFO so that contiguous blocks of
    // auto-generated jobs do not come off the queue in a row. Older entries (toward the
    // front) get proportionally higher weight, and a waiting entry's weight rises as
    // others are consumed, so nothing starves.
    let mut added = 0usize;
    let mut rng = rand::thread_rng();
    while board.quests.len() < max_jobs && !queue.entries.is_empty() {
        let index = weighted_oldest_index(queue.entries.len(), &mut rng);
        let entry = queue.entries.remove(index);
        board.add_quest(entry.quest_data, entry.generated, None, job_timeout_tick);
        added += 1;
    }
    if added > 0 {
        tracing::info!(added, "board refilled from queue");
    }
    added
}

/// Pick an index into a `len`-long queue with linear age weighting: the oldest entry
/// (front, index 0) has weight `len`, the newest (back) has weight 1. Older jobs are
/// favored while any entry can still be drawn.
fn weighted_oldest_index(len: usize, rng: &mut impl rand::Rng) -> usize {
    debug_assert!(len > 0);
    // Total weight = len + (len-1) + ... + 1 = len * (len + 1) / 2.
    let total = len * (len + 1) / 2;
    let mut pick = rng.gen_range(0..total);
    for index in 0..len {
        let weight = len - index;
        if pick < weight {
            return index;
        }
        pick -= weight;
    }
    len - 1
}

/// Push a fully generated quest onto the job queue and persist.
pub fn enqueue_quest(
    queue: &mut JobQueue,
    quest_data: QuestData,
    generated: GeneratedQuest,
) -> anyhow::Result<()> {
    queue.push(quest_data, generated);
    storage::save_job_queue(queue)
}

/// Mean stat level assumed when no chuds exist yet (matches flat starting stats).
pub const DEFAULT_MEAN_CHUD_STAT: f64 = 1.0;

/// Mean of each chud's two highest effective stats (base + item modifiers), averaged across
/// all saved players. Falls back to [`DEFAULT_MEAN_CHUD_STAT`] when there are no chuds yet.
pub fn mean_chud_stat(registry: &ItemRegistry) -> f64 {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for mean stat; using default");
            return DEFAULT_MEAN_CHUD_STAT;
        }
    };

    let mut chud_means: Vec<f64> = Vec::new();
    for id in ids {
        match storage::load_player(id) {
            Ok(Some(player)) => {
                if player.chud.is_some() {
                    let (str, smt, sth) = effective_stats(&player, registry);
                    let mut stats = [str, smt, sth];
                    stats.sort_by(|a, b| b.cmp(a));
                    chud_means.push((stats[0] as f64 + stats[1] as f64) / 2.0);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(err = %e, player = id, "failed to load player for mean stat"),
        }
    }

    if chud_means.is_empty() {
        DEFAULT_MEAN_CHUD_STAT
    } else {
        chud_means.iter().sum::<f64>() / chud_means.len() as f64
    }
}

/// Mean effective stat level used as the center for auto job difficulty rolls.
pub(crate) fn auto_job_difficulty_center(registry: &ItemRegistry) -> f64 {
    mean_chud_stat(registry)
}

/// Difficulty for a new job when the chudmaster leaves the field blank: a broad normal roll
/// centered on the mean top-two effective stat level across all chuds.
pub fn roll_job_difficulty() -> u8 {
    let registry = match storage::load_item_registry() {
        Ok(registry) => registry,
        Err(e) => {
            tracing::warn!(err = %e, "failed to load item registry for difficulty roll; using empty");
            ItemRegistry::default()
        }
    };
    let mut rng = rand::thread_rng();
    roll_auto_job_difficulty(auto_job_difficulty_center(&registry), &mut rng)
}

/// Build a [`GenerationJob::QuestCreation`] from a raw description and difficulty.
pub fn make_quest_creation_job(
    description: String,
    difficulty: u8,
    quest_goal: Option<String>,
) -> GenerationJob {
    let quest_data = QuestData {
        quest_description: description,
        quest_goal,
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty),
    };
    GenerationJob::QuestCreation {
        quest_data,
        story_index: None, // chudmaster /generate_job — goes to queue after generation
    }
}

/// Generate trial flavor via LLM and enqueue the quest (does not add directly to board).
pub async fn generate_written_job(
    generator: &QuestGenerator,
    title: String,
    giver: String,
    description: String,
    goal: String,
    difficulty: u8,
) -> anyhow::Result<(QuestData, GeneratedQuest)> {
    let quest_data = QuestData {
        quest_description: description,
        quest_goal: Some(goal),
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty),
    };
    let generated = generator
        .generate_from_explicit(&quest_data, title, giver)
        .await?;
    tracing::info!(
        title = %generated.quest_title,
        giver = %generated.quest_giver,
        "quest written"
    );
    Ok((quest_data, generated))
}

pub async fn write_job(
    generator: &QuestGenerator,
    queue: &mut JobQueue,
    title: String,
    giver: String,
    description: String,
    goal: String,
    difficulty: u8,
) -> anyhow::Result<()> {
    let (quest_data, generated) =
        generate_written_job(generator, title, giver, description, goal, difficulty).await?;
    enqueue_quest(queue, quest_data, generated)?;
    tracing::info!(queued = queue.entries.len(), "quest added to queue");
    Ok(())
}

/// Remove a quest from the board by id (any state). Errors if not found.
pub fn delete_quest(board: &mut Board, quest_id: u32) -> anyhow::Result<()> {
    if !board.remove_quest(quest_id) {
        anyhow::bail!("quest {} not found on board", quest_id);
    }
    storage::save_board(board)?;
    tracing::info!(id = quest_id, "quest removed from board");
    Ok(())
}

/// Create a chud for a Discord user and save it. Errors if one already exists.
pub fn add_chud(
    discord_user_id: u64,
    name: String,
    description: String,
) -> anyhow::Result<AddChudResult> {
    let mut benefits = storage::load_starting_benefits()?;
    let bonus = benefits.take(discord_user_id);

    if let Some(mut player) = storage::load_player(discord_user_id)? {
        if player.has_chud() {
            anyhow::bail!("chud already exists for user {}", discord_user_id);
        }
        let chud = create_chud(discord_user_id, name, description);
        player.chud = chud.chud;
        player.cash = player.cash.saturating_add(bonus);
        storage::save_player(&player)?;
        storage::save_starting_benefits(&benefits)?;
        tracing::info!(name = %player.chud_ref().name, discord_user_id, bonus, "chud respawned");
        return Ok(AddChudResult {
            player,
            benefits_claimed: bonus,
        });
    }

    let mut player = create_chud(discord_user_id, name, description);
    player.cash = player.cash.saturating_add(bonus);
    storage::save_player(&player)?;
    storage::save_starting_benefits(&benefits)?;
    tracing::info!(name = %player.chud_ref().name, discord_user_id, bonus, "chud created");
    Ok(AddChudResult {
        player,
        benefits_claimed: bonus,
    })
}

/// Delete a chud's save file. Errors if not found.
pub fn delete_chud(discord_user_id: u64) -> anyhow::Result<()> {
    storage::delete_player(discord_user_id)?;
    tracing::info!(discord_user_id, "chud deleted");
    Ok(())
}

/// Assign a chud to an open board quest.
pub fn assign_chud_to_quest(
    board: &mut Board,
    hospital: &Hospital,
    discord_user_id: u64,
    quest_id: u32,
    pre_assign_status: Option<&GuildHallStatus>,
    job_timeout_tick: u32,
) -> anyhow::Result<AssignInfo> {
    let player = storage::load_player(discord_user_id)?
        .filter(|p| p.has_chud())
        .ok_or_else(|| anyhow::anyhow!("no chud found for user {}", discord_user_id))?;

    let busy = match pre_assign_status {
        Some(status) => status.is_busy(discord_user_id),
        None => crate::game::busy::is_player_busy(board, hospital, discord_user_id).is_some(),
    };
    if busy {
        anyhow::bail!("user {} is busy", discord_user_id);
    }

    let quest = board
        .quests
        .iter()
        .find(|q| q.id == quest_id)
        .ok_or_else(|| anyhow::anyhow!("quest {} not found", quest_id))?;

    let trial_count = quest.quest_data.trials.len() as u32;
    let quest_title = quest.generated.quest_title.clone();
    let ticks_remaining = trial_count;

    if !board.assign(
        quest_id,
        discord_user_id,
        ticks_remaining,
        job_timeout_tick,
    ) {
        anyhow::bail!("quest {} is not available for assignment", quest_id);
    }

    storage::save_board(board)?;
    tracing::info!(discord_user_id, quest_id, ticks_remaining, "chud assigned to quest");
    Ok(AssignInfo {
        player,
        quest_title,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAwardDisposition {
    Stashed,
    Equipped,
    Sold(u32),
}

/// Register a quest item: stash when there is room, else equip if the slot is empty, else sell.
pub fn award_pending_item(
    registry: &mut ItemRegistry,
    merchant: &mut MerchantState,
    player: &mut Player,
    item: crate::game::domain::item::Item,
) -> anyhow::Result<(crate::game::domain::item::Item, ItemAwardDisposition)> {
    let id = registry.add_item(item);
    let awarded = registry
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("item {} missing after add", id))?
        .clone();

    if player.stash.has_room() {
        player.stash.push(id)?;
        storage::save_player(player)?;
        storage::save_item_registry(registry)?;
        return Ok((awarded, ItemAwardDisposition::Stashed));
    }

    if let Some(slot) = player
        .chud_mut()
        .equipment
        .empty_slot_for_item_type(awarded.item_type)
    {
        *player.chud_mut().equipment.slot_mut(slot) = Some(id);
        storage::save_player(player)?;
        storage::save_item_registry(registry)?;
        tracing::info!(
            player = %player.chud_ref().name,
            item = %awarded.name,
            slot = %slot.label(),
            "stash full, auto-equipped quest item"
        );
        return Ok((awarded, ItemAwardDisposition::Equipped));
    }

    let gold = awarded.value;
    merchant.recycle_item(id)?;
    player.cash = player.cash.saturating_add(gold);
    storage::save_player(player)?;
    tracing::info!(
        player = %player.chud_ref().name,
        item = %awarded.name,
        gold,
        "stash full and no empty slot, auto-sold quest item"
    );
    Ok((awarded, ItemAwardDisposition::Sold(gold)))
}

fn equip_item_id(player: &mut Player, item_id: u32, item_type: ItemType) -> Option<u32> {
    player
        .chud_mut()
        .equipment
        .slot_for_item_type(item_type)
        .replace(item_id)
}

pub fn is_item_equipped(player: &Player, item_id: u32) -> bool {
    player
        .chud_ref()
        .equipment
        .all_ids()
        .any(|id| id == item_id)
}

/// Move an item from stash into the appropriate equipment slot, swapping any displaced item into stash.
pub fn equip_from_stash(
    board: &Board,
    hospital: &Hospital,
    player: &mut Player,
    item_id: u32,
    registry: &ItemRegistry,
) -> anyhow::Result<()> {
    if crate::game::busy::is_equipment_locked(board, hospital, player.discord_user_id).is_some() {
        anyhow::bail!("equipment locked");
    }
    if !player.stash.contains(item_id) {
        anyhow::bail!("item not in stash");
    }
    let item_type = registry
        .get(item_id)
        .ok_or_else(|| anyhow::anyhow!("item {} not found", item_id))?
        .item_type;
    player.stash.remove(item_id);
    if let Some(displaced) = equip_item_id(player, item_id, item_type) {
        player.stash.push(displaced)?;
    }
    storage::save_player(player)?;
    Ok(())
}

/// Move an equipped item into the stash.
pub fn unequip_slot(
    board: &Board,
    hospital: &Hospital,
    player: &mut Player,
    slot: EquipmentSlot,
) -> anyhow::Result<()> {
    if crate::game::busy::is_equipment_locked(board, hospital, player.discord_user_id).is_some() {
        anyhow::bail!("equipment locked");
    }
    let item_id = player
        .chud_mut()
        .equipment
        .item_id_in_slot(slot)
        .ok_or_else(|| anyhow::anyhow!("slot is empty"))?;
    player.stash.push(item_id)?;
    *player.chud_mut().equipment.slot_mut(slot) = None;
    storage::save_player(player)?;
    Ok(())
}

fn player_owns_item(player: &Player, item_id: u32) -> bool {
    player.stash.contains(item_id)
        || player
            .chud_ref()
            .equipment
            .all_ids()
            .any(|id| id == item_id)
}

fn clear_equipped_item(player: &mut Player, item_id: u32) {
    let eq = &mut player.chud_mut().equipment;
    if eq.gear == Some(item_id) {
        eq.gear = None;
    } else if eq.weapon == Some(item_id) {
        eq.weapon = None;
    } else {
        for slot in &mut eq.misc {
            if *slot == Some(item_id) {
                *slot = None;
            }
        }
    }
}

fn transfer_item_from_player(player: &mut Player, item_id: u32) {
    player.stash.remove(item_id);
    clear_equipped_item(player, item_id);
}

/// Buy an item from the visiting merchant into the player's stash.
pub fn buy_merchant_item(
    merchant: &mut crate::game::merchant::MerchantState,
    player: &mut Player,
    registry: &mut ItemRegistry,
    slot: usize,
) -> Result<crate::game::domain::item::Item, crate::game::merchant::BuyError> {
    merchant.buy(slot, player, registry)
}

/// Sell an item the player owns: credit `cash`, remove from stash/equipment, recycle into Dumpster Dave.
pub fn sell_item(
    board: &Board,
    hospital: &Hospital,
    player: &mut Player,
    registry: &ItemRegistry,
    merchant: &mut MerchantState,
    item_id: u32,
) -> anyhow::Result<u32> {
    if !player_owns_item(player, item_id) {
        anyhow::bail!("item not owned");
    }
    if is_item_equipped(player, item_id)
        && crate::game::busy::is_equipment_locked(board, hospital, player.discord_user_id).is_some()
    {
        anyhow::bail!("equipment locked");
    }
    let item = registry
        .get(item_id)
        .ok_or_else(|| anyhow::anyhow!("item {} not found", item_id))?
        .clone();
    let gold = item.value;

    // TODO: vendor/transfer logic (market fees, soulbound checks, etc.) before payout/removal.
    transfer_item_from_player(player, item_id);
    merchant.recycle_item(item_id)?;
    player.cash = player.cash.saturating_add(gold);
    storage::save_player(player)?;
    Ok(gold)
}

/// Sync rolls and stat lookups for quest-result generation. Callers should release any
/// shared locks before the async `finish_quest_result` step.
pub struct QuestResultPrep {
    pub played: PlayedQuest,
    pub effective: (u8, u8, u8),
    pub trial_results: Vec<TrialResult>,
    pub failure_consequences: Option<FailureConsequences>,
    pub failure_outcome: Option<String>,
    pub adventurer_description: String,
}

pub fn prepare_quest_result(
    item_registry: &ItemRegistry,
    board_quest: &BoardQuest,
    player: &Player,
) -> anyhow::Result<QuestResultPrep> {
    let played = play_quest(
        &board_quest.quest_data,
        &board_quest.generated,
        player,
        item_registry,
    )?;

    let effective = effective_stats(player, item_registry);

    let trial_results: Vec<TrialResult> = played
        .outcomes
        .iter()
        .zip(board_quest.generated.trials.iter())
        .map(|(o, situation)| TrialResult {
            situation: situation.clone(),
            stat_used: o.stat_used.label().to_string(),
            margin: o.player_roll as i16 - o.trial_roll as i16,
            passed: o.passed,
        })
        .collect();

    let passed = played.outcomes.iter().all(|o| o.passed);
    let failure_consequences = if !passed {
        let margin = played
            .outcomes
            .last()
            .map(|o| o.player_roll as i16 - o.trial_roll as i16)
            .unwrap_or(0);
        let mut rng = rand::thread_rng();
        let consequences = roll_failure_consequences(margin, &mut rng);
        if consequences.died {
            tracing::info!(
                margin,
                death_chance = death_chance(margin),
                "chud died on quest failure (pre-LLM roll)"
            );
        } else if let Some(ticks) = consequences.hospital_ticks {
            tracing::info!(
                margin,
                injury_chance = injury_chance(margin),
                ticks,
                "chud injured on quest failure (pre-LLM roll)"
            );
        }
        Some(consequences)
    } else {
        None
    };
    let failure_outcome = failure_consequences.as_ref().map(|c| {
        if c.died {
            "died".to_string()
        } else if c.hospital_ticks.is_some() {
            "injured".to_string()
        } else {
            "failed".to_string()
        }
    });

    let equipped: Vec<_> = player
        .chud_ref()
        .equipment
        .all_ids()
        .filter_map(|id| item_registry.get(id))
        .collect();
    let adventurer_description = player.generate_description(equipped.iter().copied());

    Ok(QuestResultPrep {
        played,
        effective,
        trial_results,
        failure_consequences,
        failure_outcome,
        adventurer_description,
    })
}

/// LLM narrative and optional item generation for a prepared quest result.
pub async fn finish_quest_result(
    generator: &QuestGenerator,
    item_generator: &ItemGenerator,
    board_quest: &BoardQuest,
    player: &Player,
    force_item_drop: bool,
    prep: QuestResultPrep,
) -> anyhow::Result<QuestResult> {
    let QuestResultPrep {
        played,
        effective,
        trial_results,
        failure_consequences,
        failure_outcome,
        adventurer_description,
    } = prep;

    let failure_outcome = failure_outcome.as_deref();
    let QuestResults { trials, summary } = generator
        .generate_results(
            &board_quest.quest_data,
            &board_quest.generated,
            &trial_results,
            &player.chud_ref().name,
            &adventurer_description,
            failure_outcome,
        )
        .await?;

    let mut result = QuestResult::build(
        &board_quest.generated,
        &played,
        effective,
        trials,
        summary,
    );
    result.failure_consequences = failure_consequences;

    let difficulty = board_quest.quest_data.quest_difficulty;
    let trial_count = board_quest.quest_data.trials.len();
    if result.passed {
        let story_reward = board_quest
            .story_index
            .and_then(story_jobs::story_reward);

        let seed = if let Some(reward) = story_reward.as_ref() {
            Some(roll_item(
                difficulty,
                &mut rand::thread_rng(),
                Some(&reward.rarity),
                reward.stats.as_ref(),
            ))
        } else if board_quest.story_index.is_some() {
            None
        } else if force_item_drop || roll_item_drop(difficulty, trial_count, &mut rand::thread_rng()) {
            Some(roll_item(
                difficulty,
                &mut rand::thread_rng(),
                None,
                None,
            ))
        } else {
            None
        };

        if let Some(seed) = seed {
            let (name, description) = item_generator
                .generate_from_quest(&seed, &result, board_quest)
                .await?;
            let rarity = seed.rarity.clone();
            let mut item = seed.into_item(name, description);
            item.value = roll_item_value(&item.stats, &item.rarity, &mut rand::thread_rng());
            tracing::info!(
                name = %item.name,
                item_type = ?item.item_type,
                rarity = %rarity,
                story = board_quest.story_index.is_some(),
                drop_chance = if board_quest.story_index.is_some() {
                    1.0
                } else {
                    item_drop_chance(difficulty, trial_count)
                },
                difficulty,
                trial_count,
                net_stat = item.stats.net_stat_value(),
                value = item.value,
                "item generated for quest result"
            );
            result.pending_item = Some(item);
        }
    }

    result.log();
    Ok(result)
}

/// Run the full LLM pipeline for a single quest and return the result.
pub async fn generate_result(
    generator: &QuestGenerator,
    item_generator: &ItemGenerator,
    item_registry: &ItemRegistry,
    board_quest: &BoardQuest,
    player: &Player,
    force_item_drop: bool,
) -> anyhow::Result<QuestResult> {
    let prep = prepare_quest_result(item_registry, board_quest, player)?;
    finish_quest_result(
        generator,
        item_generator,
        board_quest,
        player,
        force_item_drop,
        prep,
    )
    .await
}

fn collect_owned_item_ids(player: &Player) -> Vec<u32> {
    let mut ids: Vec<u32> = player.stash.items().to_vec();
    for id in player.chud_ref().equipment.all_ids() {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Liquidate all items on a chud, recycling them into Dumpster Dave; returns total value before halving.
pub fn liquidate_chud_items(
    player: &mut Player,
    registry: &ItemRegistry,
    merchant: &mut MerchantState,
) -> anyhow::Result<u32> {
    let item_ids = collect_owned_item_ids(player);
    let mut total = 0u32;
    for item_id in item_ids {
        if let Some(item) = registry.get(item_id) {
            total = total.saturating_add(item.value);
        }
        transfer_item_from_player(player, item_id);
        merchant.recycle_item(item_id)?;
    }
    Ok(total)
}

/// Sync portion of chud death: liquidate gear, bury with a placeholder epitaph, clear the chud.
pub struct KillChudPending {
    pub discord_user_id: u64,
    pub chud_name: String,
    pub benefits_awarded: u32,
    pub chud: crate::game::domain::player::Chud,
    pub death_trial: String,
    pub death_outcome: String,
}

pub fn kill_chud_apply(
    discord_user_id: u64,
    ctx: &DeathContext,
    graveyard: &mut Graveyard,
    starting_benefits: &mut StartingBenefits,
    registry: &ItemRegistry,
    merchant: &mut MerchantState,
) -> anyhow::Result<KillChudPending> {
    let mut player = storage::load_player(discord_user_id)?
        .filter(|p| p.has_chud())
        .ok_or_else(|| anyhow::anyhow!("no chud found for user {}", discord_user_id))?;

    let chud = player.chud.clone().expect("checked has_chud");
    let chud_name = chud.name.clone();

    let total = liquidate_chud_items(&mut player, registry, merchant)?;
    let benefits_awarded = total / 2;
    starting_benefits.add(discord_user_id, benefits_awarded);

    graveyard.bury(GraveyardEntry {
        discord_user_id,
        chud: chud.clone(),
        epitaph: String::new(),
        death_trial: ctx.trial.clone(),
        death_outcome: ctx.outcome.clone(),
    });

    player.chud = None;
    storage::save_player(&player)?;
    storage::save_graveyard(graveyard)?;
    storage::save_starting_benefits(starting_benefits)?;

    tracing::info!(
        discord_user_id,
        chud = %chud_name,
        benefits_awarded,
        "chud killed"
    );

    Ok(KillChudPending {
        discord_user_id,
        chud_name,
        benefits_awarded,
        chud,
        death_trial: ctx.trial.clone(),
        death_outcome: ctx.outcome.clone(),
    })
}

/// Async gravestone LLM; updates the buried entry. Call without runtime mutexes held.
pub async fn finish_gravestone_epitaph(
    gravestone_generator: &GravestoneGenerator,
    pending: &KillChudPending,
) -> anyhow::Result<String> {
    let epitaph = gravestone_generator
        .generate(&pending.chud, &pending.death_trial, &pending.death_outcome)
        .await?;
    let mut graveyard = storage::load_graveyard()?;
    if let Some(entry) = graveyard
        .entries
        .iter_mut()
        .find(|e| e.discord_user_id == pending.discord_user_id)
    {
        entry.epitaph = epitaph.clone();
        storage::save_graveyard(&graveyard)?;
    }
    Ok(epitaph)
}

/// Kill a chud: liquidate items, award starting benefits, bury in graveyard, remove chud.
pub async fn kill_chud(
    discord_user_id: u64,
    ctx: &DeathContext,
    graveyard: &mut Graveyard,
    starting_benefits: &mut StartingBenefits,
    registry: &ItemRegistry,
    merchant: &mut MerchantState,
    gravestone_generator: &GravestoneGenerator,
) -> anyhow::Result<KillResult> {
    let pending = kill_chud_apply(
        discord_user_id,
        ctx,
        graveyard,
        starting_benefits,
        registry,
        merchant,
    )?;
    let epitaph = finish_gravestone_epitaph(gravestone_generator, &pending).await?;
    Ok(KillResult {
        discord_user_id: pending.discord_user_id,
        chud_name: pending.chud_name,
        benefits_awarded: pending.benefits_awarded,
        epitaph,
    })
}

pub fn save_all(board: &Board, item_registry: &ItemRegistry) -> anyhow::Result<()> {
    storage::save_board(board)?;
    storage::save_item_registry(item_registry)?;
    storage::save_graveyard(&storage::load_graveyard()?)?;
    storage::save_starting_benefits(&storage::load_starting_benefits()?)?;
    let ids = storage::list_player_ids()?;
    let count = ids.len();
    for id in ids {
        if let Some(player) = storage::load_player(id)? {
            storage::save_player(&player)?;
        }
    }
    tracing::info!(players = count, "save_all complete");
    Ok(())
}

pub fn load_all() -> anyhow::Result<(Board, ItemRegistry)> {
    let mut board = storage::load_board()?;
    let mut session = storage::load_session()?;
    storage::resume_story_state(&mut board, &mut session)?;
    Ok((board, storage::load_item_registry()?))
}
