use std::sync::atomic::Ordering;
use std::time::Duration;

use poise::serenity_prelude as serenity;

use crate::discord::buttons::post_quest_taken_announcement;
use crate::discord::channel::{append_activity_log, append_activity_log_deferred};
use crate::discord::context::{admin_guard, require_playing, Context, Error, GameRuntime};
use crate::discord::game_screens;
use crate::discord::guild_hall::recover_persistent_board_messages;
use crate::discord::report_dm;
use crate::discord::tick;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::session::{GamePhase, GameSession};
use crate::game::engine::{self, DeathContext};
use crate::game::guild_status;
use crate::game::merchant::{ForcedSpawnError, MerchantState};
use crate::game::generation::memory::LlmMemorySlot;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::llm_memory;
use crate::game::persistence::message_cache::ActivityLogKind;
use crate::game::persistence::storage;

/// Parse an admin-supplied start time into unix seconds. Accepts a raw unix timestamp, an
/// RFC3339 datetime (e.g. `2026-06-10T18:00:00-07:00`), or a naive `YYYY-MM-DD HH:MM[:SS]`
/// which is interpreted as UTC.
fn parse_start_time(input: &str) -> Option<i64> {
    let s = input.trim();
    if let Ok(unix) = s.parse::<i64>() {
        return Some(unix);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive.and_utc().timestamp());
        }
    }
    None
}

#[poise::command(slash_command)]
pub async fn add_cm(ctx: Context<'_>, discord_user_id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let discord_user_id: u64 = match discord_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };
    let added = storage::add_chudmaster(discord_user_id)?;
    if !added {
        ctx.say("that user is already a Chudmaster™").await?;
        return Ok(());
    }
    let http = &ctx.serenity_context().http;
    let display_name = match http.get_user(serenity::UserId::new(discord_user_id)).await {
        Ok(user) => user.global_name.unwrap_or(user.name),
        Err(_) => discord_user_id.to_string(),
    };
    let content = format!("{} is now a Chudmaster™", display_name);
    append_activity_log(&ctx.data().runtime.activity_log, &content).await;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn delete_cm(ctx: Context<'_>, discord_user_id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let discord_user_id: u64 = match discord_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };
    let removed = storage::remove_chudmaster(discord_user_id)?;
    if !removed {
        ctx.say("that user is not a Chudmaster™").await?;
        return Ok(());
    }
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn admin_spawn_merchant(
    ctx: Context<'_>,
    #[description = "Merchant name to spawn (random if omitted)"] merchant_name: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    if !require_playing(ctx).await {
        return Ok(());
    }
    let mut merchant = ctx.data().runtime.merchant.lock().await;
    match merchant.set_spawn_flag(merchant_name.clone()) {
        Ok(()) => {
            let msg = match merchant_name {
                Some(name) => format!("ok, {name} will appear next tick"),
                None => "ok, a merchant will appear next tick".into(),
            };
            ctx.say(msg).await?;
        }
        Err(ForcedSpawnError::AlreadyVisiting) => {
            ctx.say("a merchant is already visiting").await?;
        }
        Err(ForcedSpawnError::NoCatalog) => {
            ctx.say("no merchant catalog loaded yet").await?;
        }
        Err(ForcedSpawnError::MerchantNotFound) => {
            let name = merchant_name.unwrap_or_default();
            ctx.say(format!("no merchant named \"{name}\"")).await?;
        }
        Err(ForcedSpawnError::MerchantEmpty) => {
            let name = merchant_name.unwrap_or_default();
            ctx.say(format!("{name} has no stock")).await?;
        }
    }
    Ok(())
}

#[poise::command(slash_command)]
pub async fn admin_redraw(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    if !require_playing(ctx).await {
        return Ok(());
    }
    let http = &ctx.serenity_context().http;
    let rt = &ctx.data().runtime;
    let mut board = rt.board.lock().await;
    let mut queue = rt.job_queue.lock().await;
    let merchant = rt.merchant.lock().await;
    recover_persistent_board_messages(
        http,
        rt.channel_id,
        &mut *board,
        &mut *queue,
        &merchant,
        rt.bot_user_id,
        rt.max_non_bot_messages,
        rt.max_jobs,
        rt.job_timeout_tick,
    )
    .await?;
    ctx.say("ok").await?;
    Ok(())
}

/// Switch the game into the Attract phase and post the attract screen (simulation off).
#[poise::command(slash_command)]
pub async fn admin_attract(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let data = ctx.data();
    data.simulation.stop().await;
    {
        let mut session = data.runtime.session.lock().await;
        session.phase = GamePhase::Attract;
        storage::save_session(&session)?;
    }
    game_screens::post_attract_screen(&data.runtime).await?;
    ctx.say("ok, switched to attract").await?;
    Ok(())
}

/// Switch the game into the Playing phase and start the simulation (normal game loop).
#[poise::command(slash_command)]
pub async fn admin_game(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let data = ctx.data();
    {
        let mut session = data.runtime.session.lock().await;
        session.phase = GamePhase::Playing;
        storage::save_session(&session)?;
    }
    data.simulation.start().await?;
    ctx.say("ok, switched to playing").await?;
    Ok(())
}

/// Switch the game into the Complete phase and post the game-over screen (simulation off).
#[poise::command(slash_command)]
pub async fn admin_complete(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let data = ctx.data();
    data.simulation.stop().await;
    {
        let mut session = data.runtime.session.lock().await;
        session.phase = GamePhase::Complete;
        storage::save_session(&session)?;
    }
    game_screens::post_complete_screen(&data.runtime).await?;
    ctx.say("ok, switched to complete").await?;
    Ok(())
}

/// Wait for in-flight LLM generation to finish so reset does not race the worker.
async fn wait_for_generation_idle(rt: &GameRuntime) {
    const MAX_WAIT_MS: u64 = 120_000;
    const POLL_MS: u64 = 100;
    let mut waited = 0;
    while waited < MAX_WAIT_MS {
        if rt.pending_quests.load(Ordering::SeqCst) == 0
            && rt.pending_auto_jobs.load(Ordering::SeqCst) == 0
            && rt.pending_merchant_catalog.load(Ordering::SeqCst) == 0
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
        waited += POLL_MS;
    }
    tracing::warn!("timed out waiting for generation worker before admin reset");
}

/// Reset to a brand-new game: wipe chuds, game state, and LLM memory, then enter attract.
#[poise::command(slash_command)]
pub async fn admin_reset(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let data = ctx.data();
    data.simulation.cancel_scheduled_start().await;
    data.simulation.stop().await;
    wait_for_generation_idle(&data.runtime).await;

    storage::reset_game_data()?;

    let rt = &data.runtime;
    *rt.board.lock().await = storage::fresh_board();
    *rt.job_queue.lock().await = JobQueue::default();
    *rt.merchant.lock().await = MerchantState::default();
    *rt.item_registry.lock().await = ItemRegistry::default();
    rt.pending_quests.store(0, Ordering::SeqCst);
    rt.pending_auto_jobs.store(0, Ordering::SeqCst);
    rt.pending_merchant_catalog.store(0, Ordering::SeqCst);
    let default_session = GameSession::default();
    *rt.session.lock().await = default_session.clone();
    llm_memory::clear_llm_memory(
        &rt.generator,
        &rt.item_generator,
        &rt.gravestone_generator,
        &rt.merchant_generator,
        LlmMemorySlot::All,
    )?;
    // Persist after in-memory reset so a finishing tick cannot leave stale session.yaml values.
    storage::save_session(&default_session)?;

    game_screens::post_attract_screen(rt).await?;
    ctx.say("ok, reset to a fresh game (attract)").await?;
    Ok(())
}

/// Schedule when the game leaves attract and enters playing (updates the attract countdown).
#[poise::command(slash_command)]
pub async fn admin_schedule_start(
    ctx: Context<'_>,
    #[description = "unix timestamp, RFC3339, or `YYYY-MM-DD HH:MM` (UTC)"] when: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let data = ctx.data();

    if data.runtime.session.lock().await.phase != GamePhase::Attract {
        ctx.say("you can only schedule a start while in the attract phase").await?;
        return Ok(());
    }

    let at_unix = match parse_start_time(&when) {
        Some(t) => t,
        None => {
            ctx.say(
                "couldn't parse that time. try a unix timestamp, `2026-06-10T18:00:00-07:00`, or `2026-06-10 18:00` (UTC)",
            )
            .await?;
            return Ok(());
        }
    };

    {
        let mut session = data.runtime.session.lock().await;
        session.game_start_at = Some(at_unix);
        storage::save_session(&session)?;
    }
    data.simulation.schedule_start(at_unix).await;
    game_screens::update_attract_screen(&data.runtime).await?;

    ctx.say(format!("ok, game starts <t:{at_unix}:R>")).await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn tick(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    if !require_playing(ctx).await {
        return Ok(());
    }
    tick::execute_tick(&ctx.data().runtime).await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn admin_take_gen_item(
    ctx: Context<'_>,
    target_user_id: String,
    quest_id: u32,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    if !require_playing(ctx).await {
        return Ok(());
    }
    let target_user_id: u64 = match target_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };
    let http = &ctx.serenity_context().http;
    let hospital = storage::load_hospital()?;
    let (board, status, info) = {
        let mut board = ctx.data().runtime.board.lock().await;
        let status = crate::game::guild_status::compute_guild_hall_status(&*board, &hospital)?;
        let info = engine::take_and_enqueue_quest(
            &mut *board,
            &hospital,
            target_user_id,
            quest_id,
            &ctx.data().runtime.generation_queue,
            true,
            Some(&status),
            ctx.data().runtime.job_timeout_tick,
        )?;
        let status = crate::game::guild_status::compute_guild_hall_status(&*board, &hospital)?;
        (board.clone(), status, info)
    };
    post_quest_taken_announcement(http, &board, ctx.data(), &info, &status).await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn add_chud(
    ctx: Context<'_>,
    target_user_id: String,
    name: String,
    description: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let target_user_id: u64 = match target_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };
    if ctx.data().runtime.session.lock().await.phase == GamePhase::Complete {
        ctx.say("the game is over").await?;
        return Ok(());
    }
    super::chud::handle_chud_join(&ctx.data().runtime, target_user_id, name, description).await?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn delete_chud(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    engine::delete_chud(ctx.author().id.get())?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn save(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let rt = &ctx.data().runtime;
    let board = rt.board.lock().await;
    let registry = rt.item_registry.lock().await;
    engine::save_all(&*board, &*registry)?;
    let merchant = rt.merchant.lock().await;
    storage::save_merchant_state(&merchant)?;
    {
        let session = rt.session.lock().await;
        storage::save_session(&session)?;
    }
    crate::game::persistence::llm_memory::save_all_llm_memory(
        &rt.generator,
        &rt.item_generator,
        &rt.gravestone_generator,
        &rt.merchant_generator,
    )?;
    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn admin_kill_chud(
    ctx: Context<'_>,
    target_user_id: String,
    #[description = "Trial situation that killed them"] death_trial: Option<String>,
    #[description = "Outcome that killed them"] death_outcome: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    if !require_playing(ctx).await {
        return Ok(());
    }
    let target_user_id: u64 = match target_user_id.trim().parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.say("invalid Discord user ID").await?;
            return Ok(());
        }
    };

    let hospital = storage::load_hospital()?;
    let board = ctx.data().runtime.board.lock().await;
    let status = guild_status::compute_guild_hall_status(&*board, &hospital)?;
    if status.is_busy(target_user_id) {
        ctx.say("chud is busy").await?;
        return Ok(());
    }
    drop(board);

    let death_ctx = DeathContext {
        trial: death_trial.unwrap_or_else(|| "Unknown peril".into()),
        outcome: death_outcome.unwrap_or_else(|| "They did not make it".into()),
    };

    let mut graveyard = storage::load_graveyard()?;
    let mut starting_benefits = storage::load_starting_benefits()?;
    let pending = {
        let registry = ctx.data().runtime.item_registry.lock().await;
        let mut merchant = ctx.data().runtime.merchant.lock().await;
        engine::kill_chud_apply(
            target_user_id,
            &death_ctx,
            &mut graveyard,
            &mut starting_benefits,
            &registry,
            &mut merchant,
        )?
    };
    let epitaph = engine::finish_gravestone_epitaph(
        &ctx.data().runtime.gravestone_generator,
        &pending,
    )
    .await?;
    let kill = engine::KillResult {
        discord_user_id: pending.discord_user_id,
        chud_name: pending.chud_name.clone(),
        benefits_awarded: pending.benefits_awarded,
        epitaph,
    };

    let first_name = kill
        .chud_name
        .split_whitespace()
        .next()
        .unwrap_or(&kill.chud_name)
        .to_string();
    let return_msg = crate::chud_msg!("return_died", first_name);
    append_activity_log_deferred(
        &ctx.data().runtime.activity_log,
        ActivityLogKind::Standard,
        &return_msg,
    )
    .await;

    report_dm::send_death_dm(&ctx.serenity_context().http, &kill, None).await;

    let board = {
        let guard = ctx.data().runtime.board.lock().await;
        guard.clone()
    };
    crate::discord::guild_hall::refresh_board_status(
        &ctx.serenity_context().http,
        ctx.data().runtime.channel_id,
        &board,
        ctx.data().runtime.max_jobs,
    )
    .await?;

    ctx.say("ok").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn load(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    if !admin_guard(ctx).await {
        return Ok(());
    }
    let (new_board, new_registry) = engine::load_all()?;
    let new_merchant = storage::load_merchant_state(&new_registry)?;
    let new_session = storage::load_session()?;
    let rt = &ctx.data().runtime;
    *rt.board.lock().await = new_board;
    *rt.item_registry.lock().await = new_registry;
    *rt.merchant.lock().await = new_merchant;
    *rt.session.lock().await = new_session;
    tracing::info!("board, item registry, guild hall, and session reloaded from disk");
    ctx.say("ok").await?;
    Ok(())
}
