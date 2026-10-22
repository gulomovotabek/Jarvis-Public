use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{Duration, timeout};
use tracing::{debug, info, warn};

/// Events emitted by the Claude Code subprocess stdout stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    #[serde(rename = "system")]
    System {
        subtype: Option<String>,
        session_id: Option<String>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        subtype: Option<String>,
        #[serde(default)]
        message: Option<AssistantMessage>,
    },
    #[serde(rename = "result")]
    Result {
        subtype: Option<String>,
        #[serde(default)]
        result: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        // The CLI emits `total_cost_usd`; older/proxy variants use `cost_usd`.
        #[serde(default, alias = "total_cost_usd")]
        cost_usd: Option<f64>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        duration_api_ms: Option<u64>,
        #[serde(default)]
        usage: Option<Usage>,
    },
    #[serde(other)]
    Unknown,
}

/// Token usage reported in the `result` event's `usage` object.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        #[serde(default)]
        name: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default)]
        content: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// Rich result from a single Claude turn, including cost/duration/token metrics.
#[derive(Debug, Clone)]
struct TurnResult {
    text: String,
    cost_usd: Option<f64>,
    duration_ms: Option<u64>,
    duration_api_ms: Option<u64>,
    usage: Usage,
}

/// Configuration for the Claude Code subprocess.
#[derive(Debug, Clone)]
pub struct ClaudeCodeConfig {
    pub binary: String,
    pub model: Option<String>,
    pub timeout_secs: u64,
    pub allowed_tools: Vec<String>,
    pub session_file: String,
    /// Fallback system prompt used when subprocess auto-restarts from a background task
    /// that passes an empty system_prompt.
    pub default_system_prompt: String,
    /// Maximum turns before forcing session rotation. 0 = disabled.
    pub session_max_turns: u32,
    /// Maximum average API duration (ms) before forcing rotation. 0 = disabled.
    pub session_max_duration_ms: u64,
    /// Number of recent durations to average for the threshold check.
    pub session_duration_window: usize,
}

/// Manages a long-running Claude Code subprocess with streaming I/O.
#[allow(dead_code)]
pub struct ClaudeCode {
    config: ClaudeCodeConfig,
    session_id: Mutex<Option<String>>,
    heartbeat: Arc<AtomicU64>,
    /// Sender for writing messages to Claude's stdin.
    stdin_tx: Mutex<Option<mpsc::Sender<String>>>,
    /// Notifies when a result event is received.
    result_notify: Arc<Notify>,
    /// The latest turn result (text + metrics).
    turn_result: Arc<Mutex<Option<TurnResult>>>,
    /// Track if process is alive.
    alive: Arc<std::sync::atomic::AtomicBool>,
    child_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The OS child process — killed on stop() to prevent orphans.
    child_process: Mutex<Option<tokio::process::Child>>,
    /// Serializes concurrent send_message() calls to prevent result mixups.
    send_lock: Mutex<()>,
    /// Channel for forwarding notable subprocess events (compression, unknown types).
    event_tx: Mutex<Option<mpsc::UnboundedSender<SubprocessEvent>>>,
    /// Number of completed turns in the current session.
    session_turn_count: AtomicU32,
    /// Ring buffer of recent API durations for moving-average threshold check.
    recent_durations_ms: Mutex<VecDeque<u64>>,
    /// Epoch secs when the currently-executing turn started; 0 = idle.
    busy_since: AtomicU64,
    /// Human-readable label of the current activity (None = idle).
    activity: std::sync::Mutex<Option<String>>,
}

/// RAII guard that clears the busy/activity state when a turn finishes, on every
/// return path (success, timeout, error). Uses a std mutex so `drop` needs no await.
struct BusyGuard<'a> {
    since: &'a AtomicU64,
    label: &'a std::sync::Mutex<Option<String>>,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.since.store(0, Ordering::Relaxed);
        if let Ok(mut l) = self.label.lock() {
            *l = None;
        }
    }
}

/// Notable events from the Claude Code subprocess forwarded to the bot.
#[derive(Debug, Clone)]
pub enum SubprocessEvent {
    /// Context compression detected (raw JSON of the event).
    Compression(String),
    /// Session was rotated due to turn count or duration threshold.
    SessionRotation,
    /// A stderr line matching a suspicious pattern (error/panic/fatal/crashed).
    /// Most stderr lines are routine (debug/info from the CLI) and don't warrant
    /// notification — only forwarded lines reach this variant.
    Stderr(String),
    /// Per-turn usage/cost/latency, persisted to `claude_turns` for the dashboard.
    /// Not surfaced to the owner DM.
    TurnMetrics {
        session_id: Option<String>,
        turn_num: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        duration_api_ms: Option<u64>,
    },
}

/// Heuristic: does this stderr line look worth surfacing to the operator?
/// We're deliberately strict — false positives mean DM spam, false negatives
/// are logged to journald as `warn!` anyway.
fn stderr_line_is_suspicious(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // Anchor on patterns that reliably indicate trouble, not generic words.
    const NEEDLES: &[&str] = &[
        "panic",
        "fatal",
        "crashed",
        "segfault",
        "assertion failed",
        "unhandled exception",
        "stack overflow",
        "out of memory",
        "enospc",
        "authentication failed",
        "401 unauthorized",
        "403 forbidden",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

#[allow(dead_code)]
impl ClaudeCode {
    pub fn new(config: ClaudeCodeConfig) -> Self {
        Self {
            config,
            session_id: Mutex::new(None),
            heartbeat: Arc::new(AtomicU64::new(epoch_now())),
            stdin_tx: Mutex::new(None),
            result_notify: Arc::new(Notify::new()),
            turn_result: Arc::new(Mutex::new(None)),
            alive: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            child_handle: Mutex::new(None),
            child_process: Mutex::new(None),
            send_lock: Mutex::new(()),
            event_tx: Mutex::new(None),
            session_turn_count: AtomicU32::new(0),
            recent_durations_ms: Mutex::new(VecDeque::new()),
            busy_since: AtomicU64::new(0),
            activity: std::sync::Mutex::new(None),
        }
    }

    /// Epoch secs when the current turn started, or 0 when idle.
    pub fn busy_since(&self) -> u64 {
        self.busy_since.load(Ordering::Relaxed)
    }

    /// Human-readable description of what the subprocess is doing right now.
    pub fn activity_label(&self) -> Option<String> {
        self.activity.lock().ok().and_then(|l| l.clone())
    }

    /// Set the event notification channel for forwarding notable subprocess events.
    pub async fn set_event_sender(&self, tx: mpsc::UnboundedSender<SubprocessEvent>) {
        *self.event_tx.lock().await = Some(tx);
    }

    pub fn heartbeat_epoch(&self) -> u64 {
        self.heartbeat.load(Ordering::Relaxed)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Load persisted session ID from file.
    async fn load_session_id(&self) -> Option<String> {
        let path = &self.config.session_file;
        if path.is_empty() {
            return None;
        }
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let id = content.trim().to_string();
                if id.is_empty() { None } else { Some(id) }
            }
            Err(_) => None,
        }
    }

    /// Persist session ID to file.
    async fn save_session_id(&self, session_id: &str) {
        let path = &self.config.session_file;
        if path.is_empty() {
            return;
        }
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Err(e) = tokio::fs::write(path, session_id).await {
            warn!(?e, "failed to persist claude session id");
        }
    }

    /// Internal: send arbitrary content (string or array) to the subprocess.
    async fn send_content(&self, system_prompt: &str, content: serde_json::Value, activity: &str) -> Result<String> {
        // Serialize concurrent callers — subprocess handles one turn at a time.
        // Bounded wait so a wedged in-flight turn can't block every caller forever;
        // budget = 2× turn timeout (one turn ahead of us + our own turn).
        let lock_budget = Duration::from_secs(self.config.timeout_secs.saturating_mul(2).max(30));
        let _send_guard = match timeout(lock_budget, self.send_lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                warn!(
                    budget_secs = lock_budget.as_secs(),
                    "claude send_lock acquisition timed out — killing wedged subprocess"
                );
                self.stop().await;
                bail!(
                    "claude send_lock timed out after {}s — subprocess was wedged, killed for restart",
                    lock_budget.as_secs()
                );
            }
        };

        // Mark busy for the dashboard live-status. Placed after the send_lock so
        // only the actually-executing turn owns the state; the guard clears it
        // before the lock is released, so the next queued caller re-sets cleanly.
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.busy_since.store(started, Ordering::Relaxed);
        if let Ok(mut l) = self.activity.lock() {
            *l = Some(activity.to_string());
        }
        let _busy = BusyGuard {
            since: &self.busy_since,
            label: &self.activity,
        };

        // If not alive, spawn a new process
        if !self.is_alive() {
            let prompt = if system_prompt.trim().is_empty() {
                &self.config.default_system_prompt
            } else {
                system_prompt
            };
            self.start(prompt).await?;
        }

        // Clear previous result
        *self.turn_result.lock().await = None;

        // Build the input message (stream-json v2 format)
        let input = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": content,
            },
            "session_id": "default",
            "parent_tool_use_id": null,
        });
        let msg = serde_json::to_string(&input)?;

        // Send via stdin — scoped to drop MutexGuard before waiting
        {
            let guard = self.stdin_tx.lock().await;
            let sender = guard.as_ref().ok_or_else(|| anyhow!("claude subprocess stdin not available"))?;
            sender.send(msg).await.map_err(|_| anyhow!("claude subprocess stdin channel closed"))?;
        }
        // MutexGuard dropped here — stop() can now safely acquire stdin_tx

        // Wait for result with timeout
        let wait_result = timeout(
            Duration::from_secs(self.config.timeout_secs),
            self.result_notify.notified(),
        )
        .await;

        if wait_result.is_err() {
            warn!("claude response timed out after {}s", self.config.timeout_secs);
            // Kill and restart on next call
            self.stop().await;
            bail!("claude response timed out after {}s", self.config.timeout_secs);
        }

        let turn = self.turn_result.lock().await.take();
        let turn = turn.ok_or_else(|| anyhow!("claude returned empty result"))?;

        // Track metrics and check rotation thresholds
        let turn_num = self.session_turn_count.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            turn = turn_num,
            cost_usd = turn.cost_usd,
            duration_ms = turn.duration_ms,
            duration_api_ms = turn.duration_api_ms,
            input_tokens = turn.usage.input_tokens,
            output_tokens = turn.usage.output_tokens,
            cache_read_tokens = turn.usage.cache_read_input_tokens,
            "claude_turn_complete"
        );

        // Forward per-turn metrics for durable persistence (dashboard).
        if let Some(tx) = self.event_tx.lock().await.clone() {
            let session_id = self.session_id.lock().await.clone();
            let _ = tx.send(SubprocessEvent::TurnMetrics {
                session_id,
                turn_num: turn_num as u64,
                input_tokens: turn.usage.input_tokens,
                output_tokens: turn.usage.output_tokens,
                cache_creation_tokens: turn.usage.cache_creation_input_tokens,
                cache_read_tokens: turn.usage.cache_read_input_tokens,
                cost_usd: turn.cost_usd,
                duration_ms: turn.duration_ms,
                duration_api_ms: turn.duration_api_ms,
            });
        }

        if self.should_rotate_session(turn_num, turn.duration_api_ms).await {
            self.rotate_session().await;
        }

        Ok(turn.text)
    }

    /// Send a text message (user turn) to the running subprocess.
    /// Returns the text response from Claude.
    pub async fn send_message(&self, system_prompt: &str, user_prompt: &str, activity: &str) -> Result<String> {
        self.send_content(system_prompt, serde_json::Value::String(user_prompt.to_string()), activity).await
    }

    /// Send a message with one or more images (base64) to the running subprocess.
    pub async fn send_message_with_images(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        images: &[(&str, &str)], // (base64_data, media_type)
        activity: &str,
    ) -> Result<String> {
        let mut content_blocks: Vec<serde_json::Value> = images.iter().map(|(b64, mime)| {
            serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime,
                    "data": b64
                }
            })
        }).collect();
        content_blocks.push(serde_json::json!({
            "type": "text",
            "text": user_prompt
        }));
        self.send_content(system_prompt, serde_json::Value::Array(content_blocks), activity).await
    }

    /// Start the Claude Code subprocess.
    pub async fn start(&self, system_prompt: &str) -> Result<()> {
        // Stop any existing process
        self.stop().await;

        // Reset session tracking for the new process
        self.session_turn_count.store(0, Ordering::Relaxed);
        self.recent_durations_ms.lock().await.clear();

        let mut cmd = Command::new(&self.config.binary);
        cmd.arg("--print")
            .arg("--input-format").arg("stream-json")
            .arg("--output-format").arg("stream-json")
            .arg("--verbose");

        if let Some(model) = self.config.model.as_ref() {
            if !model.trim().is_empty() {
                cmd.arg("--model").arg(model.trim());
            }
        }

        if !system_prompt.trim().is_empty() {
            cmd.arg("--system-prompt").arg(system_prompt.trim());
        }

        if !self.config.allowed_tools.is_empty() {
            cmd.arg("--allowedTools")
                .arg(self.config.allowed_tools.join(","));
        }

        // Resume session if available
        let session_id = self.load_session_id().await;
        if let Some(ref sid) = session_id {
            cmd.arg("--resume").arg(sid);
            info!(session_id = %sid, "resuming claude session");
        }

        // Prevent nested session detection
        cmd.env_remove("CLAUDECODE");

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("failed to spawn claude subprocess")?;

        let stdin = child.stdin.take().context("failed to get claude stdin")?;
        let stdout = child.stdout.take().context("failed to get claude stdout")?;
        let stderr = child.stderr.take().context("failed to get claude stderr")?;

        // Store the child process so stop() can kill it
        *self.child_process.lock().await = Some(child);

        // Stdin writer channel
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(32);
        *self.stdin_tx.lock().await = Some(stdin_tx);

        self.alive.store(true, Ordering::Relaxed);
        self.heartbeat.store(epoch_now(), Ordering::Relaxed);

        let alive = Arc::clone(&self.alive);
        let heartbeat = Arc::clone(&self.heartbeat);
        let turn_result = Arc::clone(&self.turn_result);
        let result_notify = Arc::clone(&self.result_notify);
        let session_file = self.config.session_file.clone();
        let event_tx = self.event_tx.lock().await.clone();

        // Spawn combined task that manages all three I/O streams
        let handle = tokio::spawn(async move {
            let stdin_task = tokio::spawn(async move {
                let mut writer = stdin;
                while let Some(msg) = stdin_rx.recv().await {
                    let line = format!("{}\n", msg);
                    if let Err(e) = writer.write_all(line.as_bytes()).await {
                        warn!(?e, "failed to write to claude stdin");
                        break;
                    }
                    if let Err(e) = writer.flush().await {
                        warn!(?e, "failed to flush claude stdin");
                        break;
                    }
                }
            });

            let heartbeat_c = Arc::clone(&heartbeat);
            let turn_result_c = Arc::clone(&turn_result);
            let result_notify_c = Arc::clone(&result_notify);

            // Clone for the stderr task before the stdout task moves `event_tx`.
            let stderr_event_tx = event_tx.clone();

            let stdout_task = tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                let mut accumulated_text = String::new();
                let mut session_logged = false; // Only log first "claude session started" per subprocess

                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    heartbeat_c.store(epoch_now(), Ordering::Relaxed);

                    // Two-pass parse: extract raw type to detect compression events
                    if let Ok(raw) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        if let Some(event_type) = raw.get("type").and_then(|t| t.as_str()) {
                            let subtype = raw.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                            let is_compression = event_type.contains("compress")
                                || subtype.contains("compress");

                            if is_compression {
                                info!(event_type, "claude_context_compression");
                                if let Some(ref tx) = event_tx {
                                    let _ = tx.send(SubprocessEvent::Compression(trimmed.to_string()));
                                }
                            }
                            // Known harmless types: don't log or notify
                            // system, assistant, result — handled below
                            // user — input echo, rate_limit_event — API rate info
                        }
                    }

                    let event: StreamEvent = match serde_json::from_str(trimmed) {
                        Ok(e) => e,
                        Err(e) => {
                            debug!(?e, line = trimmed, "unparseable claude stdout line");
                            continue;
                        }
                    };

                    match event {
                        StreamEvent::System { session_id, subtype } => {
                            // Check for compression in system subtype
                            if let Some(ref st) = subtype {
                                if st.contains("compress") {
                                    info!(subtype = st.as_str(), "claude_context_compression");
                                    if let Some(ref tx) = event_tx {
                                        let _ = tx.send(SubprocessEvent::Compression(trimmed.to_string()));
                                    }
                                }
                            }
                            if let Some(sid) = session_id {
                                if !session_logged {
                                    info!(session_id = %sid, "claude session started");
                                    session_logged = true;
                                }
                                // Persist session ID
                                if !session_file.is_empty() {
                                    if let Some(parent) = std::path::Path::new(&session_file).parent() {
                                        let _ = tokio::fs::create_dir_all(parent).await;
                                    }
                                    let _ = tokio::fs::write(&session_file, &sid).await;
                                }
                            }
                        }
                        StreamEvent::Assistant { message, .. } => {
                            if let Some(msg) = message {
                                for block in &msg.content {
                                    if let ContentBlock::Text { text } = block {
                                        accumulated_text.push_str(text);
                                    }
                                }
                            }
                        }
                        StreamEvent::Result { result, session_id, cost_usd, duration_ms, duration_api_ms, usage, .. } => {
                            let final_text = if let Some(r) = result {
                                r
                            } else if !accumulated_text.is_empty() {
                                std::mem::take(&mut accumulated_text)
                            } else {
                                String::new()
                            };

                            if let Some(sid) = session_id {
                                if !session_file.is_empty() {
                                    if let Some(parent) = std::path::Path::new(&session_file).parent() {
                                        let _ = tokio::fs::create_dir_all(parent).await;
                                    }
                                    let _ = tokio::fs::write(&session_file, &sid).await;
                                }
                            }

                            *turn_result_c.lock().await = Some(TurnResult {
                                text: final_text,
                                cost_usd,
                                duration_ms,
                                duration_api_ms,
                                usage: usage.unwrap_or_default(),
                            });
                            result_notify_c.notify_one();
                            accumulated_text.clear();
                        }
                        StreamEvent::Unknown => {}
                    }
                }
            });

            let stderr_task = tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    warn!(claude_stderr = trimmed);
                    if stderr_line_is_suspicious(trimmed) {
                        if let Some(ref tx) = stderr_event_tx {
                            let _ = tx.send(SubprocessEvent::Stderr(trimmed.to_string()));
                        }
                    }
                }
            });

            // Wait for stdout to finish (process exited)
            let _ = stdout_task.await;
            stdin_task.abort();
            stderr_task.abort();
            alive.store(false, Ordering::Relaxed);
            info!("claude subprocess exited");
        });

        *self.child_handle.lock().await = Some(handle);
        *self.session_id.lock().await = session_id;

        // Give the process a moment to start
        tokio::time::sleep(Duration::from_millis(200)).await;

        if !self.is_alive() {
            bail!("claude subprocess exited immediately after spawn");
        }

        info!("claude subprocess started");
        Ok(())
    }

    /// Stop the subprocess.
    pub async fn stop(&self) {
        self.alive.store(false, Ordering::Relaxed);
        *self.stdin_tx.lock().await = None;

        // Kill the OS process to prevent orphans
        if let Some(mut child) = self.child_process.lock().await.take() {
            let _ = child.kill().await;
        }

        if let Some(handle) = self.child_handle.lock().await.take() {
            handle.abort();
        }
    }

    /// Restart the subprocess with a new system prompt.
    pub async fn restart(&self, system_prompt: &str) -> Result<()> {
        self.stop().await;
        // Small delay before restart
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start(system_prompt).await
    }

    /// Number of completed turns in the current session.
    pub fn session_turn_count(&self) -> u32 {
        self.session_turn_count.load(Ordering::Relaxed)
    }

    /// Check if the session should be rotated based on turn count or API duration.
    async fn should_rotate_session(&self, turn_count: u32, api_duration_ms: Option<u64>) -> bool {
        let max_turns = self.config.session_max_turns;
        if max_turns > 0 && turn_count >= max_turns {
            info!(turn_count, max_turns, "session_rotation: turn limit reached");
            return true;
        }

        let max_dur = self.config.session_max_duration_ms;
        if max_dur > 0 {
            if let Some(dur) = api_duration_ms {
                let mut durations = self.recent_durations_ms.lock().await;
                durations.push_back(dur);
                let window = self.config.session_duration_window.max(1);
                while durations.len() > window {
                    durations.pop_front();
                }
                if durations.len() >= window {
                    let avg: u64 = durations.iter().sum::<u64>() / durations.len() as u64;
                    if avg > max_dur {
                        info!(avg_duration_ms = avg, max_duration_ms = max_dur, window, "session_rotation: duration threshold exceeded");
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Clear session file, reset counters, and stop the subprocess.
    /// The next send_content() call will start a fresh session.
    async fn rotate_session(&self) {
        info!("session_rotation: rotating to fresh session");

        // Clear the session file so start() won't use --resume
        let path = &self.config.session_file;
        if !path.is_empty() {
            let _ = tokio::fs::remove_file(path).await;
        }

        // Reset counters
        self.session_turn_count.store(0, Ordering::Relaxed);
        self.recent_durations_ms.lock().await.clear();

        // Stop the subprocess — next send_content() will start fresh
        self.stop().await;

        // Notify via event channel
        if let Some(ref tx) = *self.event_tx.lock().await {
            let _ = tx.send(SubprocessEvent::SessionRotation);
        }
    }
}

fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn result_event_parses_usage_and_cost_alias() {
        let j = r#"{"type":"result","subtype":"success","result":"hi","session_id":"s1",
                    "total_cost_usd":0.01,"duration_ms":100,"duration_api_ms":90,
                    "usage":{"input_tokens":1234,"output_tokens":56,
                             "cache_creation_input_tokens":10,"cache_read_input_tokens":789}}"#;
        let ev: StreamEvent = serde_json::from_str(j).unwrap();
        match ev {
            StreamEvent::Result { usage, cost_usd, .. } => {
                let u = usage.expect("usage present");
                assert_eq!(u.input_tokens, 1234);
                assert_eq!(u.output_tokens, 56);
                assert_eq!(u.cache_creation_input_tokens, 10);
                assert_eq!(u.cache_read_input_tokens, 789);
                assert_eq!(cost_usd, Some(0.01));
            }
            _ => panic!("expected Result variant"),
        }
    }

    #[test]
    fn result_event_without_usage_is_fine() {
        let j = r#"{"type":"result","result":"ok"}"#;
        let ev: StreamEvent = serde_json::from_str(j).unwrap();
        match ev {
            StreamEvent::Result { usage, .. } => assert!(usage.is_none()),
            _ => panic!("expected Result variant"),
        }
    }
}
