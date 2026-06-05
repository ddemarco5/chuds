use crate::game::domain::board::{Board, BoardQuest};
use crate::game::engine::{self, DeathContext};
use crate::game::persistence::storage;
use crate::game::tick::{QuestResolved, TickContext, TickOutcome};
use crate::game::tuneable_rolls::{death_chance, injury_chance, roll_death, roll_injury_outcome};

pub fn quest_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let due = ctx.board.tick_and_take_due();
    tracing::info!(quests_due = due.len(), "quest phase complete");

    for board_quest in due {
        if let Some(resolved) = resolve_quest(
            ctx.board,
            &mut ctx.hospital,
            ctx.item_registry,
            board_quest,
        )? {
            outcome.quest_resolved.push(resolved);
        }
    }

    storage::save_board(ctx.board)?;
    storage::save_hospital(&ctx.hospital)?;
    storage::save_item_registry(ctx.item_registry)?;
    Ok(())
}

fn resolve_quest(
    board: &mut Board,
    hospital: &mut crate::game::domain::hospital::Hospital,
    item_registry: &mut crate::game::persistence::item_registry::ItemRegistry,
    board_quest: BoardQuest,
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
    }

    let mut item_awarded = None;
    let mut item_award_disposition = None;
    if passed {
        if let Some(item) = result.pending_item.take() {
            match engine::award_pending_item(item_registry, &mut player, item) {
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

    let mut died = false;
    let mut death_ctx = None;

    let hospitalized = !passed
        && result.trials.last().map(|t| {
            let margin = t.margin;
            let mut rng = rand::thread_rng();

            if roll_death(margin, &mut rng) {
                died = true;
                death_ctx = Some(DeathContext {
                    trial: t.situation.clone(),
                    outcome: t.narrative.clone(),
                });
                tracing::info!(
                    discord_user_id,
                    margin,
                    death_chance = death_chance(margin),
                    "chud died on quest failure"
                );
                return false;
            }

            roll_injury_outcome(margin, &mut rng)
                .map(|ticks| {
                    let admitted =
                        hospital.admit(discord_user_id, player_name.clone(), ticks).is_some();
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
                .unwrap_or(false)
        })
        .unwrap_or(false);

    if !passed {
        board.quests.push(BoardQuest {
            id: board_quest.id,
            quest_data: board_quest.quest_data,
            generated: board_quest.generated,
            states: Vec::new(),
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
