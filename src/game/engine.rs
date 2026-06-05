use crate::game::domain::board::{Board, BoardQuest};
use crate::game::domain::graveyard::{Graveyard, GraveyardEntry};
use crate::game::domain::hospital::Hospital;
use crate::game::domain::starting_benefits::StartingBenefits;
use crate::game::guild_status::GuildHallStatus;
use crate::game::domain::item::{EquipmentSlot, ItemType};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::player::{create_chud, Player};
use crate::game::domain::quest_result::QuestResult;
use crate::game::generation::gravestone_generator::GravestoneGenerator;
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData, QuestGenerator, QuestResults, TrialResult};
use crate::game::tuneable_rolls::{item_drop_chance, roll_item, roll_item_drop, roll_item_value, roll_trials};
use crate::game::mechanics::simulation::{effective_stats, play_quest};
use crate::game::persistence::storage;

/// Context about what killed a chud, used for epitaph generation.
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
    },
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
) -> anyhow::Result<AssignInfo> {
    let info = assign_chud_to_quest(
        board,
        hospital,
        discord_user_id,
        quest_id,
        pre_assign_status,
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

/// Move quests from the front of the queue onto the board until full or queue empty.
pub fn refill_board_from_queue(
    board: &mut Board,
    queue: &mut JobQueue,
    max_jobs: usize,
) -> usize {
    let mut added = 0usize;
    while board.quests.len() < max_jobs && !queue.entries.is_empty() {
        let entry = queue.entries.remove(0);
        board.add_quest(entry.quest_data, entry.generated);
        added += 1;
    }
    if added > 0 {
        tracing::info!(added, "board refilled from queue");
    }
    added
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
    GenerationJob::QuestCreation { quest_data }
}

/// Generate trial flavor via LLM and enqueue the quest (does not add directly to board).
pub async fn write_job(
    generator: &QuestGenerator,
    queue: &mut JobQueue,
    title: String,
    giver: String,
    description: String,
    goal: String,
    difficulty: u8,
) -> anyhow::Result<()> {
    let quest_data = QuestData {
        quest_description: description,
        quest_goal: Some(goal),
        quest_difficulty: difficulty,
        trials: roll_trials(difficulty),
    };
    let generated = generator
        .generate_from_explicit(&quest_data, title, giver)
        .await?;
    tracing::info!(title = %generated.quest_title, giver = %generated.quest_giver, "quest written");
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
pub fn add_chud(discord_user_id: u64, name: String, description: String) -> anyhow::Result<Player> {
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
        return Ok(player);
    }

    let mut player = create_chud(discord_user_id, name, description);
    player.cash = player.cash.saturating_add(bonus);
    storage::save_player(&player)?;
    storage::save_starting_benefits(&benefits)?;
    tracing::info!(name = %player.chud_ref().name, discord_user_id, bonus, "chud created");
    Ok(player)
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

    if !board.assign(quest_id, discord_user_id, ticks_remaining) {
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
    registry
        .remove(id)
        .ok_or_else(|| anyhow::anyhow!("item {} missing from registry", id))?;
    player.cash = player.cash.saturating_add(gold);
    storage::save_player(player)?;
    storage::save_item_registry(registry)?;
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

/// Move an item from stash into the appropriate equipment slot, swapping any displaced item into stash.
pub fn equip_from_stash(
    player: &mut Player,
    item_id: u32,
    registry: &ItemRegistry,
) -> anyhow::Result<()> {
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
pub fn unequip_slot(player: &mut Player, slot: EquipmentSlot) -> anyhow::Result<()> {
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

/// Sell an item the player owns: credit `cash`, remove from stash/equipment and registry.
pub fn sell_item(
    player: &mut Player,
    registry: &mut ItemRegistry,
    item_id: u32,
) -> anyhow::Result<u32> {
    if !player_owns_item(player, item_id) {
        anyhow::bail!("item not owned");
    }
    let item = registry
        .get(item_id)
        .ok_or_else(|| anyhow::anyhow!("item {} not found", item_id))?
        .clone();
    let gold = item.value;

    // TODO: vendor/transfer logic (market fees, soulbound checks, etc.) before payout/removal.
    player.stash.remove(item_id);
    clear_equipped_item(player, item_id);
    registry
        .remove(item_id)
        .ok_or_else(|| anyhow::anyhow!("item {} missing from registry", item_id))?;
    player.cash = player.cash.saturating_add(gold);
    storage::save_player(player)?;
    storage::save_item_registry(registry)?;
    Ok(gold)
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

    let equipped: Vec<_> = player
        .chud_ref()
        .equipment
        .all_ids()
        .filter_map(|id| item_registry.get(id))
        .collect();
    let adventurer_description = player.generate_description(equipped.iter().copied());

    let QuestResults { trials, summary } = generator
        .generate_results(
            &board_quest.quest_data,
            &board_quest.generated,
            &trial_results,
            &player.chud_ref().name,
            &adventurer_description,
        )
        .await?;

    let mut result = QuestResult::build(
        &board_quest.generated,
        &played,
        effective,
        trials,
        summary,
    );

    let difficulty = board_quest.quest_data.quest_difficulty;
    let trial_count = board_quest.quest_data.trials.len();
    if result.passed && (force_item_drop || roll_item_drop(difficulty, trial_count, &mut rand::thread_rng())) {
        let seed = roll_item(difficulty, &mut rand::thread_rng());
        let (name, description) = item_generator
            .generate_from_quest(&seed, &result, board_quest)
            .await?;
        let rarity = seed.rarity.clone();
        let mut item = seed.into_item(name, description);
        item.value = roll_item_value(&item.stats, &mut rand::thread_rng());
        tracing::info!(
            name = %item.name,
            item_type = ?item.item_type,
            rarity = %rarity,
            drop_chance = item_drop_chance(difficulty, trial_count),
            difficulty,
            trial_count,
            net_stat = item.stats.net_stat_value(),
            value = item.value,
            "item generated for quest result"
        );
        result.pending_item = Some(item);
    }

    result.log();
    Ok(result)
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

/// Sell all items on a chud and return the total value before halving.
pub fn liquidate_chud_items(player: &mut Player, registry: &mut ItemRegistry) -> u32 {
    let item_ids = collect_owned_item_ids(player);
    let mut total = 0u32;
    for item_id in item_ids {
        if let Some(item) = registry.get(item_id) {
            total = total.saturating_add(item.value);
        }
        player.stash.remove(item_id);
        clear_equipped_item(player, item_id);
        registry.remove(item_id);
    }
    total
}

/// Kill a chud: liquidate items, award starting benefits, bury in graveyard, remove chud.
pub async fn kill_chud(
    discord_user_id: u64,
    ctx: &DeathContext,
    graveyard: &mut Graveyard,
    starting_benefits: &mut StartingBenefits,
    registry: &mut ItemRegistry,
    gravestone_generator: &GravestoneGenerator,
) -> anyhow::Result<KillResult> {
    let mut player = storage::load_player(discord_user_id)?
        .filter(|p| p.has_chud())
        .ok_or_else(|| anyhow::anyhow!("no chud found for user {}", discord_user_id))?;

    let chud = player.chud.clone().expect("checked has_chud");
    let chud_name = chud.name.clone();

    let epitaph = gravestone_generator
        .generate(&chud, &ctx.trial, &ctx.outcome)
        .await?;

    let total = liquidate_chud_items(&mut player, registry);
    let benefits_awarded = total / 2;
    starting_benefits.add(discord_user_id, benefits_awarded);

    graveyard.bury(GraveyardEntry {
        discord_user_id,
        chud,
        epitaph: epitaph.clone(),
        death_trial: ctx.trial.clone(),
        death_outcome: ctx.outcome.clone(),
    });

    player.chud = None;
    storage::save_player(&player)?;
    storage::save_graveyard(graveyard)?;
    storage::save_starting_benefits(starting_benefits)?;
    storage::save_item_registry(registry)?;

    tracing::info!(
        discord_user_id,
        chud = %chud_name,
        benefits_awarded,
        "chud killed"
    );

    Ok(KillResult {
        discord_user_id,
        chud_name,
        benefits_awarded,
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
    Ok((storage::load_board()?, storage::load_item_registry()?))
}
