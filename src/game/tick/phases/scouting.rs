use crate::game::domain::board::QuestState;
use crate::game::mechanics::simulation;
use crate::game::persistence::storage;
use crate::game::tick::{ScoutResult, TickContext, TickOutcome};

pub fn scouting_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let scouting: Vec<(u32, u64)> = ctx
        .board
        .quests
        .iter_mut()
        .flat_map(|q| {
            let quest_id = q.id;
            let scouts: Vec<(u32, u64)> = q
                .states
                .iter()
                .filter_map(|s| {
                    if let QuestState::Scouting { discord_user_id } = s {
                        Some((quest_id, *discord_user_id))
                    } else {
                        None
                    }
                })
                .collect();
            q.states.retain(|s| !matches!(s, QuestState::Scouting { .. }));
            scouts
        })
        .collect();

    for &(quest_id, discord_user_id) in &scouting {
        let player = match storage::load_player(discord_user_id)? {
            Some(p) => p,
            None => {
                tracing::warn!(discord_user_id, quest_id, "scouting player not found, skipping");
                continue;
            }
        };
        let quest = match ctx.board.quests.iter().find(|q| q.id == quest_id) {
            Some(q) => q,
            None => {
                tracing::warn!(quest_id, "scouted quest not found on board, skipping");
                continue;
            }
        };
        let chance = simulation::check_job(
            &quest.quest_data,
            &quest.generated,
            &player,
            ctx.item_registry,
            20,
        )?;
        tracing::info!(
            "{} checked job {} and sees a {:.2}% chance of success.",
            player.name,
            quest_id,
            chance * 100.0
        );
        let active_discord_user_id = quest.assigned_to();
        let quest_title = quest.generated.quest_title.clone();
        let quest_days = quest.quest_data.trials.len() as u32;
        let other_scouting_discord_user_ids: Vec<u64> = scouting
            .iter()
            .filter(|&&(qid, uid)| qid == quest_id && uid != discord_user_id)
            .map(|&(_, uid)| uid)
            .collect();
        outcome.scout_results.push(ScoutResult {
            discord_user_id,
            player_name: player.name.clone(),
            quest_title,
            quest_days,
            chance,
            active_discord_user_id,
            other_scouting_discord_user_ids,
        });
    }

    Ok(())
}
