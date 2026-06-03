use std::{sync::Arc, time::Duration};

use chuds::discord::{
    self, cleanup_non_bot_messages, execute_tick, handle_gear_button, handle_heal_button,
    handle_scout_button, handle_take_button, recover_persistent_board_messages,
    update_board_message, validate_cached_messages_exist, Data,
};
use chuds::game::engine;
use chuds::game::generation::worker::{spawn_generation_worker, WorkerEffect};
use chuds::game::persistence::storage;
use chuds::game::state::GameState;
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

    let api_key = std::env::var("LLM_API_KEY")
        .map_err(|_| anyhow::anyhow!("LLM_API_KEY not set"))?;
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
    let max_job_queue: usize = std::env::var("MAX_JOB_QUEUE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let max_non_bot_messages: usize = std::env::var("MAX_NON_BOT_MESSAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let tick_time_s: u64 = std::env::var("TICK_TIME_S")
        .map_err(|_| anyhow::anyhow!("TICK_TIME_S not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("TICK_TIME_S must be a positive integer"))?;

    let generator = Arc::new(chuds::game::generation::quest_generator::QuestGenerator::new(
        &api_key,
    )?);
    let item_generator = Arc::new(chuds::game::generation::item_generator::ItemGenerator::new(
        &api_key,
    )?);
    let game_state = GameState::load()?;
    let board = Arc::new(tokio::sync::Mutex::new(game_state.board));
    let job_queue = Arc::new(tokio::sync::Mutex::new(game_state.job_queue));
    let item_registry = Arc::new(tokio::sync::Mutex::new(game_state.item_registry));
    let pending_quests = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    tracing::info!(tick_time_s, "chuds bot starting");

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                discord::commands::tick(),
                discord::commands::generate_job(),
                discord::commands::write_job(),
                discord::commands::delete_job(),
                discord::commands::chud(),
                discord::commands::chudlerboard(),
                discord::commands::add_chud(),
                discord::commands::delete_chud(),
                discord::commands::assign(),
                discord::commands::stats(),
                discord::commands::gear(),
                discord::commands::cash(),
                discord::commands::job(),
                discord::commands::save(),
                discord::commands::load(),
                discord::commands::add_cm(),
                discord::commands::delete_cm(),
                discord::commands::admin_take_gen_item(),
                discord::commands::admin_redraw(),
            ],
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move {
                    if let serenity::FullEvent::InteractionCreate { interaction } = event {
                        if let serenity::Interaction::Component(component) = interaction {
                            let id = &component.data.custom_id;
                            let handled = if id.starts_with("take:") {
                                Some(handle_take_button(ctx, component, data).await)
                            } else if id.starts_with("scout:") {
                                Some(handle_scout_button(ctx, component, data).await)
                            } else if id.starts_with("heal:") {
                                Some(handle_heal_button(ctx, component, data).await)
                            } else if id.starts_with("g_equip:")
                                || id.starts_with("g_unequip:")
                                || id.starts_with("g_sell:")
                                || id.starts_with("g_sell_confirm:")
                            {
                                Some(handle_gear_button(ctx, component, data).await)
                            } else {
                                None
                            };
                            if let Some(Err(e)) = handled {
                                tracing::warn!(err = %e, custom_id = %component.data.custom_id, "button handler failed");
                            }
                        }
                    }
                    Ok(())
                })
            },
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
                let bot_user_id = ready.user.id.get();

                let messages_exist = {
                    let _cache_guard = storage::message_cache_lock().await;
                    let cache = storage::load_message_cache().unwrap_or_default();
                    validate_cached_messages_exist(&ctx.http, channel_id, &cache).await
                };
                if !messages_exist {
                    let mut b = board.lock().await;
                    let mut q = job_queue.lock().await;
                    recover_persistent_board_messages(
                        &ctx.http,
                        channel_id,
                        &mut *b,
                        &mut *q,
                        bot_user_id,
                        max_non_bot_messages,
                        max_jobs,
                    )
                    .await?;
                } else {
                    cleanup_non_bot_messages(
                        &ctx.http,
                        channel_id,
                        bot_user_id,
                        max_non_bot_messages,
                    )
                    .await;
                    let mut b = board.lock().await;
                    let mut q = job_queue.lock().await;
                    engine::refill_board_from_queue(&mut *b, &mut *q, max_jobs);
                    storage::save_board(&*b)?;
                    storage::save_job_queue(&*q)?;
                    update_board_message(&ctx.http, channel_id, &mut b, max_jobs).await?;
                }

                let (generation_tx, generation_rx) =
                    tokio::sync::mpsc::unbounded_channel::<chuds::game::engine::GenerationJob>();

                {
                    let worker_board = Arc::clone(&board);
                    let worker_queue = Arc::clone(&job_queue);
                    let worker_registry = Arc::clone(&item_registry);
                    let worker_generator = Arc::clone(&generator);
                    let worker_item_generator = Arc::clone(&item_generator);
                    let worker_http = Arc::clone(&ctx.http);
                    let worker_pending = Arc::clone(&pending_quests);
                    spawn_generation_worker(
                        generation_rx,
                        Arc::clone(&worker_board),
                        Arc::clone(&worker_queue),
                        Arc::clone(&worker_registry),
                        worker_generator,
                        worker_item_generator,
                        max_jobs,
                        worker_pending,
                        move |effects| {
                            let worker_http = Arc::clone(&worker_http);
                            let worker_board = Arc::clone(&worker_board);
                            Box::pin(async move {
                                if effects.iter().any(|e| {
                                    matches!(e, WorkerEffect::BoardRefilled { .. })
                                }) {
                                    let mut b = worker_board.lock().await;
                                    if let Err(e) = update_board_message(
                                        &worker_http,
                                        channel_id,
                                        &mut b,
                                        max_jobs,
                                    )
                                    .await
                                    {
                                        tracing::warn!(err = %e, "failed to update board after quest creation");
                                    }
                                }
                            })
                        },
                    );
                }

                let tick_board = Arc::clone(&board);
                let tick_queue = Arc::clone(&job_queue);
                let tick_registry = Arc::clone(&item_registry);
                let tick_http = Arc::clone(&ctx.http);
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(tick_time_s));
                    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        tracing::info!("background tick firing");
                        if let Err(e) = execute_tick(
                            &tick_http,
                            &tick_board,
                            &tick_queue,
                            &tick_registry,
                            channel_id,
                            max_buffer_messages,
                            max_jobs,
                            bot_user_id,
                            max_non_bot_messages,
                        )
                        .await
                        {
                            tracing::error!(err = %e, "background tick failed");
                        }
                    }
                });

                Ok(Data {
                    generator,
                    item_generator,
                    board,
                    job_queue,
                    item_registry,
                    admin_user_id,
                    bot_user_id,
                    channel_id,
                    max_buffer_messages,
                    max_jobs,
                    max_job_queue,
                    max_non_bot_messages,
                    generation_queue: generation_tx,
                    pending_quests,
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
