use std::sync::atomic::Ordering;

use crate::game::engine;
use crate::game::persistence::storage;
use crate::game::tick::{TickContext, TickOutcome};
use crate::game::tuneable_rolls::roll_auto_job_difficulty;

/// Seed handed to the description agent for auto-generated jobs. The agent already holds
/// every prior job posting in its conversation memory, so this just asks for another in
/// the established style while steering away from duplicates.
const SEED_DESCRIPTION: &str = "Generate a brand-new job posting in the same world, tone, and style as the previous jobs. Do not closely repeat any earlier job's premise or goal, but you may reuse characters or settings.";

/// Mean stat level assumed when no chuds exist yet, roughly a starting chud's power.
const DEFAULT_MEAN_STAT: f64 = 3.0;

/// Top the backlog job queue back up when it runs low, generating new regular jobs in the
/// style of past postings so the board never starves between chudmaster and story jobs.
pub fn create_jobs_phase(ctx: &mut TickContext, outcome: &mut TickOutcome) -> anyhow::Result<()> {
    let (Some(gen_q), Some(pending)) = (ctx.generation_queue, ctx.pending_quests) else {
        return Ok(());
    };

    // Need enough prior postings in memory before we can imitate their style.
    if ctx.description_history_msgs < ctx.job_gen_min_history_msgs {
        return Ok(());
    }

    // Backlog fill = already-queued jobs plus generations still in flight, mirroring the
    // capacity check in `/generate_job` so we never over-generate across ticks.
    let fill = ctx.queue.entries.len() + pending.load(Ordering::SeqCst);
    if fill > ctx.job_gen_low_threshold {
        return Ok(());
    }

    let to_generate = ctx.job_gen_high_threshold.saturating_sub(fill);
    if to_generate == 0 {
        return Ok(());
    }

    let mean_stat = mean_chud_stat();
    let mut rng = rand::thread_rng();
    for _ in 0..to_generate {
        let difficulty = roll_auto_job_difficulty(mean_stat, &mut rng);
        pending.fetch_add(1, Ordering::SeqCst);
        gen_q.send(engine::make_quest_creation_job(
            SEED_DESCRIPTION.to_string(),
            difficulty,
            None,
        ))?;
    }

    outcome.jobs_requested += to_generate;
    tracing::info!(
        requested = to_generate,
        fill,
        low = ctx.job_gen_low_threshold,
        high = ctx.job_gen_high_threshold,
        mean_stat,
        "auto job generation triggered"
    );

    Ok(())
}

/// Average of every chud's strength/smarts/stealth across all saved players. Falls back to
/// `DEFAULT_MEAN_STAT` when there are no chuds yet.
fn mean_chud_stat() -> f64 {
    let ids = match storage::list_player_ids() {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(err = %e, "failed to list players for mean stat; using default");
            return DEFAULT_MEAN_STAT;
        }
    };

    let mut total: u64 = 0;
    let mut count: u64 = 0;
    for id in ids {
        match storage::load_player(id) {
            Ok(Some(player)) => {
                if let Some(chud) = player.chud {
                    total += chud.strength as u64 + chud.smarts as u64 + chud.stealth as u64;
                    count += 3;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(err = %e, player = id, "failed to load player for mean stat"),
        }
    }

    if count == 0 {
        DEFAULT_MEAN_STAT
    } else {
        total as f64 / count as f64
    }
}
