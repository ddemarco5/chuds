use crate::game::engine;
use crate::game::tick::{TickContext, TickOutcome};

pub fn refill_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let result = engine::run_tick_refill(
        ctx.board,
        ctx.queue,
        ctx.item_registry,
        ctx.max_jobs,
        ctx.job_timeout_tick,
        ctx.generation_queue,
        ctx.pending_quests,
        ctx.pending_auto_jobs,
        ctx.description_history_msgs,
        ctx.job_gen_min_history_msgs,
        ctx.reserved_cm_slot_num,
    )?;
    outcome.jobs_expired = result.jobs_expired;
    outcome.slots_filled += result.slots_filled;
    outcome.jobs_requested += result.jobs_requested;
    let _ = result.persisted;
    Ok(())
}
