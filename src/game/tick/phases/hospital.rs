use crate::game::tick::{TickContext, TickOutcome};

pub fn hospital_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    for entry in &ctx.hospital.entries {
        ctx.episode_stats
            .record_hospital_day(entry.discord_user_id, &entry.chud_name);
    }
    outcome.hospital_releases = ctx.hospital.tick();
    Ok(())
}
