use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::discord::channel::{cleanup_non_bot_messages, validate_cached_messages_exist};
use crate::discord::context::GameRuntime;
use crate::discord::guild_hall::{recover_persistent_board_messages, update_board_message};
use crate::discord::tick::execute_tick;
use crate::discord::ui::update_merchant_message;
use crate::game::domain::session::GamePhase;
use crate::game::engine;
use crate::game::persistence::storage;

/// Owns the live simulation: the background tick loop. Started when entering the `Playing`
/// phase and stopped when leaving it, so attract/complete run no ticks at all.
///
/// The generation worker is spawned once at startup and simply idles when no jobs arrive;
/// every job-enqueue path is gated on the `Playing` phase, so it stays quiet otherwise.
pub struct SimulationController {
    runtime: Arc<GameRuntime>,
    tick_handle: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    start_handle: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl SimulationController {
    pub fn new(runtime: Arc<GameRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            tick_handle: tokio::sync::Mutex::new(None),
            start_handle: tokio::sync::Mutex::new(None),
        })
    }

    /// True if the background tick loop is currently running.
    pub async fn is_running(&self) -> bool {
        self.tick_handle.lock().await.is_some()
    }

    /// Arm a timer that transitions Attract -> Playing at `at_unix` (unix seconds). Replaces
    /// any previously scheduled start. The timer no-ops if the game has left Attract by then.
    pub async fn schedule_start(self: &Arc<Self>, at_unix: i64) {
        self.cancel_scheduled_start().await;
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let now = chrono::Utc::now().timestamp();
            let delay = (at_unix - now).max(0) as u64;
            tracing::info!(at_unix, delay_s = delay, "game start scheduled");
            tokio::time::sleep(Duration::from_secs(delay)).await;

            {
                let mut session = this.runtime.session.lock().await;
                if session.phase != GamePhase::Attract {
                    tracing::info!("scheduled start fired but game is no longer in attract; ignoring");
                    return;
                }
                session.phase = GamePhase::Playing;
                if let Err(e) = storage::save_session(&session) {
                    tracing::warn!(err = %e, "failed to save session on scheduled start");
                }
            }
            tracing::info!("scheduled start firing: entering playing phase");
            if let Err(e) = this.start().await {
                tracing::error!(err = %e, "failed to start simulation on scheduled start");
            }
        });
        *self.start_handle.lock().await = Some(handle);
    }

    /// Cancel a pending scheduled start, if any.
    pub async fn cancel_scheduled_start(&self) {
        if let Some(handle) = self.start_handle.lock().await.take() {
            handle.abort();
            tracing::info!("scheduled start cancelled");
        }
    }

    /// Bring the simulation up: rebuild/refresh the job board channel and spawn the tick loop.
    /// Idempotent — any existing tick loop is stopped first.
    pub async fn start(&self) -> anyhow::Result<()> {
        self.stop().await;
        self.bootstrap_channel().await?;

        let runtime = Arc::clone(&self.runtime);
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(runtime.tick_time_s));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(e) = execute_tick(&runtime).await {
                    tracing::error!(err = %e, "background tick failed");
                }
            }
        });
        *self.tick_handle.lock().await = Some(handle);
        tracing::info!("simulation started");
        Ok(())
    }

    /// Stop the background tick loop. Safe to call when already stopped.
    pub async fn stop(&self) {
        if let Some(handle) = self.tick_handle.lock().await.take() {
            handle.abort();
            tracing::info!("simulation tick loop stopped");
        }
    }

    /// Validate the cached board UI (recovering the channel if stale), then refill and render.
    async fn bootstrap_channel(&self) -> anyhow::Result<()> {
        let rt = &self.runtime;

        let messages_exist = {
            let _cache_guard = storage::message_cache_lock().await;
            let cache = storage::load_message_cache().unwrap_or_default();
            // A lingering phase screen (from attract/complete) means the board UI was torn
            // down: force a full recover so the screen is purged before we rebuild the board.
            cache.phase_screen_message_id.is_none()
                && validate_cached_messages_exist(&rt.http, rt.channel_id, &cache).await
        };

        {
            let mut board = rt.board.lock().await;
            let mut queue = rt.job_queue.lock().await;

            if !messages_exist {
                let merchant = rt.merchant.lock().await;
                recover_persistent_board_messages(
                    &rt.http,
                    rt.channel_id,
                    &mut board,
                    &mut queue,
                    &merchant,
                    rt.bot_user_id,
                    rt.max_non_bot_messages,
                    rt.max_jobs,
                    rt.job_timeout_tick,
                )
                .await?;
            } else {
                cleanup_non_bot_messages(
                    &rt.http,
                    rt.channel_id,
                    rt.bot_user_id,
                    rt.max_non_bot_messages,
                )
                .await;
            }

            engine::refill_board(
                &mut board,
                &mut queue,
                rt.max_jobs,
                rt.job_timeout_tick,
                Some(&rt.generation_queue),
                Some(&rt.pending_quests),
            );
            storage::save_board(&board)?;
            storage::save_job_queue(&queue)?;
            update_board_message(&rt.http, rt.channel_id, &board, rt.max_jobs, None).await?;
        }

        let merchant = rt.merchant.lock().await;
        update_merchant_message(&rt.http, rt.channel_id, &merchant).await?;
        Ok(())
    }
}
