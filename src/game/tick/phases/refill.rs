use crate::game::engine;
use crate::game::tick::{TickContext, TickOutcome};

pub fn refill_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let expired = engine::drain_expired_board_jobs(ctx.board);
    for entry in &expired {
        ctx.queue
            .push(entry.quest_data.clone(), entry.generated.clone());
    }
    outcome.jobs_expired = expired.len();

    let added = engine::refill_board(
        ctx.board,
        ctx.queue,
        ctx.max_jobs,
        ctx.job_timeout_tick,
        ctx.generation_queue,
        ctx.pending_quests,
    );
    outcome.slots_filled += added;
    if !expired.is_empty() {
        let n = expired.len();
        tracing::info!(
            timed_out = n,
            replaced = added,
            "{n} jobs timed out and were replaced"
        );
    }
    if !expired.is_empty() || added > 0 {
        crate::game::persistence::storage::save_board(ctx.board)?;
        crate::game::persistence::storage::save_job_queue(ctx.queue)?;
    }
    Ok(())
}
