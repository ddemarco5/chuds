mod board;
mod chudmasters;
mod commands;
mod messages;
mod engine;
mod hospital;
mod message_cache;
mod player;
mod quest_builder;
mod quest_generator;
mod quest_result;
mod simulation;
mod storage;

use std::{sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};

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
    let max_non_bot_messages: usize = std::env::var("MAX_NON_BOT_MESSAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let tick_time_s: u64 = std::env::var("TICK_TIME_S")
        .map_err(|_| anyhow::anyhow!("TICK_TIME_S not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("TICK_TIME_S must be a positive integer"))?;

    let generator = Arc::new(quest_generator::QuestGenerator::new(&api_key)?);
    let board = Arc::new(tokio::sync::Mutex::new(storage::load_board()?));
    let pending_quests = Arc::new(AtomicUsize::new(0));

    tracing::info!(tick_time_s, "chuds bot starting");

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                commands::tick(),
                commands::generate_job(),
                commands::write_job(),
                commands::delete_job(),
                commands::chud(),
                commands::add_chud(),
                commands::delete_chud(),
                commands::assign(),
                commands::stats(),
                commands::cash(),
                commands::job(),
                commands::save(),
                commands::load(),
                commands::add_cm(),
                commands::delete_cm(),
            ],
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move {
                    if let serenity::FullEvent::InteractionCreate { interaction } = event {
                        if let serenity::Interaction::Component(component) = interaction {
                            let id = &component.data.custom_id;
                            let handled = if id.starts_with("take:") {
                                Some(commands::handle_take_button(ctx, component, data).await)
                            } else if id.starts_with("scout:") {
                                Some(commands::handle_scout_button(ctx, component, data).await)
                            } else if id.starts_with("heal:") {
                                Some(commands::handle_heal_button(ctx, component, data).await)
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

                // -----------------------------------------------------------------------
                // Startup validation: check if all cached message IDs still exist.
                // If any are missing, purge the channel and start fresh.
                // -----------------------------------------------------------------------
                let cache = storage::load_message_cache().unwrap_or_default();
                let mut b = board.lock().await;
                let messages_exist = commands::validate_cached_messages_exist(&ctx.http, channel_id, &*b, &cache).await;
                if !messages_exist {
                    tracing::warn!("cached messages missing, purging channel and resetting message cache");
                    commands::delete_all_messages_in_channel(&ctx.http, channel_id).await;
                    // Reset message IDs so new messages will be posted
                    b.chudlerboard_message_id = None;
                    // Clear the message cache so update_board_message posts fresh messages
                    if let Err(e) = storage::save_message_cache(&Default::default()) {
                        tracing::warn!(err = %e, "failed to clear message cache");
                    }
                }
                drop(b);

                commands::cleanup_non_bot_messages(&ctx.http, channel_id, bot_user_id, max_non_bot_messages).await;
                {
                    let mut b = board.lock().await;
                    commands::update_board_message(&ctx.http, channel_id, &mut b, max_jobs).await?;
                }

                // -----------------------------------------------------------------------
                // Generation worker
                //
                // LLM calls are decoupled from both the tick loop and the Discord command
                // handler via an unbounded mpsc channel.  The flow is:
                //
                //   /generate_job command
                //     └─ validates board capacity (sync, fast)
                //     └─ sends GenerationJob::QuestCreation { quest_data } down `generation_tx`
                //
                //   /take or /assign command
                //     └─ assigns quest on board (sync, fast)
                //     └─ sends GenerationJob::QuestResult { board_quest, player } down `generation_tx`
                //
                //   worker task (single sequential task — one LLM call at a time)
                //     └─ QuestResult: calls engine::generate_result, stores in board.completed_results
                //     └─ QuestCreation: calls generate_from_description, adds quest to board,
                //        updates the board message
                //
                //   tick (periodic or /tick command)
                //     └─ decrements ticks_remaining on all active quests
                //     └─ picks up quests where ticks_remaining == 0 AND result is present
                //     └─ applies stats, DMs player, posts to channel
                //
                // If generate_result errors the quest stays Active with ticks_remaining=0
                // and is silently skipped every tick until manually removed.
                // -----------------------------------------------------------------------
                let (generation_tx, mut generation_rx) = tokio::sync::mpsc::unbounded_channel::<engine::GenerationJob>();

                {
                    let worker_board = Arc::clone(&board);
                    let worker_generator = Arc::clone(&generator);
                    let worker_http = Arc::clone(&ctx.http);
                    let worker_pending = Arc::clone(&pending_quests);
                    tokio::spawn(async move {
                        // Sequential: we only pick up the next job after the current one
                        // finishes. This avoids hammering the LLM API concurrently and
                        // keeps completed_results writes orderly.
                        while let Some(job) = generation_rx.recv().await {
                            match job {
                                engine::GenerationJob::QuestResult { board_quest, player } => {
                                    match engine::generate_result(&*worker_generator, &board_quest, &player).await {
                                        Ok(result) => {
                                            let mut b = worker_board.lock().await;
                                            b.completed_results.insert(board_quest.id, result);
                                            if let Err(e) = storage::save_board(&*b) {
                                                tracing::error!(err = %e, "failed to save board after generation");
                                            }
                                            tracing::info!(quest_id = board_quest.id, "generation complete, result stored");
                                        }
                                        Err(e) => {
                                            // The quest remains Active on the board. Every subsequent
                                            // tick will skip it (no entry in completed_results) until
                                            // someone manually deletes it with /delete_job.
                                            tracing::error!(
                                                quest_id = board_quest.id,
                                                err = %e,
                                                "generate_result failed; quest will remain active and be skipped each tick"
                                            );
                                        }
                                    }
                                }
                                engine::GenerationJob::QuestCreation { quest_data } => {
                                    match worker_generator.generate_from_description(&quest_data).await {
                                        Ok(generated) => {
                                            tracing::info!(title = %generated.quest_title, giver = %generated.quest_giver, "quest generated");
                                            let mut b = worker_board.lock().await;
                                            let id = b.add_quest(quest_data, generated);
                                            worker_pending.fetch_sub(1, Ordering::SeqCst);
                                            if let Err(e) = storage::save_board(&*b) {
                                                tracing::error!(err = %e, "failed to save board after quest creation");
                                            }
                                            tracing::info!(quest_id = id, "quest added to board");
                                            if let Err(e) = commands::update_board_message(&worker_http, channel_id, &mut *b, max_jobs).await {
                                                tracing::warn!(err = %e, "failed to update board message after quest creation");
                                            }
                                        }
                                        Err(e) => {
                                            worker_pending.fetch_sub(1, Ordering::SeqCst);
                                            tracing::error!(err = %e, "quest creation failed");
                                        }
                                    }
                                }
                            }
                        }
                    });
                }

                // Periodic tick task — fires every TICK_TIME_S seconds.
                // This is separate from the generation worker above: by the
                // time a tick fires the worker has (usually) already written
                // the LLM result into board.completed_results. execute_tick
                // just applies those cached results and sends Discord messages.
                let tick_board = Arc::clone(&board);
                let tick_http = Arc::clone(&ctx.http);
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(tick_time_s));
                    // Skip missed ticks instead of bursting to catch up after lag.
                    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        tracing::info!("background tick firing");
                        if let Err(e) = commands::execute_tick(
                            &tick_http,
                            &tick_board,
                            channel_id,
                            max_buffer_messages,
                            max_jobs,
                            bot_user_id,
                            max_non_bot_messages,
                        ).await {
                            tracing::error!(err = %e, "background tick failed");
                        }
                    }
                });

                Ok(Data {
                    generator,
                    board,
                    admin_user_id,
                    bot_user_id,
                    channel_id,
                    max_buffer_messages,
                    max_jobs,
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
