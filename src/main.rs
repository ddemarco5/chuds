mod board;
mod commands;
mod engine;
mod player;
mod quest_builder;
mod quest_generator;
mod quest_result;
mod storage;

use std::{sync::Arc, time::Duration};

use commands::Data;
use poise::serenity_prelude as serenity;
use tokio::time::MissedTickBehavior;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set"))?;
    let token = std::env::var("DISCORD_TOKEN")
        .map_err(|_| anyhow::anyhow!("DISCORD_TOKEN not set"))?;
    let admin_user_id: u64 = std::env::var("ADMIN_USER_ID")
        .map_err(|_| anyhow::anyhow!("ADMIN_USER_ID not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("ADMIN_USER_ID must be a u64"))?;
    let channel_id: u64 = std::env::var("CHANNEL_ID")
        .map_err(|_| anyhow::anyhow!("CHANNEL_ID not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("CHANNEL_ID must be a u64"))?;
    let guild_id: u64 = std::env::var("GUILD_ID")
        .map_err(|_| anyhow::anyhow!("GUILD_ID not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("GUILD_ID must be a u64"))?;
    let max_buffer_messages: usize = std::env::var("MESSAGE_BUFFER_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let max_jobs: usize = std::env::var("MAX_JOBS")
        .map_err(|_| anyhow::anyhow!("MAX_JOBS not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("MAX_JOBS must be a positive integer"))?;
    let tick_time_s: u64 = std::env::var("TICK_TIME_S")
        .map_err(|_| anyhow::anyhow!("TICK_TIME_S not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("TICK_TIME_S must be a positive integer"))?;

    let generator = Arc::new(quest_generator::QuestGenerator::new(&api_key)?);
    let board = Arc::new(tokio::sync::Mutex::new(storage::load_board()?));

    tracing::info!(tick_time_s, "chuds bot starting");

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                commands::tick(),
                commands::generate_job(),
                commands::write_job(),
                commands::delete_quest(),
                commands::chud(),
                commands::add_chud(),
                commands::delete_chud(),
                commands::assign(),
                commands::take(),
                commands::stats(),
                commands::job(),
                commands::save(),
                commands::load(),
            ],
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                tracing::info!(
                    bot = %ready.user.name,
                    bot_id = ready.user.id.get(),
                    guilds = ready.guilds.len(),
                    "connected to Discord"
                );
                poise::builtins::register_in_guild(
                    ctx,
                    &framework.options().commands,
                    serenity::GuildId::new(guild_id),
                )
                .await?;
                tracing::info!(guild_id, "slash commands registered");
                let guild_name = ctx.http
                    .get_guild(serenity::GuildId::new(guild_id))
                    .await
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|_| "Server".to_string());
                tracing::info!(guild_name, "fetched guild name");
                {
                    let mut b = board.lock().await;
                    commands::update_board_message(&ctx.http, channel_id, &mut b, &guild_name).await?;
                }

                let tick_board = Arc::clone(&board);
                let tick_generator = Arc::clone(&generator);
                let tick_http = Arc::clone(&ctx.http);
                let tick_guild_name = guild_name.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(tick_time_s));
                    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        tracing::info!("background tick firing");
                        if let Err(e) = commands::execute_tick(
                            &tick_http,
                            &tick_generator,
                            &tick_board,
                            channel_id,
                            max_buffer_messages,
                            &tick_guild_name,
                        ).await {
                            tracing::error!(err = %e, "background tick failed");
                        }
                    }
                });

                Ok(Data {
                    generator,
                    board,
                    admin_user_id,
                    channel_id,
                    max_buffer_messages,
                    max_jobs,
                    guild_name,
                })
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged();
    let mut client = serenity::ClientBuilder::new(&token, intents)
        .framework(framework)
        .await
        .map_err(|e| anyhow::anyhow!("failed to build Discord client: {e}"))?;

    tracing::info!("bot connected, listening for slash commands");
    client
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Discord client error: {e}"))
}
