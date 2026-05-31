use std::sync::{
    atomic::AtomicUsize,
    Arc,
};

use crate::game::domain::board::Board;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::engine::GenerationJob;
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::quest_generator::QuestGenerator;

pub struct Data {
    pub generator: Arc<QuestGenerator>,
    pub item_generator: Arc<ItemGenerator>,
    pub board: Arc<tokio::sync::Mutex<Board>>,
    pub job_queue: Arc<tokio::sync::Mutex<JobQueue>>,
    pub item_registry: Arc<tokio::sync::Mutex<ItemRegistry>>,
    pub admin_user_id: u64,
    pub bot_user_id: u64,
    pub channel_id: u64,
    pub max_buffer_messages: usize,
    pub max_jobs: usize,
    pub max_job_queue: usize,
    pub max_non_bot_messages: usize,
    pub generation_queue: tokio::sync::mpsc::UnboundedSender<GenerationJob>,
    pub pending_quests: Arc<AtomicUsize>,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub async fn admin_guard(ctx: Context<'_>) -> bool {
    let data = ctx.data();
    let ok = ctx.author().id.get() == data.admin_user_id
        && ctx.channel_id().get() == data.channel_id;
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

pub fn chudmaster_check(data: &Data, user_id: u64, channel_id: u64) -> bool {
    let is_admin = user_id == data.admin_user_id;
    let is_chudmaster =
        crate::game::persistence::storage::is_chudmaster(user_id).unwrap_or(false);
    (is_admin || is_chudmaster) && channel_id == data.channel_id
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
