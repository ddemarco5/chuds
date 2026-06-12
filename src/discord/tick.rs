mod render;

use crate::discord::channel::{
    append_activity_log_deferred, cleanup_non_bot_messages, clear_persistent_message_reactions,
};
use crate::discord::context::GameRuntime;
use crate::chud_msg;
use crate::game::engine::{self, KillChudPending};
use crate::game::persistence::message_cache::ActivityLogKind;
use crate::game::persistence::storage;
use crate::game::tick::{run_tick, TickContext};

pub use render::render_tick_outcome;

/// Apply one game tick and post results to Discord.
///
/// No-op outside the `Playing` phase, so a stray background tick during attract/complete
/// never mutates state or touches the channel.
pub async fn execute_tick(runtime: &GameRuntime) -> anyhow::Result<()> {
    {
        let mut session = runtime.session.lock().await;
        if !session.is_playing() {
            tracing::debug!("tick skipped: game is not in the playing phase");
            return Ok(());
        }
        session.total_ticks = session.total_ticks.saturating_add(1);
        if let Err(e) = storage::save_session(&session) {
            tracing::warn!(err = %e, "failed to persist tick count");
        }
    }

    cleanup_non_bot_messages(
        &runtime.http,
        runtime.channel_id,
        runtime.bot_user_id,
        runtime.max_non_bot_messages,
    )
    .await;

    clear_persistent_message_reactions(&runtime.http, runtime.channel_id).await;

    {
        let board_guard = runtime.board.lock().await;
        if board_guard.active_quest_count() > 0 {
            let new_day_msg = chud_msg!("new_day");
            append_activity_log_deferred(&runtime.activity_log, ActivityLogKind::World, &new_day_msg)
                .await;
        }
    }

    let mut hospital = storage::load_hospital()?;
    let mut graveyard = storage::load_graveyard()?;
    let mut starting_benefits = storage::load_starting_benefits()?;

    let (outcome, pending_deaths) = {
        let mut board = runtime.board.lock().await;
        let mut queue = runtime.job_queue.lock().await;
        let mut registry = runtime.item_registry.lock().await;
        let mut merchant = runtime.merchant.lock().await;

        let description_history_msgs = runtime.generator.description_history_len();

        let outcome = run_tick(&mut TickContext {
            board: &mut *board,
            hospital: &mut hospital,
            queue: &mut *queue,
            item_registry: &mut *registry,
            merchant: &mut *merchant,
            max_jobs: runtime.max_jobs,
            generation_queue: Some(&runtime.generation_queue),
            pending_quests: Some(&runtime.pending_quests),
            pending_auto_jobs: Some(&runtime.pending_auto_jobs),
            description_history_msgs,
            reserved_cm_slot_num: runtime.reserved_cm_slot_num,
            job_gen_min_history_msgs: runtime.job_gen_min_history_msgs,
            job_timeout_tick: runtime.job_timeout_tick,
        })?;

        storage::save_hospital(&hospital)?;

        let mut pending_deaths: Vec<KillChudPending> = Vec::new();
        for qr in &outcome.quest_resolved {
            if qr.died {
                if let Some(ctx) = &qr.death_ctx {
                    pending_deaths.push(engine::kill_chud_apply(
                        qr.discord_user_id,
                        ctx,
                        &mut graveyard,
                        &mut starting_benefits,
                        &registry,
                        &mut merchant,
                    )?);
                }
            }
        }

        engine::ensure_merchant_catalog_generation(
            &merchant,
            Some(&runtime.generation_queue),
            Some(&runtime.pending_merchant_catalog),
        );
        let force = merchant.spawn_next_tick;
        merchant.spawn_next_tick = false;
        merchant.advance_tick(force);
        storage::save_guild_hall(&merchant)?;

        (outcome, pending_deaths)
    };

    render_tick_outcome(runtime, &outcome, &pending_deaths).await
}
