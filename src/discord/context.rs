use std::sync::{atomic::AtomicUsize, Arc, RwLock};
use std::collections::HashSet;

use poise::serenity_prelude::{self as serenity, UserId};

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
use crate::game::generation::quest_generator::QuestGenerator;

/// Shared game + Discord runtime: every long-lived handle the simulation, ticks, and
/// commands need. Bundled once so `execute_tick(&runtime)` and the simulation loop avoid
/// threading a dozen parameters everywhere.
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
    pub activity_log: Arc<ActivityLogSync>,
    pub pending_quests: Arc<AtomicUsize>,
    pub last_mobile: Arc<RwLock<HashSet<UserId>>>,
    pub session: Arc<tokio::sync::Mutex<GameSession>>,
    pub generation_queue: tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    pub story_shutdown_tx: tokio::sync::mpsc::UnboundedSender<()>,
    pub admin_user_id: u64,
    pub bot_user_id: u64,
    pub channel_id: u64,
    pub guild_id: u64,
    pub max_jobs: usize,
    pub max_job_queue: usize,
    pub max_non_bot_messages: usize,
    pub tick_time_s: u64,
}

impl GameRuntime {
    /// True if the current phase is `Playing`.
    pub async fn is_playing(&self) -> bool {
        self.session.lock().await.is_playing()
    }
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
        ctx.say("you don't have permission for this command (sorry bud)")
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
    ctx.say("the game isn't running right now (sorry bud)")
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
    #[name = "Difficulty (1-10)"]
    #[placeholder = "e.g. 5"]
    pub difficulty: String,
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
