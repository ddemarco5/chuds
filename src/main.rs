use std::{sync::Arc, time::Duration};

use chuds::discord::{
    self, game_screens, handle_gear_button, handle_heal_button, handle_merchant_shop_button,
    handle_scout_button, handle_shop_buy, handle_shop_select, handle_take_button,
    update_board_message, ActivityLogSync, Data, GameCompletion, GameRuntime, SimulationController,
};
use chuds::game::domain::session::GamePhase;
use chuds::game::engine::GenerationJob;
use chuds::game::generation::worker::{spawn_generation_worker, WorkerEffect};
use chuds::game::persistence::storage;
use chuds::game::state::GameState;
use poise::serenity_prelude as serenity;

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
    let activity_log_max_lines: usize = std::env::var("ACTIVITY_LOG_MAX_LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let activity_log_debounce_ms: u64 = std::env::var("ACTIVITY_LOG_DEBOUNCE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
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
    let job_gen_low_threshold: usize = std::env::var("JOB_GEN_LOW_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut job_gen_high_threshold: usize = std::env::var("JOB_GEN_HIGH_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let job_gen_min_history_msgs: usize = std::env::var("JOB_GEN_MIN_HISTORY_MSGS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let job_timeout_tick: u32 = std::env::var("JOB_TIMEOUT_TICK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    // The high threshold is the backlog target; it must exceed the low trigger and never
    // exceed the queue cap. Clamp loud rather than silently misbehaving at runtime.
    if job_gen_high_threshold <= job_gen_low_threshold {
        let clamped = job_gen_low_threshold + 1;
        tracing::warn!(
            high = job_gen_high_threshold,
            low = job_gen_low_threshold,
            clamped,
            "JOB_GEN_HIGH_THRESHOLD must be greater than JOB_GEN_LOW_THRESHOLD; clamping"
        );
        job_gen_high_threshold = clamped;
    }
    if job_gen_high_threshold > max_job_queue {
        tracing::warn!(
            high = job_gen_high_threshold,
            max_job_queue,
            "JOB_GEN_HIGH_THRESHOLD exceeds MAX_JOB_QUEUE; clamping to MAX_JOB_QUEUE"
        );
        job_gen_high_threshold = max_job_queue;
    }

    let llm_memory = chuds::game::persistence::llm_memory::load_llm_memory_bundle();
    let generator = Arc::new(chuds::game::generation::quest_generator::QuestGenerator::new(
        &api_key,
        &llm_memory,
    )?);
    let item_generator = Arc::new(chuds::game::generation::item_generator::ItemGenerator::new(
        &api_key,
        &llm_memory,
    )?);
    let gravestone_generator = Arc::new(
        chuds::game::generation::gravestone_generator::GravestoneGenerator::new(
            &api_key,
            &llm_memory,
        )?,
    );
    let merchant_generator = Arc::new(
        chuds::game::generation::merchant_generator::MerchantGenerator::new(
            &api_key,
            &llm_memory,
        )?,
    );
    let shutdown_quest = Arc::clone(&generator);
    let shutdown_item = Arc::clone(&item_generator);
    let shutdown_gravestone = Arc::clone(&gravestone_generator);
    let shutdown_merchant = Arc::clone(&merchant_generator);
    let game_state = GameState::load()?;
    let mut board = game_state.board;
    board.backfill_job_timeouts(job_timeout_tick);
    if let Err(e) = storage::save_board(&board) {
        tracing::warn!(err = %e, "failed to persist job timeout backfill");
    }
    let board = Arc::new(tokio::sync::Mutex::new(board));
    let job_queue = Arc::new(tokio::sync::Mutex::new(game_state.job_queue));
    let item_registry = Arc::new(tokio::sync::Mutex::new(game_state.item_registry));
    let merchant = Arc::new(tokio::sync::Mutex::new(game_state.guild_hall.merchant));
    let session = Arc::new(tokio::sync::Mutex::new(game_state.session));
    let pending_quests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pending_merchant_catalog = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    tracing::info!(tick_time_s, "chuds bot starting");

    let (complete_tx, complete_rx) = tokio::sync::mpsc::unbounded_channel::<GameCompletion>();
    let setup_complete_tx = complete_tx.clone();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            // After defer_ephemeral(), poise sends follow-ups via ctx.say — they need the
            // ephemeral flag set explicitly or they become public channel messages.
            reply_callback: Some(|ctx, mut builder| {
                if matches!(ctx, poise::Context::Application(_)) {
                    builder = builder.ephemeral(true);
                }
                builder
            }),
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
                discord::commands::inspect(),
                discord::commands::gear(),
                discord::commands::cash(),
                discord::commands::job(),
                discord::commands::save(),
                discord::commands::load(),
                discord::commands::add_cm(),
                discord::commands::delete_cm(),
                discord::commands::admin_take_gen_item(),
                discord::commands::admin_redraw(),
                discord::commands::admin_kill_chud(),
                discord::commands::admin_spawn_merchant(),
                discord::commands::admin_attract(),
                discord::commands::admin_game(),
                discord::commands::admin_complete(),
                discord::commands::admin_reset(),
                discord::commands::admin_schedule_start(),
                discord::commands::graveyard(),
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
                            } else if id == "merchant:shop" {
                                Some(handle_merchant_shop_button(ctx, component, data).await)
                            } else if id == "shop_select" {
                                Some(handle_shop_select(ctx, component, data).await)
                            } else if id.starts_with("shop_buy:") {
                                Some(handle_shop_buy(ctx, component, data).await)
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
                let activity_log = ActivityLogSync::new(
                    Arc::clone(&ctx.http),
                    channel_id,
                    activity_log_max_lines,
                    Duration::from_millis(activity_log_debounce_ms),
                );

                let (generation_tx, generation_rx) =
                    tokio::sync::mpsc::unbounded_channel::<GenerationJob>();

                let runtime = Arc::new(GameRuntime {
                    http: Arc::clone(&ctx.http),
                    cache: Arc::clone(&ctx.cache),
                    board,
                    job_queue,
                    item_registry,
                    merchant,
                    generator,
                    item_generator,
                    gravestone_generator,
                    merchant_generator,
                    activity_log,
                    pending_quests,
                    pending_merchant_catalog,
                    session,
                    generation_queue: generation_tx,
                    complete_tx: setup_complete_tx,
                    admin_user_id,
                    bot_user_id,
                    channel_id,
                    guild_id,
                    max_jobs,
                    max_job_queue,
                    max_non_bot_messages,
                    tick_time_s,
                    job_gen_low_threshold,
                    job_gen_high_threshold,
                    job_gen_min_history_msgs,
                    job_timeout_tick,
                });

                // Generation worker: spawned once and left running; it simply idles until a
                // job arrives. Every enqueue path is gated on the Playing phase, so attract /
                // complete produce no work.
                {
                    let worker_board = Arc::clone(&runtime.board);
                    let worker_queue = Arc::clone(&runtime.job_queue);
                    let worker_registry = Arc::clone(&runtime.item_registry);
                    let worker_generator = Arc::clone(&runtime.generator);
                    let worker_item_generator = Arc::clone(&runtime.item_generator);
                    let worker_merchant_generator = Arc::clone(&runtime.merchant_generator);
                    let worker_merchant = Arc::clone(&runtime.merchant);
                    let worker_http = Arc::clone(&runtime.http);
                    let worker_pending = Arc::clone(&runtime.pending_quests);
                    let worker_pending_merchant = Arc::clone(&runtime.pending_merchant_catalog);
                    let worker_session = Arc::clone(&runtime.session);
                    spawn_generation_worker(
                        generation_rx,
                        Arc::clone(&worker_board),
                        worker_queue,
                        worker_registry,
                        worker_merchant,
                        worker_generator,
                        worker_item_generator,
                        worker_merchant_generator,
                        max_jobs,
                        job_timeout_tick,
                        worker_pending,
                        worker_pending_merchant,
                        move |effects| {
                            let worker_http = Arc::clone(&worker_http);
                            let worker_board = Arc::clone(&worker_board);
                            let worker_session = Arc::clone(&worker_session);
                            Box::pin(async move {
                                if effects.iter().any(|e| {
                                    matches!(e, WorkerEffect::BoardRefilled { .. })
                                }) {
                                    // Attract/complete own the channel; don't redraw the board.
                                    if !worker_session.lock().await.is_playing() {
                                        return;
                                    }
                                    let mut b = worker_board.lock().await;
                                    if let Err(e) = update_board_message(
                                        &worker_http,
                                        channel_id,
                                        &mut b,
                                        max_jobs,
                                        None,
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

                let simulation = SimulationController::new(Arc::clone(&runtime));

                // Completion supervisor: the tick task signals here when the final story
                // mission is finished. Running off the tick task lets us stop the simulation
                // (which aborts that task) and then paint the complete screen cleanly.
                {
                    let sup_sim = Arc::clone(&simulation);
                    let sup_rt = Arc::clone(&runtime);
                    let mut complete_rx = complete_rx;
                    tokio::spawn(async move {
                        while let Some(completion) = complete_rx.recv().await {
                            tracing::info!(
                                user_id = completion.user_id,
                                chud = %completion.chud_name,
                                "final story mission complete; transitioning to Complete phase"
                            );
                            {
                                let mut session = sup_rt.session.lock().await;
                                session.phase = GamePhase::Complete;
                                session.final_completer_user_id = Some(completion.user_id);
                                session.final_completer_chud_name = Some(completion.chud_name);
                                if let Err(e) = storage::save_session(&session) {
                                    tracing::warn!(err = %e, "failed to save session on completion");
                                }
                            }
                            sup_sim.stop().await;
                            if let Err(e) = game_screens::post_complete_screen(&sup_rt).await {
                                tracing::warn!(err = %e, "failed to post complete screen");
                            }
                        }
                    });
                }

                // Phase-aware startup: only Playing brings up the simulation; attract and
                // complete just paint their screen with no ticks.
                let (phase, scheduled_start) = {
                    let session = runtime.session.lock().await;
                    (session.phase, session.game_start_at)
                };
                match phase {
                    GamePhase::Playing => {
                        {
                            let merchant_guard = runtime.merchant.lock().await;
                            chuds::game::engine::ensure_merchant_catalog_generation(
                                &merchant_guard,
                                Some(&runtime.generation_queue),
                                Some(&runtime.pending_merchant_catalog),
                            );
                        }
                        simulation.start().await?;
                    }
                    GamePhase::Attract => {
                        game_screens::post_attract_screen(&runtime).await?;
                        // Re-arm a previously scheduled start across restarts.
                        if let Some(at) = scheduled_start {
                            simulation.schedule_start(at).await;
                        }
                    }
                    GamePhase::Complete => game_screens::post_complete_screen(&runtime).await?,
                }

                Ok(Data {
                    runtime,
                    simulation,
                })
            })
        })
        .build();

    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::GUILD_PRESENCES;
    let mut client = serenity::ClientBuilder::new(&token, intents)
        .framework(framework)
        .await
        .map_err(|e| anyhow::anyhow!("failed to build Discord client: {e}"))?;

    tracing::info!("bot connected, listening for slash commands");
    tokio::select! {
        res = client.start() => {
            chuds::game::persistence::llm_memory::save_all_llm_memory(
                &shutdown_quest,
                &shutdown_item,
                &shutdown_gravestone,
                &shutdown_merchant,
            )?;
            res.map_err(|e| anyhow::anyhow!("Discord client error: {e}"))
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received, saving LLM memory");
            chuds::game::persistence::llm_memory::save_all_llm_memory(
                &shutdown_quest,
                &shutdown_item,
                &shutdown_gravestone,
                &shutdown_merchant,
            )?;
            Ok(())
        }
    }
}
