use crate::game::domain::board::{Board, BoardQuest};
use crate::game::engine::{self, DeathContext};
use crate::game::persistence::storage;
use crate::game::tick::{QuestResolved, TickContext, TickOutcome};
use crate::game::tuneable_rolls::{death_chance, injury_chance, roll_failure_consequences};

pub fn quest_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let due = ctx.board.tick_and_take_due();
    tracing::info!(quests_due = due.len(), "quest phase complete");

    for board_quest in due {
        if let Some(resolved) = resolve_quest(
            ctx.board,
            &mut ctx.hospital,
            ctx.item_registry,
            ctx.merchant,
            board_quest,
            ctx.job_timeout_tick,
            outcome,
        )? {
            outcome.quest_resolved.push(resolved);
        }
    }

    // Trigger the next story job the moment a story success is recorded (story_next_index
    // was advanced above). Guarded, so it is a no-op except right after a story quest
    // passes; refill remains only as the bootstrap/recovery fallback.
    engine::ensure_story_generation(ctx.board, ctx.generation_queue, ctx.pending_quests);

    storage::save_board(ctx.board)?;
    storage::save_hospital(&ctx.hospital)?;
    storage::save_item_registry(ctx.item_registry)?;
    Ok(())
}

fn resolve_quest(
    board: &mut Board,
    hospital: &mut crate::game::domain::hospital::Hospital,
    item_registry: &mut crate::game::persistence::item_registry::ItemRegistry,
    merchant: &mut crate::game::merchant::MerchantState,
    board_quest: BoardQuest,
    job_timeout_tick: u32,
    outcome: &mut TickOutcome,
) -> anyhow::Result<Option<QuestResolved>> {
    let discord_user_id = match board_quest.assigned_to() {
        Some(id) => id,
        None => {
            tracing::warn!(quest_id = board_quest.id, "due quest has no assigned player, skipping");
            return Ok(None);
        }
    };

    let mut result = match board.completed_results.remove(&board_quest.id) {
        Some(r) => r,
        None => {
            tracing::warn!(quest_id = board_quest.id, "no completed result found, skipping");
            return Ok(None);
        }
    };

    let mut player = match storage::load_player(discord_user_id)? {
        Some(p) => p,
        None => {
            tracing::warn!(
                discord_user_id,
                quest_id = board_quest.id,
                "player not found, skipping quest resolution"
            );
            return Ok(None);
        }
    };

    let passed = result.passed;
    let summary = result.summary.clone();
    let quest_title = board_quest.generated.quest_title.clone();
    let reward = board_quest.generated.reward;

    let level_up = player.record_quest(&result);
    let player_name = player.chud_ref().name.clone();
    if passed {
        tracing::info!("{} made ${}", player_name, reward);
        player.cash += reward;

        if board_quest.story_index.is_some() {
            board.story_next_index += 1;
            tracing::info!(
                story_next_index = board.story_next_index,
                catalog_len = board.effective_story_catalog_len(),
                "story quest passed, advancing progress"
            );
            if board.story_series_complete() {
                outcome.story_series_complete = true;
                outcome.final_completer_user_id = Some(discord_user_id);
                outcome.final_completer_chud_name = Some(player_name.clone());
            }
        }
    }

    let mut item_awarded = None;
    let mut item_award_disposition = None;
    if passed {
        if let Some(item) = result.pending_item.take() {
            match engine::award_pending_item(item_registry, merchant, &mut player, item) {
                Ok((item, disposition)) => {
                    item_awarded = Some(item);
                    item_award_disposition = Some(disposition);
                }
                Err(e) => {
                    tracing::warn!(err = %e, player = %player_name, "could not award quest item");
                }
            }
        }
    }

    storage::save_player(&player)?;

    tracing::info!(
        discord_user_id,
        quest = %quest_title,
        passed,
        "quest resolved and player saved"
    );

    let consequences = result.failure_consequences.clone().or_else(|| {
        if !passed {
            result.trials.last().map(|t| {
                roll_failure_consequences(t.margin, &mut rand::thread_rng())
            })
        } else {
            None
        }
    });

    let died = consequences.as_ref().is_some_and(|c| c.died);
    let mut death_ctx = None;
    if died {
        if let Some(t) = result.trials.last() {
            death_ctx = Some(DeathContext {
                trial: t.situation.clone(),
                outcome: t.narrative.clone(),
            });
            tracing::info!(
                discord_user_id,
                margin = t.margin,
                death_chance = death_chance(t.margin),
                "chud died on quest failure"
            );
        }
    }

    let hospitalized = consequences
        .and_then(|c| c.hospital_ticks)
        .map(|ticks| {
            let margin = result.trials.last().map(|t| t.margin).unwrap_or(0);
            let admitted = hospital.admit(discord_user_id, player_name.clone(), ticks).is_some();
            if admitted {
                tracing::info!(
                    discord_user_id,
                    margin,
                    injury_chance = injury_chance(margin),
                    ticks,
                    "chud injured and admitted to hospital"
                );
            }
            admitted
        })
        .unwrap_or(false);

    if !passed {
        board.quests.push(BoardQuest {
            id: board_quest.id,
            quest_data: board_quest.quest_data,
            generated: board_quest.generated,
            states: Vec::new(),
            story_index: board_quest.story_index,
            timeout: job_timeout_tick,
        });
    }

    Ok(Some(QuestResolved {
        discord_user_id,
        player_name,
        quest_title,
        summary,
        result,
        player,
        level_up,
        reward,
        hospitalized,
        died,
        death_ctx,
        item_awarded,
        item_award_disposition,
    }))
}
