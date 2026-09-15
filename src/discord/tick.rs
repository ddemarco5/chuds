mod render;

use crate::discord::channel::{
    append_activity_log_deferred, cleanup_non_bot_messages, clear_persistent_message_reactions,
};
use crate::discord::context::GameRuntime;
use crate::chud_msg;
use crate::game::engine::{self, KillChudPending};
use crate::game::merchant::{MerchantTickEvent, MerchantVisit};
use crate::game::persistence::message_cache::ActivityLogKind;
use crate::game::persistence::storage;
use crate::game::tick::{run_tick, TickContext, TickOutcome};

pub use render::render_tick_outcome;

/// Apply one game tick and post results to Discord.
///
/// No-op outside the `Playing` phase, so a stray background tick during attract/complete
/// never mutates state or touches the channel.
pub async fn execute_tick(runtime: &GameRuntime) -> anyhow::Result<()> {
    let tick_n = {
        let mut session = runtime.session.lock().await;
        if !session.is_playing() {
            tracing::debug!("tick skipped: game is not in the playing phase");
            return Ok(());
        }
        session.total_ticks = session.total_ticks.saturating_add(1);
        if let Err(e) = storage::save_session(&session) {
            tracing::warn!(err = %e, "failed to persist tick count");
        }
        session.total_ticks
    };

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
    let mut episode_stats = storage::load_episode_stats()?;

    let (outcome, pending_deaths) = {
        let mut board = runtime.board.lock().await;
        let mut queue = runtime.job_queue.lock().await;
        let mut registry = runtime.item_registry.lock().await;
        let mut merchant = runtime.merchant.lock().await;

        let description_history_msgs = runtime.generator.description_history_len();

        let outcome = run_tick(&mut TickContext {
            board: &mut *board,
            hospital: &mut hospital,
            episode_stats: &mut episode_stats,
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
            jobs_per_tick: runtime.jobs_per_tick,
        })?;

        storage::save_hospital(&hospital)?;
        storage::save_episode_stats(&episode_stats)?;

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
        let departing = merchant.visit.as_ref().map(|v| v.merchant_name.clone());
        let merchant_event = merchant.advance_tick(force);
        log_merchant_tick_event(merchant_event, departing.as_deref(), merchant.visit.as_ref());
        storage::save_guild_hall(&merchant)?;

        log_tick_summary(tick_n, &outcome, board.quests.len(), runtime.max_jobs, merchant_event);

        (outcome, pending_deaths)
    };

    render_tick_outcome(runtime, &outcome, &pending_deaths).await
}

fn log_merchant_tick_event(
    event: MerchantTickEvent,
    departing: Option<&str>,
    visit: Option<&MerchantVisit>,
) {
    match event {
        MerchantTickEvent::Spawned => {
            if let Some(visit) = visit {
                tracing::info!(
                    merchant = %visit.merchant_name,
                    ticks_remaining = visit.ticks_remaining,
                    "merchant arrived"
                );
            }
        }
        MerchantTickEvent::Departed => {
            tracing::info!(merchant = departing.unwrap_or("unknown"), "merchant departed");
        }
        MerchantTickEvent::Tick => {
            if let Some(visit) = visit {
                tracing::debug!(
                    merchant = %visit.merchant_name,
                    ticks_remaining = visit.ticks_remaining,
                    "merchant visit tick"
                );
            }
        }
        MerchantTickEvent::None => {}
    }
}

fn log_tick_summary(
    tick: u32,
    outcome: &TickOutcome,
    board: usize,
    max_jobs: usize,
    merchant_event: MerchantTickEvent,
) {
    let merchant_changed = matches!(
        merchant_event,
        MerchantTickEvent::Spawned | MerchantTickEvent::Departed
    );
    let quiet = outcome.hospital_releases.is_empty()
        && outcome.scout_results.is_empty()
        && outcome.quest_resolved.is_empty()
        && outcome.slots_filled == 0
        && outcome.jobs_expired == 0
        && outcome.jobs_requested == 0
        && outcome.quests_due == 0
        && !merchant_changed;

    if quiet {
        tracing::debug!(
            tick,
            due = outcome.quests_due,
            resolved = outcome.quest_resolved.len(),
            scouts = outcome.scout_results.len(),
            hospital = outcome.hospital_releases.len(),
            filled = outcome.slots_filled,
            expired = outcome.jobs_expired,
            requested = outcome.jobs_requested,
            board,
            max_jobs,
            "tick"
        );
    } else {
        tracing::info!(
            tick,
            due = outcome.quests_due,
            resolved = outcome.quest_resolved.len(),
            scouts = outcome.scout_results.len(),
            hospital = outcome.hospital_releases.len(),
            filled = outcome.slots_filled,
            expired = outcome.jobs_expired,
            requested = outcome.jobs_requested,
            board,
            max_jobs,
            "tick"
        );
    }
}
