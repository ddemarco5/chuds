use crate::game::engine;
use crate::game::tick::{TickContext, TickOutcome};

pub fn refill_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let added = engine::refill_board(
        ctx.board,
        ctx.queue,
        ctx.max_jobs,
        ctx.generation_queue,
        ctx.pending_quests,
    );
    outcome.slots_filled += added;
    if added > 0 {
        crate::game::persistence::storage::save_board(ctx.board)?;
        crate::game::persistence::storage::save_job_queue(ctx.queue)?;
    }
    Ok(())
}
