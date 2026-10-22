use telegram_codex_jarvis::claude_code::ClaudeCode;
use telegram_codex_jarvis::chat_profiles::ChatProfileManager;
use telegram_codex_jarvis::config::Config;
use telegram_codex_jarvis::formatting::markdown_like_to_telegram_html;
use telegram_codex_jarvis::db::{
    Database, RecentChatMessage, StoredMessageInput, UpsertChatInput,
    UpsertUserInput,
};
use telegram_codex_jarvis::focus::{FocusDecision, FocusManager};
use telegram_codex_jarvis::metrics::BotMetrics;
use telegram_codex_jarvis::proactive;
use telegram_codex_jarvis::rag::{format_context, truncate, RagService};
use telegram_codex_jarvis::security::{SecurityDecision, SecurityGate};
use anyhow::{Context, Result};
use base64::Engine as _;
use std::collections::HashMap;
use std::sync::Arc;
use teloxide::dispatching::UpdateFilterExt;
use teloxide::error_handlers::LoggingErrorHandler;
use teloxide::prelude::*;
use teloxide::types::{Chat, ChatAction, ChatId, Message, MessageId, ParseMode, ReactionType, ThreadId};
use teloxide::update_listeners::{webhooks, UpdateListener};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::time::{sleep, Duration};
use tracing::{error, info, instrument, warn};

/// Collection window: wait this long after first message for rapid follow-ups.
const BATCH_COLLECT_MS: u64 = 1000;

/// Minimum seconds between active follow-up LLM evaluations per chat.
const FOLLOWUP_EVAL_COOLDOWN_SECS: i64 = 10;

/// Maximum bytes to download for image/media files.
const MAX_MEDIA_DOWNLOAD_BYTES: u64 = 5 * 1024 * 1024;

/// Maximum bytes to download for inline text file content.
const MAX_TEXT_FILE_BYTES: u64 = 64 * 1024;

/// Directory where ingested documents (PDFs, etc.) are saved so MCP tools
/// like `extract_pdf_text` can read them later. Shared with the MCP server's
/// own `DOWNLOAD_DIR` so both halves of the bot see the same files.
const DOCUMENT_STASH_DIR: &str = "/tmp/jarvis_downloads";

/// Maximum bytes to download for PDF documents on ingest.
const MAX_PDF_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024;

/// Maximum retry attempts when rate limited.
const MAX_RATE_LIMIT_RETRIES: u32 = 3;

#[derive(Debug, Clone)]
pub enum MediaAttachment {
    Image {
        base64_data: String,
        media_type: String,
        description: String,
    },
    FileDescription(String),
}

pub struct QueuedMessage {
    pub chat_id: ChatId,
    pub user_id: i64,
    pub sender_id: Option<i64>,
    pub text: String,
    pub reply_to_text: Option<String>,
    pub thread_id: Option<i32>,
    pub is_proactive: bool,
    pub message_id: Option<i64>,
    pub media: Option<MediaAttachment>,
}

/// Maximum number of concurrent proactive-trigger evaluator tasks. Each
/// evaluation spawns a Claude turn, so capping concurrency here protects the
/// subprocess from task storms when a chatty group fires rapid messages.
const PROACTIVE_CONCURRENCY_LIMIT: usize = 4;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: Database,
    pub rag: RagService,
    pub started_at: i64,
    pub claude: Arc<ClaudeCode>,
    pub bot_username: String,
    pub bot_user_id: i64,
    pub focus: Arc<FocusManager>,
    pub security: Arc<SecurityGate>,
    pub chat_profiles: Arc<ChatProfileManager>,
    pub metrics: Arc<BotMetrics>,
    /// Per-chat batcher senders for message debouncing.
    pub chat_batchers: Arc<Mutex<HashMap<i64, mpsc::UnboundedSender<QueuedMessage>>>>,
    /// Per-chat timestamp of last active follow-up LLM evaluation.
    pub followup_last_eval: Arc<Mutex<HashMap<i64, i64>>>,
    /// Backpressure for the proactive-trigger background tasks.
    pub proactive_semaphore: Arc<Semaphore>,
}

pub async fn run_bot(bot: Bot, state: Arc<AppState>) -> Result<()> {
    let handler = Update::filter_message().endpoint(handle_message);

    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(teloxide::dptree::deps![state.clone()])
        .enable_ctrlc_handler()
        .build();

    if let Some(webhook_url) = state.config.webhook_url.clone() {
        let bind_addr = state.config.webhook_bind;
        info!(
            "starting in webhook mode on {} with url {}",
            bind_addr, webhook_url
        );

        let options = webhooks::Options::new(bind_addr, webhook_url)
            .drop_pending_updates();
        let (mut update_listener, stop_flag, app) = webhooks::axum_to_router(bot.clone(), options)
            .await
            .context("failed to start webhook listener")?;

        // Webhook verification
        match bot.get_webhook_info().await {
            Ok(info) => {
                let url_str = info.url.as_ref().map(|u| u.as_str()).unwrap_or("(not set)");
                info!(
                    url = url_str,
                    pending = info.pending_update_count,
                    "webhook_info"
                );
                if let Some(err) = info.last_error_message.as_deref() {
                    warn!(error = err, "webhook_last_error");
                }
                if info.url.is_none() {
                    warn!("webhook_url_not_set");
                }
            }
            Err(e) => warn!(?e, "webhook_info_failed"),
        }

        // Merge /health route
        let health_state = Arc::clone(&state);
        let app = app.route("/health", axum::routing::get(move || {
            let st = Arc::clone(&health_state);
            async move {
                let now = epoch_now() as u64;
                let uptime = now.saturating_sub(st.started_at as u64);
                let hb_epoch = st.claude.heartbeat_epoch();
                let hb_ago = if hb_epoch > 0 { now.saturating_sub(hb_epoch) } else { 0 };
                let counters = st.metrics.to_json();
                let status = if st.claude.is_alive() { "ok" } else { "degraded" };

                axum::Json(serde_json::json!({
                    "status": status,
                    "uptime_secs": uptime,
                    "claude": {
                        "alive": st.claude.is_alive(),
                        "last_heartbeat_secs_ago": hb_ago,
                    },
                    "counters": counters,
                }))
            }
        }));

        // Merge dashboard routes (/dash + /api/*). Gated by the X-Dash-Auth
        // header inside each handler; disabled when DASHBOARD_AUTH_SECRET is unset.
        let app = app.merge(crate::dashboard::router(Arc::clone(&state)));

        let stop_token = update_listener.stop_token();
        tokio::spawn(async move {
            let tcp_listener = tokio::net::TcpListener::bind(bind_addr)
                .await
                .map_err(|err| {
                    stop_token.stop();
                    err
                })
                .expect("Couldn't bind to the address");
            axum::serve(tcp_listener, app)
                .with_graceful_shutdown(stop_flag)
                .await
                .map_err(|err| {
                    stop_token.stop();
                    err
                })
                .expect("Axum server error");
        });

        dispatcher
            .dispatch_with_listener(
                update_listener,
                LoggingErrorHandler::with_custom_text("update listener error"),
            )
            .await;
    } else {
        info!("starting in long-polling mode");
        dispatcher.dispatch().await;
    }

    Ok(())
}

async fn handle_message(bot: Bot, message: Message, state: Arc<AppState>) -> ResponseResult<()> {
    if let Err(err) = process_message(&bot, &message, &state).await {
        error!(?err, "message processing failed");
    }

    Ok(())
}

// ── Message processing pipeline ─────────────────────────────────────────────
//
// `process_message` is decomposed into four typed stages:
//
//   ingest_message  → classify_message  → pass_gates  → dispatch_message
//
// Each stage is a small function with one job, owning its own early-return
// logic and structured logging. Unit tests for the security/focus gates live
// against `classify_message` + `pass_gates` directly, without needing a real
// teloxide runtime.

/// Accumulated context for a message moving through the pipeline. Fields are
/// populated by stage 1 and consumed by later stages.
struct MessageCtx<'a> {
    message: &'a Message,
    chat_id: i64,
    message_id: i64,
    sender_id: i64,
    user_is_bot: bool,
    thread_id: Option<i64>,
    /// Unified text — either the message text or a synthesized description
    /// from a media attachment. Guaranteed non-empty if the ctx exists.
    text: String,
    media: Option<MediaAttachment>,
    /// Set in classify — whether this chat/topic is on the proactive monitoring list.
    is_monitored: bool,
    /// Set in classify — whether standard trigger rules fire (@mention, reply, alias).
    is_standard_trigger: bool,
}

/// Outcome of the classify stage. Decides which terminal branch the pipeline
/// takes after message storage is already committed.
enum Disposition {
    /// Stop processing — message is stored but no response path applies.
    Drop,
    /// Stop synchronous processing, but spawn a background proactive-trigger
    /// evaluator so Claude can decide whether to engage.
    SpawnProactive,
    /// Continue through gates, admin check, and batcher dispatch.
    Dispatch,
}

#[instrument(
    name = "process_message",
    skip(bot, state, message),
    fields(chat_id = message.chat.id.0, message_id = message.id.0)
)]
async fn process_message(bot: &Bot, message: &Message, state: &Arc<AppState>) -> Result<()> {
    let Some(mut ctx) = ingest_message(bot, message, state).await? else {
        return Ok(());
    };

    match classify_message(&mut ctx, state).await? {
        Disposition::Drop => return Ok(()),
        Disposition::SpawnProactive => {
            spawn_proactive_trigger_task(bot, state, &ctx);
            return Ok(());
        }
        Disposition::Dispatch => {}
    }

    if !pass_gates(bot, &ctx, state).await? {
        return Ok(());
    }

    dispatch_message(bot, ctx, state).await
}

/// Stage 1 — ingest: upsert chat/user, extract text+media, persist the row,
/// index for RAG. Returns `None` if the message is stale or has no content to
/// act on (drop it silently after indexing if appropriate).
#[instrument(name = "ingest", skip(bot, message, state))]
async fn ingest_message<'a>(
    bot: &Bot,
    message: &'a Message,
    state: &Arc<AppState>,
) -> Result<Option<MessageCtx<'a>>> {
    let seen_at = message.date.timestamp();
    let chat_id = message.chat.id.0;

    // Skip stale messages from before this process started (e.g. queued
    // updates that arrive after a restart).
    if seen_at < state.started_at {
        info!(chat_id, "msg_skipped_stale");
        state.metrics.inc(&state.metrics.messages_dropped_stale);
        return Ok(None);
    }

    let message_id = i64::from(message.id.0);
    info!(chat_id, message_id, "msg_received");
    state.metrics.inc(&state.metrics.messages_received);
    state.metrics.set_now(&state.metrics.last_message_at);

    state
        .db
        .upsert_chat(&UpsertChatInput {
            chat_id,
            kind: chat_kind(&message.chat).to_string(),
            title: message.chat.title().map(|v| v.to_string()),
            username: message.chat.username().map(|v| v.to_string()),
            seen_at,
        })
        .await?;

    let sender = message.from.as_ref();
    let sender_id = sender.and_then(|u| i64::try_from(u.id.0).ok());

    if let Some(user) = sender {
        if let Ok(user_id) = i64::try_from(user.id.0) {
            state
                .db
                .upsert_user(&UpsertUserInput {
                    user_id,
                    username: user.username.clone(),
                    first_name: user.first_name.clone(),
                    last_name: user.last_name.clone(),
                    is_bot: user.is_bot,
                    seen_at,
                })
                .await?;
        }
    }

    let content = message_text(message);
    let media = extract_media(bot, message).await;

    // Synthesize a textual representation for media-only messages so the DB
    // row and RAG index still have searchable content.
    let content_for_db = match (&content, &media) {
        (Some(text), _) => Some(text.clone()),
        (None, Some(MediaAttachment::Image { description, .. })) => {
            Some(format!("[{}]", description))
        }
        (None, Some(MediaAttachment::FileDescription(desc))) => Some(desc.clone()),
        (None, None) => None,
    };

    let raw_json = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());

    let stored_row_id = state
        .db
        .insert_or_update_message(&StoredMessageInput {
            chat_id,
            telegram_message_id: message_id,
            sender_id,
            content: content_for_db.clone(),
            raw_json,
            created_at: seen_at,
            ingested_at: epoch_now(),
            reply_to_message_id: message.reply_to_message().map(|m| i64::from(m.id.0)),
            message_link: build_message_link(&message.chat, message_id),
        })
        .await?;

    if let Some(text) = content_for_db.as_deref() {
        if let Err(err) = state.rag.index_message(stored_row_id, text).await {
            warn!(?err, "message indexing failed");
        }
    }

    // A message with no text, no user, or no numeric user id can be persisted
    // but has nothing else for us to do with it.
    let Some(text) = content_for_db else {
        return Ok(None);
    };
    let Some(user) = sender else {
        return Ok(None);
    };
    let Some(user_id) = sender_id else {
        return Ok(None);
    };

    Ok(Some(MessageCtx {
        message,
        chat_id,
        message_id,
        sender_id: user_id,
        user_is_bot: user.is_bot,
        thread_id: message.thread_id.map(|t| i64::from(t.0 .0)),
        text,
        media,
        is_monitored: false,
        is_standard_trigger: false,
    }))
}

/// Stage 2 — classify: decide whether this message should be dropped, should
/// spawn a background proactive-trigger evaluator, or should continue through
/// the gates. Populates `ctx.is_monitored` and `ctx.is_standard_trigger`.
#[instrument(name = "classify", skip(ctx, state), fields(chat_id = ctx.chat_id))]
async fn classify_message(
    ctx: &mut MessageCtx<'_>,
    state: &AppState,
) -> Result<Disposition> {
    let chat_id = ctx.chat_id;

    // Owner-only mode: the bot responds exclusively to the owner. Messages from
    // anyone else are already stored (ingest stage) for history/RAG, but never
    // reach a response, proactive, or monitored-group path. This also guarantees
    // owner-only tools (e.g. browser control) are unreachable by non-owners,
    // since only owner turns ever drive the shared Claude subprocess.
    if state.config.owner_user_id != Some(ctx.sender_id) {
        return Ok(Disposition::Drop);
    }

    ctx.is_monitored = state
        .db
        .is_monitored_group(chat_id, ctx.thread_id)
        .await
        .unwrap_or(false);

    // If this chat has ANY monitored entry but the current thread doesn't
    // match, suppress all responses — the bot only participates in the
    // monitored topic, even for @mentions and replies.
    if !ctx.is_monitored
        && state.db.chat_has_any_monitor(chat_id).await.unwrap_or(false)
    {
        return Ok(Disposition::Drop);
    }

    // Skip bot messages outside a monitored group.
    if ctx.user_is_bot && !ctx.is_monitored {
        return Ok(Disposition::Drop);
    }

    ctx.is_standard_trigger = !ctx.user_is_bot
        && should_respond(ctx.message, ctx.text.as_str(), state);
    if ctx.is_standard_trigger {
        info!(chat_id, user_id = ctx.sender_id, "trigger_matched");
    }

    if !ctx.is_standard_trigger && !ctx.is_monitored {
        // No direct trigger, not in a monitored group — consider the
        // proactive path (active follow-up OR cold LLM trigger).
        let followup_enabled = state.config.active_followup_secs > 0;
        let cold_enabled = state.config.proactive_cold_trigger_enabled;
        let is_group = ctx.message.chat.is_group() || ctx.message.chat.is_supergroup();
        if !ctx.user_is_bot && is_group && (followup_enabled || cold_enabled) {
            return Ok(Disposition::SpawnProactive);
        }
        return Ok(Disposition::Drop);
    }

    if !ctx.is_standard_trigger && ctx.is_monitored {
        return Ok(Disposition::Drop);
    }

    // Defensive: bots shouldn't reach here because `is_standard_trigger`
    // short-circuits on `user_is_bot`, but keep the check for clarity.
    if ctx.user_is_bot {
        return Ok(Disposition::Drop);
    }

    Ok(Disposition::Dispatch)
}

/// Spawn the proactive evaluator as a fire-and-forget task. The spawned task
/// owns cloned references so the caller can return immediately.
///
/// Uses `Arc<Semaphore>::try_acquire_owned()` for non-blocking backpressure:
/// when `PROACTIVE_CONCURRENCY_LIMIT` tasks are already running we log and
/// drop the event. A permit is held for the entire task lifetime and released
/// implicitly when the task ends.
fn spawn_proactive_trigger_task(bot: &Bot, state: &Arc<AppState>, ctx: &MessageCtx<'_>) {
    let permit = match Arc::clone(&state.proactive_semaphore).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            warn!(
                chat_id = ctx.chat_id,
                limit = PROACTIVE_CONCURRENCY_LIMIT,
                "proactive_trigger_dropped_no_permit"
            );
            return;
        }
    };

    let bot_clone = bot.clone();
    let state_clone = Arc::clone(state);
    let msg_text = ctx.text.clone();
    let chat_id = ctx.chat_id;
    let user_id = ctx.sender_id;
    let sender_id_opt = Some(ctx.sender_id);
    let msg_id = ctx.message_id;
    let thread = ctx.thread_id;
    tokio::spawn(async move {
        // Hold the permit for the entire task lifetime; drop at the end
        // releases it back to the pool.
        let _permit = permit;
        if let Err(e) = try_proactive_trigger(
            &bot_clone,
            &state_clone,
            chat_id,
            user_id,
            sender_id_opt,
            &msg_text,
            msg_id,
            thread,
        )
        .await
        {
            warn!(?e, "proactive-trigger error");
        }
    });
}

/// Stage 3 — gates: run the security gate (spam/rate/strike/system-query
/// block) and the focus gate. Returns `false` if a gate dropped the message
/// (already logged and, where relevant, replied to the user).
#[instrument(name = "gates", skip(bot, ctx, state), fields(chat_id = ctx.chat_id, user_id = ctx.sender_id))]
async fn pass_gates(bot: &Bot, ctx: &MessageCtx<'_>, state: &AppState) -> Result<bool> {
    let chat_id = ctx.chat_id;
    let user_id = ctx.sender_id;

    let security_decision = state
        .security
        .check_message(user_id, chat_id, &ctx.text, ctx.message.chat.is_private())
        .await?;
    match security_decision {
        SecurityDecision::Allow => {}
        SecurityDecision::SystemQueryBlocked => {
            info!(chat_id, user_id, "security_blocked_system_query");
            bot.send_message(
                ctx.message.chat.id,
                "Kechirasiz, tizim/arxitektura/ichki sozlamalar haqida ma'lumot bera olmayman.",
            )
            .await?;
            return Ok(false);
        }
        SecurityDecision::RateLimit => {
            info!(chat_id, user_id, "security_rate_limited");
            bot.send_message(
                ctx.message.chat.id,
                "Juda ko'p xabar yubordingiz. Biroz kutib turing.",
            )
            .await?;
            return Ok(false);
        }
        SecurityDecision::SpamFiltered => {
            warn!(chat_id, user_id, "security_spam_filtered");
            return Ok(false);
        }
        SecurityDecision::Banned => {
            info!(chat_id, user_id, "security_banned");
            return Ok(false);
        }
    }

    match state.focus.should_process(chat_id, user_id).await? {
        FocusDecision::Process | FocusDecision::Bypass => {}
        FocusDecision::Queue => {
            info!(chat_id, "focus_gated");
            return Ok(false);
        }
    }

    Ok(true)
}

/// Stage 4 — dispatch: handle admin command short-circuit, then push to the
/// per-chat batcher. The handler returns immediately; actual Claude work runs
/// inside `chat_batcher_task`.
#[instrument(name = "dispatch", skip(bot, ctx, state), fields(chat_id = ctx.chat_id))]
async fn dispatch_message(
    bot: &Bot,
    ctx: MessageCtx<'_>,
    state: &Arc<AppState>,
) -> Result<()> {
    if try_handle_bot_admin_command(bot, ctx.message, ctx.text.as_str(), ctx.sender_id, state)
        .await?
    {
        return Ok(());
    }

    let reply_to_text = ctx.message.reply_to_message().and_then(message_text);

    info!(chat_id = ctx.chat_id, "msg_queued");
    let tx = get_or_spawn_batcher(ctx.chat_id, bot.clone(), Arc::clone(state)).await;
    let _ = tx.send(QueuedMessage {
        chat_id: ctx.message.chat.id,
        user_id: ctx.sender_id,
        sender_id: Some(ctx.sender_id),
        text: ctx.text,
        reply_to_text,
        thread_id: ctx.thread_id.map(|t| t as i32),
        is_proactive: false,
        message_id: Some(ctx.message_id),
        media: ctx.media,
    });

    Ok(())
}

pub async fn get_or_spawn_batcher(
    chat_id: i64,
    bot: Bot,
    state: Arc<AppState>,
) -> mpsc::UnboundedSender<QueuedMessage> {
    let mut batchers = state.chat_batchers.lock().await;

    if let Some(existing) = batchers.get(&chat_id) {
        if !existing.is_closed() {
            return existing.clone();
        }
    }

    let (tx, rx) = mpsc::unbounded_channel();
    batchers.insert(chat_id, tx.clone());
    drop(batchers);

    tokio::spawn(chat_batcher_task(rx, bot, state));

    tx
}

async fn chat_batcher_task(
    mut rx: mpsc::UnboundedReceiver<QueuedMessage>,
    bot: Bot,
    state: Arc<AppState>,
) {
    loop {
        // Wait for first message. `recv()` returning None means every sender
        // has been dropped — log the closure so ops know the batcher exited
        // (was previously a silent exit).
        let Some(first) = rx.recv().await else {
            info!("chat_batcher_channel_closed");
            break;
        };

        // Dynamic collection window: longer for proactive messages (8s default)
        // to batch messages arriving 6-10s apart in monitored groups
        let collect_ms = if first.is_proactive {
            state.config.proactive_batch_collect_ms
        } else {
            BATCH_COLLECT_MS
        };
        sleep(Duration::from_millis(collect_ms)).await;

        // Drain any accumulated messages
        let mut batch = vec![first];
        while let Ok(msg) = rx.try_recv() {
            batch.push(msg);
        }

        let was_proactive = batch.iter().any(|m| m.is_proactive);

        // Try to generate response, with rate limit retry + accumulation
        let mut retries = 0u32;
        loop {
            let result = generate_and_send_response_batch(&bot, &state, &batch).await;
            match result {
                Ok(()) => break,
                Err(e) => {
                    if let Some(retry_secs) = parse_rate_limit_secs(&e) {
                        retries += 1;
                        if retries > MAX_RATE_LIMIT_RETRIES {
                            error!("rate limit: max retries ({MAX_RATE_LIMIT_RETRIES}) exceeded, giving up");
                            break;
                        }
                        warn!(retry_secs, retries, "rate limited, waiting then retrying with accumulated messages");
                        sleep(Duration::from_secs(retry_secs + 1)).await;
                        // Drain messages that arrived during the wait
                        while let Ok(msg) = rx.try_recv() {
                            batch.push(msg);
                        }
                        // Continue retry loop
                    } else {
                        error!(?e, "response generation failed");
                        break;
                    }
                }
            }
        }

        // Post-response drain for proactive batches: messages that arrived during
        // the ~18s Claude call are already in DB (visible via dialog window).
        // Wait briefly then drain them to prevent an immediate next cycle.
        if was_proactive {
            sleep(Duration::from_secs(2)).await;
            let mut drained = 0u32;
            while let Ok(_msg) = rx.try_recv() {
                drained += 1;
            }
            if drained > 0 {
                info!(drained, "post-response drain: discarded proactive follow-ups already in DB");
            }
        }
    }
}

/// Parse "Retry after Xs" from an error message, returning the delay in seconds.
fn parse_rate_limit_secs(err: &anyhow::Error) -> Option<u64> {
    let msg = format!("{err:#}");
    // Look for "Retry after Xs" pattern
    if let Some(idx) = msg.find("Retry after ") {
        let rest = &msg[idx + "Retry after ".len()..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(secs) = num_str.parse::<u64>() {
            return Some(secs.max(2)); // at least 2s
        }
    }
    // Also catch "rate limit" mentions without explicit retry time
    let lower = msg.to_ascii_lowercase();
    if lower.contains("rate limit") || lower.contains("too many requests") || lower.contains("429") {
        return Some(15); // default 15s backoff
    }
    None
}

/// Build the "incoming messages" section for the prompt from a batch of queued messages.
async fn build_incoming_section(state: &AppState, batch: &[QueuedMessage]) -> String {
    if batch.len() == 1 {
        let msg = &batch[0];
        if msg.is_proactive {
            return format!(
                "Guruhdagi oxirgi xabar (siz suhbatga qo'shilmoqdasiz):\n{}",
                msg.text
            );
        }
        return match msg.reply_to_text.as_deref() {
            Some(rt) => format!(
                "Incoming message (replying to: \"{}\"):\n{}",
                truncate(rt, 500),
                msg.text
            ),
            None => format!("Incoming message:\n{}", msg.text),
        };
    }

    // Multiple messages — build a list with sender names
    let mut lines = Vec::new();
    for msg in batch {
        let sender_name = if let Some(sid) = msg.sender_id {
            state.db.get_user_display_name(sid).await.ok().flatten()
                .unwrap_or_else(|| format!("id={sid}"))
        } else {
            "unknown".to_string()
        };
        lines.push(format!("{}: {}", sender_name, msg.text));
    }

    format!(
        "Bu xabarlar javob kutmoqda (bot band edi, barchasiga bitta javobda javob ber):\n{}",
        lines.join("\n")
    )
}

async fn generate_and_send_response_batch(
    bot: &Bot,
    state: &AppState,
    batch: &[QueuedMessage],
) -> Result<()> {
    // Use last message for chat/thread/media context
    let last = batch.last().unwrap();
    let chat_id = last.chat_id;
    let user_id = last.user_id;
    let sender_id = last.sender_id;
    let thread_id = last.thread_id;
    let message_id = last.message_id;
    let _is_proactive = batch.iter().any(|m| m.is_proactive);
    let batch_size = batch.len();
    info!(chat_id = chat_id.0, batch_size, "batch_start");
    state.metrics.inc(&state.metrics.claude_calls);
    let batch_start_time = std::time::Instant::now();

    // Collect all images from the batch
    let images: Vec<(String, String)> = batch.iter().filter_map(|m| {
        if let Some(MediaAttachment::Image { base64_data, media_type, .. }) = &m.media {
            Some((base64_data.clone(), media_type.clone()))
        } else {
            None
        }
    }).collect();

    bot.send_chat_action(chat_id, ChatAction::Typing).await?;

    let rag_query = &last.text;
    let retrieved = state
        .rag
        .search(rag_query)
        .await?;
    let context = format_context(&retrieved, state.config.max_context_chars);
    let recent_messages = state.db.fetch_recent_chat_messages_for_thread(
        chat_id.0, thread_id.map(|t| t as i64), 4
    ).await?;
    let recent_dialog = format_recent_dialog_window(
        &recent_messages,
        user_id,
        state.bot_user_id,
    );

    let chat_style = state.chat_profiles.get_profile(chat_id.0).await;
    let system_prompt = build_system_prompt(&state.config, sender_id, &recent_messages, chat_style.as_deref());

    let default_rules =
        "- Javob doim ozbek tilida bolsin.\n- Tarix umuman topilmasa, buni ochiq ayt; yetarli tarix bo'lsa 'tarix toliq emas' demang.";
    let configured_rules = state.config.user_rules_template.trim();
    let rules = if configured_rules.is_empty() {
        default_rules
    } else {
        configured_rules
    };

    let incoming_section = build_incoming_section(state, batch).await;

    let thread_id_str = thread_id.map(|t| t.to_string()).unwrap_or_else(|| "none".to_string());
    let message_id_str = message_id.map(|m| m.to_string()).unwrap_or_else(|| "none".to_string());

    let user_prompt = format!(
        "Chat: chat_id={} thread_id={} message_id={}\nRequester telegram_user_id={}\n{}\n\nRecent dialog window:\n{}\n\nRelevant history:\n{}\n\nRules:\n{}",
        chat_id.0,
        thread_id_str,
        message_id_str,
        sender_id.unwrap_or_default(),
        incoming_section,
        recent_dialog,
        context,
        rules
    );

    let answer = if !images.is_empty() {
        let refs: Vec<(&str, &str)> = images.iter().map(|(b, m)| (b.as_str(), m.as_str())).collect();
        state.claude.send_message_with_images(&system_prompt, &user_prompt, &refs, &format!("Responding · chat {}", chat_id.0)).await
    } else {
        state.claude.send_message(&system_prompt, &user_prompt, &format!("Responding · chat {}", chat_id.0)).await
    };

    let elapsed_ms = batch_start_time.elapsed().as_millis() as u64;
    // Record every turn in the histogram — success and error. Failed turns are
    // just as interesting for latency profiling (a 60s hang is the same shape
    // of problem whether it succeeded or timed out).
    state.metrics.claude_turn_duration.record(elapsed_ms);
    match &answer {
        Ok(_) => {
            info!(chat_id = chat_id.0, elapsed_ms, "claude_done");
            state.metrics.inc(&state.metrics.messages_responded);
            state.metrics.set_now(&state.metrics.last_response_at);
        }
        Err(e) => {
            error!(chat_id = chat_id.0, elapsed_ms, ?e, "claude_error");
            state.metrics.inc(&state.metrics.claude_errors);
        }
    }
    let reply = answer?;

    // Claude is told to deliver replies via bot_send_message and return
    // [BOT_SENT]/[SILENT]. If it instead returned plain text it never sent —
    // deliver that text as a fallback so the user always gets a reply.
    let trimmed = reply.trim();
    if !trimmed.is_empty() && !trimmed.contains("[BOT_SENT]") && !trimmed.contains("[SILENT]") {
        info!(chat_id = chat_id.0, "claude returned plain text; sending as fallback");
        let chars: Vec<char> = trimmed.chars().collect();
        for chunk in chars.chunks(4000) {
            let part: String = chunk.iter().collect();
            if let Err(e) = bot.send_message(chat_id, part).await {
                error!(chat_id = chat_id.0, ?e, "fallback send failed");
            }
        }
    }

    // Update proactive cooldown
    if state.db.chat_has_any_monitor(chat_id.0).await.unwrap_or(false) {
        let _ = state.db.update_proactive_cooldown(chat_id.0, Some(epoch_now()), None).await;
    }

    Ok(())
}

async fn try_proactive_trigger(
    bot: &Bot,
    state: &Arc<AppState>,
    chat_id: i64,
    user_id: i64,
    sender_id: Option<i64>,
    text: &str,
    message_id: i64,
    thread_id: Option<i64>,
) -> Result<()> {
    let now = epoch_now();

    // 1. Determine whether we're in the active follow-up window (warm) or cold.
    let bot_last = state
        .db
        .get_bot_last_message_at(chat_id, state.bot_user_id)
        .await?;
    let window = state.config.active_followup_secs as i64;
    let secs_ago: Option<i64> = match bot_last {
        Some(ts) if window > 0 && (now - ts) <= window => Some(now - ts),
        _ => None,
    };
    let is_cold = secs_ago.is_none();
    let mode = if is_cold { "cold" } else { "warm" };

    // 2. Cold path requires the feature flag + global cooldown to avoid LLM hammering.
    if is_cold {
        if !state.config.proactive_cold_trigger_enabled {
            return Ok(());
        }
        let on_cooldown = proactive::check_cooldown(
            &state.db,
            chat_id,
            false,
            state.config.proactive_cooldown_secs,
            state.config.proactive_bot_cooldown_secs,
        )
        .await
        .unwrap_or(false);
        if on_cooldown {
            info!(chat_id, mode, "proactive_cold_cooldown");
            return Ok(());
        }
    }

    // 3. Per-chat evaluation debouncer — prevents bursts in either mode.
    {
        let evals = state.followup_last_eval.lock().await;
        if let Some(&last_eval) = evals.get(&chat_id) {
            if now - last_eval < FOLLOWUP_EVAL_COOLDOWN_SECS {
                return Ok(());
            }
        }
    }

    // 4. Fetch recent messages for LLM context
    let recent = state
        .db
        .fetch_recent_chat_messages_with_names(chat_id, thread_id, 20)
        .await
        .unwrap_or_default();
    if recent.is_empty() {
        return Ok(());
    }

    // 5. LLM decision
    info!(chat_id, mode, "proactive_eval");
    let decision = match proactive::evaluate_proactive(
        &state.claude,
        &recent,
        state.bot_user_id,
        false,
        secs_ago,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            warn!(?e, mode, "proactive: evaluation failed");
            // Still update eval timestamp to avoid rapid retries
            state.followup_last_eval.lock().await.insert(chat_id, now);
            return Ok(());
        }
    };

    // 6. Always update eval timestamp
    state.followup_last_eval.lock().await.insert(chat_id, now);

    let chat_id_tg = ChatId(chat_id);

    match decision {
        proactive::ProactiveDecision::Respond(reason) => {
            info!(chat_id, mode, %reason, "proactive_respond");
            let tx = get_or_spawn_batcher(chat_id, bot.clone(), Arc::clone(state)).await;
            let _ = tx.send(QueuedMessage {
                chat_id: chat_id_tg,
                user_id,
                sender_id,
                text: text.to_string(),
                reply_to_text: None,
                thread_id: thread_id.map(|t| t as i32),
                is_proactive: true,
                message_id: Some(message_id),
                media: None,
            });
            let _ = state
                .db
                .update_proactive_cooldown(chat_id, Some(now), None)
                .await;
        }
        proactive::ProactiveDecision::React(emoji) => {
            info!(chat_id, mode, %emoji, "proactive_react");
            let _ = bot
                .set_message_reaction(chat_id_tg, MessageId(message_id as i32))
                .reaction(vec![ReactionType::Emoji { emoji }])
                .await;
            let _ = state
                .db
                .update_proactive_cooldown(chat_id, None, Some(now))
                .await;
        }
        proactive::ProactiveDecision::Silent => {
            info!(chat_id, mode, "proactive_silent");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Media extraction helpers
// ---------------------------------------------------------------------------

async fn extract_media(
    bot: &Bot,
    message: &Message,
) -> Option<MediaAttachment> {
    // Priority: photo > sticker > animation > video > document
    // Voice/video-note: metadata only (no transcription)
    if let Some(voice) = message.voice() {
        return Some(MediaAttachment::FileDescription(
            format!("[Ovozli xabar: {}s]", voice.duration)
        ));
    }

    if let Some(vnote) = message.video_note() {
        // Try thumbnail for visual context
        if let Some(thumb) = vnote.thumbnail.as_ref() {
            if let Ok(data) = download_telegram_file(bot, &thumb.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                let mime = guess_image_type(&data);
                return Some(MediaAttachment::Image {
                    base64_data: b64,
                    media_type: mime,
                    description: format!("Video xabar ({}s)", vnote.duration),
                });
            }
        }
        return Some(MediaAttachment::FileDescription(
            format!("[Video xabar: {}s]", vnote.duration)
        ));
    }

    if let Some(photos) = message.photo() {
        // Pick the largest photo
        if let Some(largest) = photos.iter().max_by_key(|p| p.width * p.height) {
            match download_telegram_file(bot, &largest.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                Ok(data) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    let mime = guess_image_type(&data);
                    return Some(MediaAttachment::Image {
                        base64_data: b64,
                        media_type: mime,
                        description: "Rasm".to_string(),
                    });
                }
                Err(e) => {
                    warn!(?e, "photo download failed");
                    return Some(MediaAttachment::FileDescription("[Rasm: yuklab bo'lmadi]".to_string()));
                }
            }
        }
    }

    if let Some(sticker) = message.sticker() {
        let emoji_hint = sticker.emoji.as_deref().unwrap_or("");
        let is_animated = sticker.is_animated() || sticker.is_video();

        if !is_animated {
            // Static sticker — download directly (WebP)
            match download_telegram_file(bot, &sticker.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                Ok(data) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    let mime = guess_image_type(&data);
                    return Some(MediaAttachment::Image {
                        base64_data: b64,
                        media_type: mime,
                        description: format!("Stiker (emoji: {})", emoji_hint),
                    });
                }
                Err(e) => warn!(?e, "sticker download failed"),
            }
        }

        // Animated/video sticker or download failed — try thumbnail
        if let Some(thumb) = sticker.thumbnail.as_ref() {
            if let Ok(data) = download_telegram_file(bot, &thumb.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                let mime = guess_image_type(&data);
                return Some(MediaAttachment::Image {
                    base64_data: b64,
                    media_type: mime,
                    description: format!("Stiker (emoji: {}, {})", emoji_hint, if is_animated { "animatsion" } else { "statik" }),
                });
            }
        }

        return Some(MediaAttachment::FileDescription(
            format!("[Stiker: emoji={}, set={}]", emoji_hint, sticker.set_name.as_deref().unwrap_or("?"))
        ));
    }

    if let Some(anim) = message.animation() {
        // GIF — use thumbnail
        if let Some(thumb) = anim.thumbnail.as_ref() {
            if let Ok(data) = download_telegram_file(bot, &thumb.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                let mime = guess_image_type(&data);
                return Some(MediaAttachment::Image {
                    base64_data: b64,
                    media_type: mime,
                    description: format!("GIF ({}s)", anim.duration),
                });
            }
        }
        return Some(MediaAttachment::FileDescription(
            format!("[GIF: {}s]", anim.duration)
        ));
    }

    if let Some(video) = message.video() {
        // Video — use thumbnail
        if let Some(thumb) = video.thumbnail.as_ref() {
            if let Ok(data) = download_telegram_file(bot, &thumb.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                let mime = guess_image_type(&data);
                let size_mb = video.file.size as f64 / (1024.0 * 1024.0);
                return Some(MediaAttachment::Image {
                    base64_data: b64,
                    media_type: mime,
                    description: format!("Video ({}s, {:.1}MB)", video.duration, size_mb),
                });
            }
        }
        let size_mb = video.file.size as f64 / (1024.0 * 1024.0);
        return Some(MediaAttachment::FileDescription(
            format!("[Video: {}s, {:.1}MB]", video.duration, size_mb)
        ));
    }

    if let Some(doc) = message.document() {
        let file_name = doc.file_name.as_deref().unwrap_or("unknown");
        let size = doc.file.size;
        let mime = doc.mime_type.as_ref().map(|m| m.to_string()).unwrap_or_default();

        // Image document
        if mime.starts_with("image/") && size <= MAX_MEDIA_DOWNLOAD_BYTES as u32 {
            if let Ok(data) = download_telegram_file(bot, &doc.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                let detected_mime = guess_image_type(&data);
                return Some(MediaAttachment::Image {
                    base64_data: b64,
                    media_type: detected_mime,
                    description: format!("Rasm fayl: {}", file_name),
                });
            }
        }

        // Small text document — inline content
        if is_text_mime(&mime) && (size as u64) <= MAX_TEXT_FILE_BYTES {
            if let Ok(data) = download_telegram_file(bot, &doc.file.id, MAX_TEXT_FILE_BYTES).await {
                if let Ok(text) = String::from_utf8(data) {
                    let trimmed = truncate(text.trim(), 8000);
                    return Some(MediaAttachment::FileDescription(
                        format!("[Fayl: {}]\n{}", file_name, trimmed)
                    ));
                }
            }
        }

        // PDF (or similar binary doc we want to keep): download to the
        // shared stash so Claude can call extract_pdf_text on it later.
        // We don't try to read the bytes here — that's the tool's job.
        let is_pdf = mime == "application/pdf" || file_name.to_lowercase().ends_with(".pdf");
        if is_pdf && (size as u64) <= MAX_PDF_DOWNLOAD_BYTES {
            if let Ok(data) = download_telegram_file(bot, &doc.file.id, MAX_PDF_DOWNLOAD_BYTES).await {
                let _ = tokio::fs::create_dir_all(DOCUMENT_STASH_DIR).await;
                // Make a deterministic, collision-resistant filename so
                // re-ingesting the same file overwrites instead of
                // accumulating garbage.
                let safe_name = file_name
                    .chars()
                    .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
                    .collect::<String>();
                let path = format!(
                    "{DOCUMENT_STASH_DIR}/{}_{}",
                    doc.file.id.chars().take(12).collect::<String>(),
                    safe_name
                );
                match tokio::fs::write(&path, &data).await {
                    Ok(_) => {
                        info!(chat_id = message.chat.id.0, ?path, "pdf_ingested");
                        let byte_len = data.len();
                        // Extract the text right here and inline it, so Claude
                        // reads it directly instead of having to call a tool
                        // (which it sometimes fumbles, reaching for a non-allowed
                        // built-in file reader and reporting a permission error).
                        #[cfg(feature = "pdf")]
                        {
                            let extract_path = path.clone();
                            let extracted = tokio::task::spawn_blocking(move || {
                                pdf_extract::extract_text(&extract_path).ok()
                            })
                            .await
                            .ok()
                            .flatten();
                            if let Some(text) = extracted {
                                if !text.trim().is_empty() {
                                    return Some(MediaAttachment::FileDescription(format!(
                                        "[PDF: {file_name} — extracted text below; full file at {path}]\n{}",
                                        truncate(text.trim(), 12000)
                                    )));
                                }
                            }
                        }
                        // Non-text/scanned PDF (or pdf feature disabled): hand
                        // Claude the path + tool hint as a fallback.
                        return Some(MediaAttachment::FileDescription(format!(
                            "[PDF ingested: {file_name} ({byte_len} bytes) saved to {path}. \
                             It may be a scanned/image PDF. Call extract_pdf_text with path=\"{path}\" to try reading it.]"
                        )));
                    }
                    Err(e) => {
                        warn!(?e, "pdf_stash_write_failed");
                    }
                }
            }
        }

        let size_fmt = if size > 1_000_000 {
            format!("{:.1}MB", size as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.0}KB", size as f64 / 1024.0)
        };
        return Some(MediaAttachment::FileDescription(
            format!("[Fayl: {}, {}, {}]", file_name, size_fmt, mime)
        ));
    }

    None
}

async fn download_telegram_file(bot: &Bot, file_id: &str, max_bytes: u64) -> Result<Vec<u8>> {
    use teloxide::net::Download;
    let file = bot.get_file(file_id).await?;
    let file_size = file.size as u64;
    if file_size > max_bytes {
        anyhow::bail!("file too large: {} bytes (max {})", file_size, max_bytes);
    }
    let mut data = Vec::with_capacity(file_size as usize);
    bot.download_file(&file.path, &mut data).await?;
    Ok(data)
}

fn guess_image_type(data: &[u8]) -> String {
    if data.len() >= 8 {
        if data.starts_with(b"\x89PNG") {
            return "image/png".to_string();
        }
        if data.starts_with(b"GIF8") {
            return "image/gif".to_string();
        }
        if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
            return "image/webp".to_string();
        }
    }
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        return "image/jpeg".to_string();
    }
    "image/jpeg".to_string()
}

fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/xml"
        || mime == "application/javascript"
        || mime == "application/x-yaml"
        || mime == "application/toml"
}

fn should_respond(message: &Message, text: &str, state: &AppState) -> bool {
    if message.chat.is_private() {
        return true;
    }

    if !(message.chat.is_group() || message.chat.is_supergroup()) {
        return false;
    }

    is_reply_to_bot(message, state.bot_user_id)
        || mentions_bot(text, state.bot_username.as_str())
        || contains_calling_name(text, state)
}

fn is_reply_to_bot(message: &Message, bot_user_id: i64) -> bool {
    let Some(reply) = message.reply_to_message() else {
        return false;
    };

    reply.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()) == Some(bot_user_id)
}

fn mentions_bot(text: &str, bot_username: &str) -> bool {
    if bot_username.trim().is_empty() {
        return false;
    }

    text.to_ascii_lowercase()
        .contains(&format!("@{}", bot_username.to_ascii_lowercase()))
}

fn contains_calling_name(text: &str, state: &AppState) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }

    for alias in trigger_aliases(state) {
        if alias.is_empty() {
            continue;
        }

        if normalized.contains(&alias) {
            return true;
        }
    }

    false
}

fn trigger_aliases(state: &AppState) -> Vec<String> {
    let mut aliases: Vec<String> = state
        .config
        .bot_aliases
        .iter()
        .map(|a| a.trim().to_ascii_lowercase())
        .filter(|a| !a.is_empty())
        .collect();

    let username = state.bot_username.trim().to_ascii_lowercase();
    if !username.is_empty() {
        aliases.push(username);
    }

    aliases.sort();
    aliases.dedup();
    aliases
}

fn build_system_prompt(
    config: &Config,
    requester_id: Option<i64>,
    recent_messages: &[RecentChatMessage],
    chat_style_instruction: Option<&str>,
) -> String {
    let mut prompt = config.system_prompt_template.trim().to_string();
    if !prompt.is_empty() && !prompt.ends_with(' ') {
        prompt.push(' ');
    }

    // Use rich per-chat profile if available, otherwise fall back to keyword-based tone detection
    if let Some(style) = chat_style_instruction {
        prompt.push_str(style);
        prompt.push(' ');
    } else {
        let tone = detect_chat_tone(recent_messages);
        match tone {
            ChatTone::Formal => {
                prompt.push_str("Bu rasmiy guruh. Professional, tizimli va hurmatli javob bering. 'Siz' shaklida murojaat qiling. ");
            }
            ChatTone::Casual => {
                prompt.push_str("Bu do'stona chat. Samimiy, qisqa va iliq javob bering. Humor mos bo'lsa ishlating. ");
            }
            ChatTone::Mixed => {}
        }
    }

    if let Some(owner) = config.owner_user_id {
        if requester_id == Some(owner) {
            prompt.push_str(
                "Bu foydalanuvchi bot egasi. Unga doim hurmatli, professional va natijaga yo'naltirilgan javob bering. Zarur va tabiiy holatda uni 'Boss' deb chaqiring, lekin ortiqcha takrorlamang. Roast, sarkazm yoki slangni faqat chat konteksti mos bo'lsa va hurmat saqlansa ishlating. Emojini faqat zarur holatda ishlating. Sizda Telegram MCP vositalari mavjud — egasi so'raganda ularni ishlatishingiz mumkin. ",
            );
        } else {
            prompt.push_str(&format!(
                "Telegram user id {owner} bot egasi. Egaga oid topshiriqlarni yuqori ustuvorlikda bajaring va egasi haqida hurmat bilan gapiring. MUHIM: Siz faqat umumiy yordamchi botsiz. Telegram akkaunt bilan ishlash (xabar yuborish, dialoglar, o'qilmagan xabarlar, kontaktlar) imkoniyatlaringiz haqida hech qachon gapmang va bu xususiyatlarni eslatmang. Bu imkoniyatlar mavjud emas deb biling. "
            ));
        }
    }

    prompt.push_str(
        "Agar suhbatga o'zingiz qo'shilayotgan bo'lsangiz (sizni hech kim chaqirmagan), \
         \"bu xabar menga yo'naltirilmagan\" yoki shunga o'xshash rad javobini HECH QACHON bermang. \
         Tabiiy ravishda suhbatga qo'shiling: foydali fikr bildiring, savol bering yoki mavzuni rivojlantiring. "
    );

    prompt
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ChatTone {
    Formal,
    Casual,
    Mixed,
}

fn detect_chat_tone(messages: &[RecentChatMessage]) -> ChatTone {
    if messages.len() < 3 {
        return ChatTone::Mixed;
    }

    let mut formal_signals = 0i32;
    let mut casual_signals = 0i32;

    for msg in messages.iter().rev().take(10) {
        let lower = msg.content.to_ascii_lowercase();

        // Formal indicators
        for kw in ["hurmatli", "assalomu", "iltimos", "marhamat", "rahmat", "sizga", "sizning"] {
            if lower.contains(kw) {
                formal_signals += 1;
            }
        }

        // Casual indicators
        for kw in ["bro", "aka", "uka", "haha", "lol", "xd", ")))", "😂", "🤣", "gap yo'q", "zo'r", "qalaysan"] {
            if lower.contains(kw) {
                casual_signals += 1;
            }
        }

        // Emoji density as casual signal
        let emoji_count = msg.content.chars().filter(|c| {
            let cp = *c as u32;
            (0x1F600..=0x1F64F).contains(&cp) || (0x1F300..=0x1F5FF).contains(&cp) || (0x2600..=0x27BF).contains(&cp)
        }).count();
        if emoji_count > 2 {
            casual_signals += 1;
        }
    }

    if formal_signals > casual_signals + 2 {
        ChatTone::Formal
    } else if casual_signals > formal_signals + 2 {
        ChatTone::Casual
    } else {
        ChatTone::Mixed
    }
}

fn message_text(message: &Message) -> Option<String> {
    let text = message
        .text()
        .or_else(|| message.caption())
        .map(|v| v.trim().to_string())?;

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn build_message_link(chat: &Chat, message_id: i64) -> Option<String> {
    if let Some(username) = chat.username() {
        return Some(format!("https://t.me/{username}/{message_id}"));
    }

    let chat_id = chat.id.0;
    if chat_id < -1_000_000_000_000 {
        let internal = (-chat_id) - 1_000_000_000_000;
        if internal > 0 {
            return Some(format!("https://t.me/c/{internal}/{message_id}"));
        }
    }

    None
}

fn chat_kind(chat: &Chat) -> &'static str {
    if chat.is_private() {
        "private"
    } else if chat.is_group() {
        "group"
    } else if chat.is_supergroup() {
        "supergroup"
    } else if chat.is_channel() {
        "channel"
    } else {
        "unknown"
    }
}

fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

async fn try_handle_bot_admin_command(
    bot: &Bot,
    message: &Message,
    text: &str,
    requester_id: i64,
    state: &AppState,
) -> Result<bool> {
    if !message.chat.is_private() {
        return Ok(false);
    }
    let Some(owner_id) = state.config.owner_user_id else {
        return Ok(false);
    };
    if requester_id != owner_id {
        return Ok(false);
    }

    let lower = text.trim().to_ascii_lowercase();

    let wants_name = is_name_change_request(&lower);
    let wants_desc = is_desc_change_request(&lower);

    if !wants_name && !wants_desc {
        // Fall through to photo check below
    } else {
        let mut reply_parts: Vec<String> = Vec::new();

        if wants_name {
            match extract_value_via_llm(&state.claude, "ism", text).await {
                Ok(new_name) => {
                    info!(name = %new_name, len = new_name.len(), bytes = ?new_name.as_bytes(), "setting bot name");
                    match bot.set_my_name().name(new_name.clone()).await {
                        Ok(_) => reply_parts.push(format!("Ism o'zgartirildi: {new_name}")),
                        Err(e) => reply_parts.push(format!("Ism o'zgartirib bo'lmadi: {e}")),
                    }
                }
                Err(e) => reply_parts.push(format!("Ismni aniqlab bo'lmadi: {e}")),
            }
        }

        if wants_desc {
            match extract_value_via_llm(&state.claude, "tavsif (description)", text).await {
                Ok(new_desc) => match bot.set_my_description().description(new_desc.clone()).await {
                    Ok(_) => reply_parts.push(format!("Tavsif o'zgartirildi: {new_desc}")),
                    Err(e) => reply_parts.push(format!("Tavsif o'zgartirib bo'lmadi: {e}")),
                },
                Err(e) => reply_parts.push(format!("Tavsifni aniqlab bo'lmadi: {e}")),
            }
        }

        bot.send_message(message.chat.id, reply_parts.join("\n"))
            .await?;
        return Ok(true);
    }

    // !setphoto with attached photo
    if lower.starts_with("!setphoto")
        || lower.contains("rasmingni o'zgartir")
        || lower.contains("avatarni o'zgartir")
        || lower.contains("suratni o'zgartir")
        || lower.contains("change photo")
        || lower.contains("change avatar")
    {
        if let Some(photos) = message.photo() {
            if let Some(largest) = photos.iter().max_by_key(|p| p.width * p.height) {
                match download_telegram_file(bot, &largest.file.id, MAX_MEDIA_DOWNLOAD_BYTES).await {
                    Ok(data) => {
                        let input_file = teloxide::types::InputFile::memory(data).file_name("photo.jpg");
                        // Bot API doesn't support changing bot's own profile photo
                        bot.send_message(
                            message.chat.id,
                            "Telegram Bot API bot profil rasmini o'zgartirishni qo'llab-quvvatlamaydi. @BotFather ga /setuserpic buyrug'ini yuboring.",
                        )
                        .await?;
                        let _ = input_file; // suppress unused warning
                        return Ok(true);
                    }
                    Err(e) => {
                        bot.send_message(message.chat.id, format!("Rasm yuklab bo'lmadi: {e}"))
                            .await?;
                        return Ok(true);
                    }
                }
            }
        }
        bot.send_message(
            message.chat.id,
            "Rasm biriktiring yoki @BotFather da /setuserpic ishlating.",
        )
        .await?;
        return Ok(true);
    }

    Ok(false)
}

fn is_name_change_request(lower: &str) -> bool {
    lower.contains("ismingni") || lower.contains("nomingni")
        || lower.starts_with("!setname")
        || lower.contains("change name") || lower.contains("set name")
        || lower.contains("change your name") || lower.contains("rename yourself")
}

fn is_desc_change_request(lower: &str) -> bool {
    lower.contains("tavsifingni") || lower.contains("description")
        || lower.starts_with("!setdesc")
        || lower.contains("change desc") || lower.contains("set desc")
}

/// Ask Claude to extract a value from a natural language command.
/// Uses the main streaming subprocess to avoid rate limit contention.
async fn extract_value_via_llm(claude: &ClaudeCode, kind: &str, user_message: &str) -> Result<String> {
    let system = format!(
        "Foydalanuvchi Telegram botning {kind}ini o'zgartirmoqchi. \
         Xabardan faqat yangi {kind} qiymatini chiqar. \
         FAQAT qiymatni qaytar, hech qanday izoh, qo'shimcha so'z yoki tirnoq qo'shma."
    );
    let result = claude.send_message(&system, user_message, &format!("Extracting {kind}")).await?;
    // Strip quotes, markdown, control chars
    let cleaned = result
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .trim_matches('*')
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    info!(kind, raw = %result, cleaned = %cleaned, "LLM extraction result");
    if cleaned.is_empty() {
        anyhow::bail!("LLM returned empty extraction");
    }
    Ok(cleaned)
}

fn format_recent_dialog_window(
    messages: &[RecentChatMessage],
    requester_id: i64,
    bot_user_id: i64,
) -> String {
    if messages.is_empty() {
        return "(recent dialog topilmadi)".to_string();
    }

    // Build lookup: telegram_message_id → (speaker, content snippet)
    let mut lookup: HashMap<i64, (&str, String)> = HashMap::new();
    for msg in messages {
        let speaker = if msg.sender_id == Some(bot_user_id) {
            "Jarvis"
        } else if msg.sender_id == Some(requester_id) {
            "Siz"
        } else {
            "Ishtirokchi"
        };
        let snippet = truncate(&msg.content, 60);
        lookup.insert(msg.telegram_message_id, (speaker, snippet));
    }

    let mut out = String::new();
    for msg in messages {
        let speaker = if msg.sender_id == Some(bot_user_id) {
            "Jarvis"
        } else if msg.sender_id == Some(requester_id) {
            "Siz"
        } else {
            "Ishtirokchi"
        };

        let reply_tag = msg.reply_to_message_id.map(|reply_id| {
            if let Some((ref_speaker, ref_snippet)) = lookup.get(&reply_id) {
                format!(" (↩ {}: \"{}\")", ref_speaker, ref_snippet)
            } else {
                " (↩ earlier message)".to_string()
            }
        });

        out.push_str(&format!(
            "[{}] {}{}: {}\n",
            msg.created_at,
            speaker,
            reply_tag.as_deref().unwrap_or(""),
            msg.content
        ));
    }

    truncate(out.trim_end(), 2800)
}


async fn send_markdown_like_message(bot: &Bot, chat_id: ChatId, text: &str, thread_id: Option<i32>) -> Result<Message> {
    let rendered = markdown_like_to_telegram_html(text);
    let mut req = bot.send_message(chat_id, rendered)
        .parse_mode(ParseMode::Html);
    if let Some(tid) = thread_id {
        req = req.message_thread_id(ThreadId(MessageId(tid)));
    }
    let sent = req.await?;
    Ok(sent)
}

// markdown_like_to_telegram_html, escape_html, apply_double_asterisk_bold
// moved to telegram_codex_jarvis::formatting
