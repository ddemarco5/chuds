use std::sync::{atomic::AtomicUsize, Arc};

use poise::serenity_prelude::{self as serenity};

use crate::discord::channel::ActivityLogSync;
use crate::discord::simulation::SimulationController;
use crate::game::domain::board::Board;
use crate::game::domain::session::GameSession;
use crate::game::merchant::MerchantState;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::engine::GenerationJob;
use crate::game::generation::gravestone_generator::GravestoneGenerator;
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::merchant_generator::MerchantGenerator;
use crate::game::generation::quest_generator::QuestGenerator;

/// Shared game + Discord runtime: every long-lived handle the simulation, ticks, and
/// commands need. Bundled once so `execute_tick(&runtime)` and the simulation loop avoid
/// threading a dozen parameters everywhere.
///
/// Runtime mutex lock order (always acquire in this order when taking multiple locks):
/// `board` → `job_queue` → `item_registry` → `merchant`.
pub struct GameRuntime {
    pub http: Arc<serenity::Http>,
    pub cache: Arc<serenity::Cache>,
    pub board: Arc<tokio::sync::Mutex<Board>>,
    pub job_queue: Arc<tokio::sync::Mutex<JobQueue>>,
    pub item_registry: Arc<tokio::sync::Mutex<ItemRegistry>>,
    pub merchant: Arc<tokio::sync::Mutex<MerchantState>>,
    pub generator: Arc<QuestGenerator>,
    pub item_generator: Arc<ItemGenerator>,
    pub gravestone_generator: Arc<GravestoneGenerator>,
    pub merchant_generator: Arc<MerchantGenerator>,
    pub activity_log: Arc<ActivityLogSync>,
    pub pending_quests: Arc<AtomicUsize>,
    pub pending_auto_jobs: Arc<AtomicUsize>,
    pub pending_merchant_catalog: Arc<AtomicUsize>,
    pub session: Arc<tokio::sync::Mutex<GameSession>>,
    pub generation_queue: tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    /// Signals the completion supervisor to transition the game into the Complete phase.
    pub complete_tx: tokio::sync::mpsc::UnboundedSender<GameCompletion>,
    pub admin_user_id: u64,
    pub bot_user_id: u64,
    pub channel_id: u64,
    pub guild_id: u64,
    pub max_jobs: usize,
    pub max_job_queue: usize,
    pub max_non_bot_messages: usize,
    pub tick_time_s: u64,
    /// Empty board slots reserved for chudmaster-submitted jobs before auto-fill runs.
    pub reserved_cm_slot_num: usize,
    /// Minimum description-memory messages required before auto-generation runs.
    pub job_gen_min_history_msgs: usize,
    /// Ticks an idle regular job may sit on the board before returning to the queue.
    pub job_timeout_tick: u32,
}

impl GameRuntime {
    /// True if the current phase is `Playing`.
    pub async fn is_playing(&self) -> bool {
        self.session.lock().await.is_playing()
    }
}

/// Payload sent when the final story mission is completed, driving the Playing -> Complete
/// transition (handled off the tick task so the simulation can stop itself cleanly).
#[derive(Debug, Clone)]
pub struct GameCompletion {
    pub user_id: u64,
    pub chud_name: String,
}

pub struct Data {
    pub runtime: Arc<GameRuntime>,
    pub simulation: Arc<SimulationController>,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub async fn admin_guard(ctx: Context<'_>) -> bool {
    let rt = &ctx.data().runtime;
    let ok = ctx.author().id.get() == rt.admin_user_id
        && ctx.channel_id().get() == rt.channel_id;
    if !ok {
        tracing::warn!(
            user = ctx.author().id.get(),
            channel = ctx.channel_id().get(),
            "unauthorized or off-channel command ignored"
        );
        say_ephemeral(ctx, "you don't have permission for this command (sorry bud)")
            .await
            .ok();
    }
    ok
}

/// Reject (with an ephemeral reply) any command run outside the `Playing` phase.
/// Returns `true` when the game is playing and the command may proceed.
pub async fn require_playing(ctx: Context<'_>) -> bool {
    if ctx.data().runtime.is_playing().await {
        return true;
    }
    tracing::info!(
        user = ctx.author().id.get(),
        "command rejected: game is not in the playing phase"
    );
    // Keep the rejection out of the channel — reply only to the caller.
    ctx.send(
        poise::CreateReply::default()
            .content("the game isn't running right now (sorry bud)")
            .ephemeral(true),
    )
    .await
    .ok();
    false
}

pub fn chudmaster_check(data: &Data, user_id: u64, channel_id: u64) -> bool {
    let rt = &data.runtime;
    let is_admin = user_id == rt.admin_user_id;
    let is_chudmaster =
        crate::game::persistence::storage::is_chudmaster(user_id).unwrap_or(false);
    (is_admin || is_chudmaster) && channel_id == rt.channel_id
}

pub fn parse_difficulty(raw: &str) -> Result<u8, &'static str> {
    let difficulty: u8 = raw.trim().parse().map_err(|_| "difficulty must be a number")?;
    if !(1..=10).contains(&difficulty) {
        return Err("difficulty must be between 1 and 10");
    }
    Ok(difficulty)
}

/// Parse an optional difficulty field; blank/missing values roll auto difficulty from chud stats.
pub fn parse_difficulty_optional(raw: Option<&str>) -> Result<u8, &'static str> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(crate::game::engine::roll_job_difficulty()),
        Some(s) => parse_difficulty(s),
    }
}

pub async fn say_ephemeral(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

#[derive(Debug, poise::Modal)]
#[name = "Generate Job"]
pub struct GenerateJobModal {
    #[name = "Description"]
    #[paragraph]
    pub description: String,
    #[name = "Goal (optional)"]
    #[paragraph]
    pub goal: Option<String>,
    #[name = "Difficulty (optional, 1-10)"]
    #[placeholder = "leave blank for auto"]
    pub difficulty: Option<String>,
}

#[derive(Debug, poise::Modal)]
#[name = "Write Job"]
pub struct WriteJobModal {
    #[name = "Title"]
    pub title: String,
    #[name = "Giver"]
    pub giver: String,
    #[name = "Description"]
    #[paragraph]
    pub description: String,
    #[name = "Goal"]
    #[paragraph]
    pub goal: String,
    #[name = "Difficulty (1-10)"]
    #[placeholder = "e.g. 5"]
    pub difficulty: String,
}
