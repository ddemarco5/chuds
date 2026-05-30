use crate::game::tick::{TickContext, TickOutcome};

pub fn hospital_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    outcome.hospital_releases = ctx.hospital.tick();
    Ok(())
}
