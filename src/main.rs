mod bot;
mod dashboard;

use anyhow::{Context, Result};
use bot::{run_bot, AppState};
use std::collections::HashMap;
use std::sync::Arc;
use telegram_codex_jarvis::chat_profiles::ChatProfileManager;
use telegram_codex_jarvis::claude_code::{ClaudeCode, ClaudeCodeConfig, SubprocessEvent};
use telegram_codex_jarvis::config::Config;
use telegram_codex_jarvis::db::Database;
use telegram_codex_jarvis::focus::FocusManager;
use telegram_codex_jarvis::metrics::BotMetrics;
use telegram_codex_jarvis::rag::RagService;
use telegram_codex_jarvis::reminders::ReminderService;
use telegram_codex_jarvis::scheduled_tasks::ScheduledTaskService;
use telegram_codex_jarvis::security::SecurityGate;
use teloxide::prelude::*;
use tokio::sync::Mutex;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = Config::from_env()?;
    let db = Database::connect(&config.database_url).await?;
    db.init().await?;

    let rag = RagService::new(db.clone(), config.rag_top_k, config.rag_candidate_limit);

    let claude = Arc::new(ClaudeCode::new(ClaudeCodeConfig {
        binary: config.claude_code_bin.clone(),
        model: config.claude_code_model.clone(),
        timeout_secs: config.claude_code_timeout_secs,
        allowed_tools: config.claude_code_allowed_tools.clone(),
        session_file: config.claude_code_session_file.clone(),
        default_system_prompt: config.system_prompt_template.trim().to_string(),
        session_max_turns: config.claude_session_max_turns,
        session_max_duration_ms: config.claude_session_max_duration_ms,
        session_duration_window: config.claude_session_duration_window,
    }));

    // Claude subprocess event channel (compression notifications, unknown events)
    let (claude_event_tx, claude_event_rx) = tokio::sync::mpsc::unbounded_channel::<SubprocessEvent>();
    claude.set_event_sender(claude_event_tx).await;

    let bot_token = config
        .telegram_bot_token
        .clone()
        .context("TELOXIDE_TOKEN is required")?;
    let bot = Bot::new(bot_token);
    let me = bot.get_me().await?;
    let bot_username = me.user.username.clone().unwrap_or_default();
    let bot_user_id = i64::try_from(me.user.id.0).unwrap_or_default();
    info!("bot user: {} ({})", bot_username, bot_user_id);

    let focus = Arc::new(FocusManager::new(
        db.clone(),
        config.owner_user_id,
        config.allowed_users.clone(),
    ));

    let security = Arc::new(SecurityGate::new(
        db.clone(),
        config.owner_user_id,
        config.allowed_users.clone(),
    ));

    // Seed monitored groups from env var
    if !config.monitored_groups.is_empty() {
        db.seed_monitored_groups(&config.monitored_groups).await?;
        info!(
            "seeded {} monitored groups from env",
            config.monitored_groups.len()
        );
    }

    // Spawn reminder service
    let reminder_service = ReminderService::new(db.clone(), bot.clone());
    tokio::spawn(async move {
        reminder_service.run().await;
    });

    // Spawn scheduled task service (cron jobs)
    let sched_service = ScheduledTaskService::new(
        db.clone(),
        bot.clone(),
        Arc::clone(&claude),
    );
    tokio::spawn(async move {
        sched_service.run().await;
    });

    // Chat style profile manager
    let chat_profiles = Arc::new(ChatProfileManager::new(
        db.clone(),
        Arc::clone(&claude),
    ));
    {
        let profiles = Arc::clone(&chat_profiles);
        tokio::spawn(async move { profiles.run_background().await; });
    }

    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let metrics = Arc::new(BotMetrics::new());

    let owner_user_id = config.owner_user_id;
    let has_webhook = config.webhook_url.is_some();

    let state = Arc::new(AppState {
        config,
        db,
        rag,
        started_at,
        claude,
        bot_username,
        bot_user_id,
        focus,
        security,
        chat_profiles,
        metrics: Arc::clone(&metrics),
        chat_batchers: Arc::new(Mutex::new(HashMap::new())),
        followup_last_eval: Arc::new(Mutex::new(HashMap::new())),
        proactive_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
    });

    // Periodic heartbeat recording (every 60s)
    {
        let db = state.db.clone();
        let metrics = Arc::clone(&metrics);
        let claude = Arc::clone(&state.claude);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let details = serde_json::json!({
                    "claude_alive": claude.is_alive(),
                    "session_turns": claude.session_turn_count(),
                    "counters": metrics.to_json(),
                });
                if let Err(e) = db.record_heartbeat("bot", Some(&details.to_string())).await {
                    warn!(?e, "heartbeat_record_failed");
                }
            }
        });
    }

    if owner_user_id.is_none() {
        warn!("owner_user_id_unset: set OWNER_USER_ID, the bot ignores every message without it");
    }

    // Owner startup notification
    if let Some(owner_id) = owner_user_id {
        let last_hb = state.db.get_recent_heartbeats(1).await.unwrap_or_default();
        let restart_info = if let Some((ts, source, _)) = last_hb.first() {
            let age = started_at - ts;
            if source == "clean_shutdown" {
                format!("Clean restart (last shutdown {}s ago)", age)
            } else {
                format!("Crash restart (last heartbeat {}s ago, source: {})", age, source)
            }
        } else {
            "First start (no prior heartbeats)".to_string()
        };

        let mode = if has_webhook { "webhook" } else { "long-polling" };
        let msg = format!(
            "Jarvis started\n{}\nMode: {}",
            restart_info, mode
        );

        let chat_id = ChatId(owner_id);
        if let Err(e) = bot.send_message(chat_id, &msg).await {
            warn!(?e, "startup_notification_failed");
        }
    }

    // Claude subprocess event receiver: persists per-turn metrics + events to the
    // DB (always), and forwards notable events to the owner DM (if configured).
    {
        let bot_clone = bot.clone();
        let db = state.db.clone();
        tokio::spawn(run_claude_event_receiver(
            claude_event_rx,
            bot_clone,
            db,
            owner_user_id,
        ));
    }

    // SIGTERM handler: record clean_shutdown heartbeat before process exits.
    // Without this, WSL2 restarts (which send SIGTERM via systemd) produce
    // false "Crash restart" notifications because the process exits before
    // reaching the clean_shutdown heartbeat at the end of main().
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            let mut sigterm = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            )
            .expect("failed to install SIGTERM handler");
            sigterm.recv().await;
            info!("received SIGTERM, recording clean shutdown");
            let _ = db.record_heartbeat("clean_shutdown", None).await;
            std::process::exit(0);
        });
    }

    run_bot(bot, Arc::clone(&state)).await?;

    // Record clean shutdown heartbeat
    let _ = state.db.record_heartbeat("clean_shutdown", None).await;

    Ok(())
}

/// Receives events from the Claude Code subprocess: persists per-turn metrics and
/// session events to the DB (always), and forwards notable events to the owner DM.
async fn run_claude_event_receiver(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<SubprocessEvent>,
    bot: Bot,
    db: Database,
    owner_id: Option<i64>,
) {
    while let Some(event) = rx.recv().await {
        // Durable persistence first (independent of owner notifications).
        match &event {
            SubprocessEvent::TurnMetrics {
                session_id,
                turn_num,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                cost_usd,
                duration_ms,
                duration_api_ms,
            } => {
                if let Err(e) = db
                    .insert_claude_turn(
                        session_id.as_deref(),
                        Some(*turn_num as i64),
                        *input_tokens as i64,
                        *output_tokens as i64,
                        *cache_creation_tokens as i64,
                        *cache_read_tokens as i64,
                        *cost_usd,
                        duration_ms.map(|v| v as i64),
                        duration_api_ms.map(|v| v as i64),
                    )
                    .await
                {
                    warn!(?e, "claude_turn_persist_failed");
                }
                continue; // never surfaced to owner
            }
            SubprocessEvent::Compression(raw) => {
                let _ = db.insert_claude_event("compaction", Some(raw)).await;
            }
            SubprocessEvent::SessionRotation => {
                let _ = db.insert_claude_event("rotation", None).await;
            }
            SubprocessEvent::Stderr(_) => {}
        }

        // Owner DM (only when an owner is configured).
        let Some(owner_id) = owner_id else { continue };
        let msg = match &event {
            SubprocessEvent::Compression(raw) => {
                format!("Claude context compression detected:\n{}", clip_str(raw, 500))
            }
            SubprocessEvent::SessionRotation => {
                "Claude session rotated (fresh context started)".to_string()
            }
            SubprocessEvent::Stderr(line) => {
                format!("⚠️ Claude subprocess stderr:\n{}", clip_str(line, 500))
            }
            SubprocessEvent::TurnMetrics { .. } => continue,
        };
        if let Err(e) = bot.send_message(ChatId(owner_id), &msg).await {
            warn!(?e, "claude_event_notification_failed");
        }
    }
}

fn clip_str(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}
