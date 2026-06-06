mod render;

use crate::discord::channel::{append_activity_log_deferred, cleanup_non_bot_messages};
use crate::discord::context::GameRuntime;
use crate::chud_msg;
use crate::game::persistence::message_cache::ActivityLogKind;
use crate::game::persistence::storage;
use crate::game::tick::{run_tick, TickContext};

pub use render::render_tick_outcome;

/// Apply one game tick and post results to Discord.
///
/// No-op outside the `Playing` phase, so a stray background tick during attract/complete
/// never mutates state or touches the channel.
pub async fn execute_tick(runtime: &GameRuntime) -> anyhow::Result<()> {
    if !runtime.is_playing().await {
        tracing::debug!("tick skipped: game is not in the playing phase");
        return Ok(());
    }

    cleanup_non_bot_messages(
        &runtime.http,
        runtime.channel_id,
        runtime.bot_user_id,
        runtime.max_non_bot_messages,
    )
    .await;

    {
        let board_guard = runtime.board.lock().await;
        if board_guard.active_quest_count() > 0 {
            let new_day_msg = chud_msg!("new_day");
            append_activity_log_deferred(&runtime.activity_log, ActivityLogKind::World, &new_day_msg)
                .await;
        }
    }

    let mut hospital = storage::load_hospital()?;
    let mut board = runtime.board.lock().await;
    let mut queue = runtime.job_queue.lock().await;
    let mut registry = runtime.item_registry.lock().await;

    let outcome = run_tick(&mut TickContext {
        board: &mut *board,
        hospital: &mut hospital,
        queue: &mut *queue,
        item_registry: &mut *registry,
        max_jobs: runtime.max_jobs,
        generation_queue: Some(&runtime.generation_queue),
        pending_quests: Some(&runtime.pending_quests),
    })?;

    storage::save_hospital(&hospital)?;
    drop(queue);

    render_tick_outcome(runtime, &outcome, &mut board, &mut registry).await
}
