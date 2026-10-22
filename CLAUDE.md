# Telegram Codex Jarvis

Rust Telegram bot that ingests messages, stores them in SQLite, and answers queries via Claude Code CLI streaming subprocess.

## Architecture

```
lib.rs           — library crate, re-exports shared modules (claude_code, config, db, focus, mcp, memory, metrics, rag, reminders, scheduled_tasks, security, self_update)
main.rs          — entrypoint, wires config/db/rag/claude/focus/security/reminders/scheduled_tasks, heartbeat task, owner startup notification, `--version` (prints build sha, no .env needed), self-update loop (notices → owner DM, restart flag → clean exit once Claude is idle)
self_update.rs   — self-edit plumbing: data-dir paths (notices/, restart_requested, deploy_pending, self_improve/<job>/), detached job spawner with scrubbed env, `GIT_SHA` baked in by build.rs
claude_code.rs   — streaming Claude Code subprocess (stream-json I/O, session persistence, heartbeat, deadlock-safe, serialized sends, child process cleanup, image support)
bot.rs           — teloxide bot dispatcher: message ingestion, trigger logic, security/focus gating, tone detection, codex answering, media extraction, /health endpoint, structured logging
dashboard.rs     — ops dashboard: axum routes (/dash + /api/*) gated by X-Dash-Auth header, served on the bot's existing webhook server (binary module). `/api/metrics` also reports the active model (`config.claude_code_model`) and live activity (`claude.busy`/`busy_for_secs`/`activity`, sourced from `ClaudeCode::busy_since`/`activity_label`) → dashboard "Right now" panel + header model tag
dashboard.html   — self-contained dashboard SPA (inline CSS/JS, SVG charts, 3s polling)
metrics.rs       — BotMetrics: atomic counters for messages received/responded/dropped, claude calls/errors, timestamps, latency histogram (p50/p90/p99)
config.rs        — Config + McpConfig structs, all env var parsing (from .env via dotenvy)
db.rs            — SQLite via sqlx: 16 tables, upserts, queries, WAL mode, generic SELECT executor
rag.rs           — lexical RAG: token-based scoring over recent messages (no embeddings)
focus.rs         — single-chat focus mode: FocusManager with Process/Queue/Bypass decisions
memory.rs        — persistent memory CRUD: MemoryManager with DB storage, path-traversal protection
reminders.rs     — time-based reminders: ReminderService with 30s background checker
scheduled_tasks.rs — recurring cron jobs: ScheduledTaskService with background checker, uses shared Arc<ClaudeCode> subprocess
security.rs      — SecurityGate: spam filtering, rate limiting, strike/ban system, keyword blocking
proactive.rs     — proactive monitoring: evaluate_proactive decides whether/how Jarvis joins group conversations
chat_profiles.rs — per-chat style profiles: Claude-analyzed tone/personality stored in chat_profiles table
faceit/mod.rs    — FACEIT CS2 API client: player profiles, match history, match details (HTTP API only)
google_sheets.rs — Google Sheets API client: service account JWT auth, CRUD operations on spreadsheets
google_calendar.rs — Google Calendar API client: same service-account key as sheets, list/create/delete events
gmail.rs         — Gmail IMAP client: inbox/sent search and read (app-password auth)
smtp.rs          — SMTP outbound sender: plain-text body + optional file attachment (lettre)
formatting.rs    — shared text/markdown→Telegram-HTML formatting helpers
mcp.rs           — MCP protocol handler: JSON-RPC over stdio, ~87 tools (includes web_fetch, web_search, download_file, google sheets/calendar)
mcp_tools/       — one module per MCP tool, registered into the dispatcher
```

### Bin utilities
- `src/bin/mcp_server.rs` — MCP server for Claude Code (stdio JSON-RPC, exposes Telegram + memory + focus + reminder + scheduled task + admin tools)
- `src/bin/guard.rs` — `jarvis-guard`: runs the bot as its child; after a self-deploy, enforces a health probation and rolls the binaries back on failure (see Self-Edit)

### MCP Server (`mcp-telegram`)
Standalone binary that speaks MCP JSON-RPC over stdio. Claude Code launches it as subprocess.
- Shares SQLite database for search/read operations
- Has its own MemoryManager and FocusManager instances
- **~87 tools** across ~19 categories (see below)
- Config: `~/.mcp.json` (global) or `.mcp.json` (project)
- Build: `cargo build --release --bin mcp-telegram`

### MCP Tools (~87)

**Messaging** (2 tools):
- `search_messages` — keyword search over DB
- `send_reaction` — set emoji reaction on any message (via Bot API `setMessageReaction`)

**Bot API — Telegram** (27 tools):
- `bot_send_message` — send text message via Bot API (HTML/markdown/plain)
- `bot_edit_message` — edit a previously sent bot message
- `send_chat_action` — send typing/uploading indicator
- `send_photo` — send photo (URL, file_id, or local path, with optional caption)
- `send_video` — send video (URL, file_id, or local path, with optional caption)
- `send_audio` — send audio/music file (URL, file_id, or local path, with title/performer)
- `send_document` — send document/file (URL, file_id, or local path, with optional caption)
- `send_sticker` — send sticker (file_id, URL, or local path)
- `send_location` — send a map point (latitude/longitude)
- `send_contact` — send a phone contact card
- `pin_message` — pin a message in a chat
- `unpin_message` — unpin a message or all messages
- `delete_message` — delete a message
- `forward_message` — forward a message to another chat (with sender info)
- `copy_message` — copy a message to another chat (without "Forwarded from")
- `set_chat_title` — change chat title (admin required)
- `set_chat_description` — change chat description (admin required)
- `set_chat_photo` — set chat profile photo from URL (admin required)
- `delete_chat_photo` — delete chat profile photo (admin required)
- `create_chat_invite_link` — create invite link with optional expiry/limit
- `get_chat_member_count` — get number of members in a chat
- `send_poll` — create a poll (anonymous/named, single/multi answer)
- `ban_chat_member` / `unban_chat_member` — ban/unban users (admin required)
- `approve_chat_join_request` / `decline_chat_join_request` — handle join requests
- `leave_chat` — make bot leave a group/channel

**Database** (1 tool — new):
- `query` — execute read-only SELECT queries (max 100 rows, server-enforced keyword blocking)

**Memory** (6 tools — new):
- `create_memory` — create a new persistent memory at a path
- `read_memory` — read memory by path (tracks read state for edit protection)
- `edit_memory` — edit memory (requires prior read in same session)
- `list_memories` — list all stored memory paths with timestamps
- `search_memories` — search memories by keyword in path or content
- `delete_memory` — delete a memory entry

**Reminders** (3 tools):
- `set_reminder` — set a timed reminder (fires at Unix timestamp)
- `list_reminders` — list all pending reminders
- `cancel_reminder` — cancel a pending reminder by ID

**Scheduled Tasks** (4 tools):
- `create_scheduled_task` — create a recurring cron job (prompt executed via Claude oneshot at each scheduled time)
- `list_scheduled_tasks` — list all scheduled tasks with status, cron, next run
- `update_scheduled_task` — update task fields (name, prompt, cron, timezone, enabled)
- `delete_scheduled_task` — permanently delete a scheduled task

**Focus** (4 tools — new):
- `switch_focus` — switch bot's focus to a specific chat (or clear)
- `get_focus_state` — get current focus state
- `mute_chat` — mute a chat (optional auto-unmute timestamp)
- `unmute_chat` — unmute a previously muted chat

**Admin** (3 tools — new):
- `now` — get current Unix timestamp
- `get_heartbeats` — get recent heartbeat records
- `get_tool_stats` — per-tool call count, error count, and cumulative/mean duration for every tool invoked since the MCP server started (sorted by call count desc)

**Self-edit** (4 tools — `self_improve`/`self_deploy` require `SELF_IMPROVE_ENABLED=true`):
- `self_improve` — start a background job: sandboxed coding agent changes Jarvis on a fresh `jarvis/<job-id>` branch → gates → push → install → restart (returns job id at once)
- `self_deploy` — build + deploy an existing GitHub branch (`main` after a merge, or a `jarvis/*` branch to roll back/forward), same gates
- `self_improve_status` — one job's state/task/summary/log tail, or the running version + 10 most recent jobs
- `restart_self` — restart onto the installed binaries after the current turn (stuck state, prompt reload)

**Monitored groups** (3 tools):
- `add_monitored_group` — add a group (optionally a specific forum topic) to proactive monitoring
- `remove_monitored_group` — remove a group from proactive monitoring
- `list_monitored_groups` — list all groups/topics currently under proactive monitoring

**FACEIT** (3 tools — HTTP API only):
- `faceit_profile` — look up FACEIT CS2 player by nickname (ELO, level, lifetime K/D, win rate, ADR)
- `faceit_matches` — recent CS2 match history for a player (W/L, scores, durations)
- `faceit_match` — detailed per-player scoreboard for a specific match (K/D/A, HS%)

**Gmail** (3 tools):
- `gmail_search` — search Gmail INBOX; plain queries match subject/body, prefix with `gmail:` for native search syntax (from:, subject:, newer_than:, has:attachment). Read-only.
- `gmail_list_recent` — list the most recent inbox messages as summaries (uid, subject, from, date). Read-only.
- `gmail_read_message` — fetch the full body of a message by UID (subject, from, date, plain-text body; HTML stripped). Read-only.

**PDF** (1 tool):
- `extract_pdf_text` — extract text from a local text-based PDF (no OCR)

**Email** (1 tool):
- `send_email_application` — generic email sender: plain-text body + optional file attachment via the configured SMTP account (defaults to the same Gmail app-password used by IMAP)

**Web** (5 tools):
- `web_fetch` — fetch URL contents via reqwest (max 100KB, 30s timeout)
- `web_search` — backend chain: Brave (BRAVE_SEARCH_API_KEY) → Tavily (TAVILY_API_KEY) → DuckDuckGo scrape fallback (DDG blocks datacenter IPs — a key is effectively required on the VPS). Claude Code's built-in WebSearch/WebFetch are also in the default allowlist.
- `download_file` — download file from URL to local storage (max 50MB, 2min timeout), returns local path for use with send_* tools
- `rss_read` — fetch + parse an RSS/Atom feed, latest items (stable news source, no bot-detection)
- `youtube_transcript` — caption transcript of a YouTube video as plain text (no API key)

**Info** (2 tools):
- `weather` — current + daily forecast via open-meteo (no API key)
- `currency_rates` — fiat rates vs UZS (official CBU.uz) + optional crypto USD prices (CoinGecko), no API key

**Audio** (1 tool):
- `transcribe_voice` — transcribe a Telegram voice message (file_id) or local audio file via Whisper (GROQ_API_KEY preferred, OPENAI_API_KEY fallback)

**Google Calendar** (3 tools — reuses GOOGLE_SERVICE_ACCOUNT_KEY; share the calendar with the service account email, set GOOGLE_CALENDAR_ID):
- `calendar_list_events` — events in a time window
- `calendar_create_event` — create event (summary, RFC3339 start/end, description/location)
- `calendar_delete_event` — delete event by id

**Google Sheets** (10 tools):
- `sheets_read` — read a range of cells from a spreadsheet
- `sheets_write` — write values to a range (overwrites existing data)
- `sheets_append` — append rows after the last row of data in a range
- `sheets_clear` — clear all values from a range (preserves formatting)
- `sheets_info` — get spreadsheet metadata (title, sheet names, row/column counts)
- `sheets_create_sheet` — add a new sheet/tab to an existing spreadsheet
- `sheets_delete_sheet` — delete a sheet/tab from a spreadsheet by sheet ID
- `sheets_create_spreadsheet` — create a brand new spreadsheet
- `sheets_batch_read` — read multiple ranges in one API call
- `sheets_batch_write` — write to multiple ranges in one API call

### Database Tables (16)

**Core** (3 tables — original, untouched):
- `chats` — chat metadata (id, kind, title, username, timestamps)
- `users` — user metadata (id, username, name, is_bot, timestamps)
- `messages` — message storage (chat_id, telegram_message_id, content, raw_json, timestamps, reply_to, link)

**Focus** (2 tables — new):
- `focus_state` — singleton: which chat is currently focused
- `focus_chats` — per-chat cursor and debounce tracking

**Memory** (1 table — new):
- `memories` — path-based persistent storage (path, content, timestamps)

**Reminders** (1 table):
- `reminders` — scheduled reminders (chat_id, user_id, message, remind_at, fired, cancelled)

**Scheduled Tasks** (1 table):
- `scheduled_tasks` — recurring cron jobs (chat_id, user_id, name, prompt, cron_expr, timezone, next_run_at, last_run_at, enabled, running)

**Security** (2 tables):
- `strikes` — per-user strike records (user_id, chat_id, reason, timestamp)
- `dm_rate_limits` — per-user DM rate limiting (message_count, window_start)

**Config** (2 tables — new):
- `chat_aliases` — alias → chat_id mapping
- `muted_chats` — muted chat tracking with optional auto-unmute

**Proactive** (2 tables):
- `monitored_groups` — groups/topics under proactive monitoring (chat_id, thread_id, added_at)
- `proactive_cooldowns` — per-chat last response/reaction timestamps for proactive rate limiting

**Profiles** (1 table):
- `chat_profiles` — per-chat style profile (profile_json, prompt_instruction, message_count_at_analysis, analyzed_at)

**Monitoring** (1 table — new):
- `heartbeats` — timestamped heartbeat records (source, details)

**Dashboard** (3 tables — new):
- `claude_turns` — per-turn tokens (input/output/cache), duration (written from `SubprocessEvent::TurnMetrics`); dashboard cost is computed from tokens at the configured model's official rates (`model_rates` in db.rs; 1h cache-write rate; unknown model → labelled Opus 5.5 fallback) in `dash_turns` (the CLI's `cost_usd` is ignored)
- `claude_events` — context compaction / session rotation events
- `tool_calls` — per-call MCP tool name/success/duration/error (written by `mcp-telegram`)

## Key Dependencies

- **teloxide 0.13** — Telegram Bot API (webhook via axum, long-polling fallback)
- **axum 0.7** — HTTP framework (used for `/health` endpoint alongside webhook router)
- **sqlx 0.8** (sqlite) — async SQLite with runtime-tokio-rustls, WAL mode
- **tokio 1** — async runtime
- **serde/serde_json** — serialization
- **anyhow** — error handling
- **reqwest 0.11** — HTTP client for FACEIT API and web tools (rustls TLS backend)
- **base64 0.22** — base64 encoding for image data
- **cron 0.15** — cron expression parsing (7-field format with chrono iterators)
- **chrono 0.4 + chrono-tz 0.10** — timezone-aware datetime handling for scheduled tasks
- **jsonwebtoken 9** — JWT signing for Google service account OAuth2 (`sheets` feature)
- **imap 2 + native-tls 0.2** — Gmail IMAP inbox/sent access (`gmail` feature)
- **pdf-extract 0.10** — text extraction from text-based PDFs (`pdf` feature)
- **lettre 0.11** — SMTP outbound email with attachments (`smtp` feature)
- **feed-rs 2** — RSS/Atom feed parsing (`rss` feature)

### Cargo Features

`default = ["faceit", "sheets", "gmail", "pdf", "smtp", "rss", "calendar"]` — the production build enables every optional integration. Each is a feature flag so lean builds can opt out:
- `faceit` = `[]` — HTTP-only FACEIT API client; no extra deps
- `sheets` = `jsonwebtoken`
- `gmail` = `imap` + `native-tls`
- `pdf` = `pdf-extract`
- `smtp` = `lettre`
- `rss` = `feed-rs`
- `calendar` = `jsonwebtoken`

## Data Flow

1. Every incoming message (bot API) is stored in SQLite (`messages` table)
   - **Logged**: `msg_received` (with chat_id, message_id) or `msg_skipped_stale` for pre-restart messages
2. **Media extraction**: photos, stickers, GIFs, videos, voice messages, documents are processed
   - Photos/stickers → downloaded and base64-encoded for Claude vision
   - Voice/video-note messages → stored as metadata only, e.g. `[Ovozli xabar: Ns]` (no transcription)
   - Video/animation → thumbnail extracted as image
   - Documents → image types as base64, text types inline, others as metadata
3. Messages are indexed for lexical RAG search (media-only messages get synthetic text descriptions)
4. **Security gate** checks message: spam patterns → rate limits → strike/ban → system query blocking
   - **Logged**: `security_blocked_system_query`, `security_rate_limited`, `security_spam_filtered`, `security_banned`
5. **Focus gate** checks: Process (in focused chat or no focus) | Queue (different chat focused) | Bypass (owner/allowed user)
   - **Logged**: `focus_gated` when message is queued due to focus
6. When bot should respond (DM allowed user, group trigger):
   - **Logged**: `trigger_matched`, `msg_queued`, `batch_start` (with batch_size)
   - RAG retrieves relevant history
   - Recent dialog window (last 4 messages) is fetched
   - **Chat tone detected** (formal/casual/mixed) from recent messages
   - User prompt built: chat ids + incoming batch + recent dialog + RAG history + chat guidance (chat profile style or detected tone, owner/requester note) + user rules (`prompts/user_rules.txt`). The system prompt is static (`prompts/system_prompt.txt` only, set once at spawn) so the prompt-cache prefix never changes between turns, chats, or restarts
   - If image attached: Claude Code receives base64 image via `send_message_with_image()`
   - Otherwise: text-only via `send_message()`
   - **Logged**: `claude_done` (with elapsed_ms) or `claude_error`
   - Response is sent back as Telegram HTML (with `**bold**` conversion)

## Claude Code Subprocess

**Replaced one-shot `claude -p` with long-running streaming subprocess.**

- Spawns `claude --print --input-format stream-json --output-format stream-json --verbose --model <model> --system-prompt "<prompts/system_prompt.txt>" --strict-mcp-config --mcp-config <CLAUDE_CODE_MCP_CONFIG> --tools <non-mcp allowlist entries> --allowedTools <allowlist> [--resume SESSION_ID]`
- System prompt comes only from `ClaudeCodeConfig::system_prompt`; callers pass just the user turn. Per-chat content must go in the user message — anything else changes the cached prefix
- `--strict-mcp-config` keeps the account's claude.ai connectors (Gmail/Slack/Notion/Linear/...) out of the subprocess; `--tools` drops unused built-ins and ToolSearch, so the telegram MCP tools are inlined instead of deferred
- Three async tasks: stdin writer (channel-based), stdout JSON parser (event stream), stderr reader
- `Arc<AtomicU64>` heartbeat updated on every stdout event
- Session ID persisted to file (`data/claude_session_id`) for `--resume` across restarts
- Automatically parsed event types: `system` (session start), `assistant` (text blocks), `result` (final response)
- **Event forwarding**: two-pass stdout parsing detects unknown/compression events, forwards via `SubprocessEvent` channel to owner DM
- On timeout: kills process, next call spawns fresh
- Methods: `start()`, `send_message()`, `stop()`, `restart()`, `is_alive()`, `heartbeat_epoch()`, `set_event_sender()`
- All Claude calls (scheduled tasks, chat profiles, proactive decisions, summaries) route through the single subprocess — no separate oneshot processes
- `CLAUDECODE` env var stripped to prevent nested session detection
- **Auto-compaction**: `CONTEXT_WINDOW_TOKENS` (default 200000) is passed as `CLAUDE_CODE_AUTO_COMPACT_WINDOW` (clamped to the CLI's 100k–1M range; 0 = CLI default), so the session compacts itself instead of rotating. `compact_boundary` events are logged (`claude_context_compression`) and DMed to the owner with pre/post token counts

### Concurrency & Recovery (3 fixes applied)

1. **Deadlock prevention**: `send_message()` scopes the `stdin_tx` MutexGuard in a block so it's dropped before the timeout wait. Previously, variable shadowing (`let tx = tx.as_ref()`) kept the guard alive, and on timeout `stop()` tried to re-acquire the same lock → deadlock that killed the entire bot for 50+ minutes.
2. **Child process cleanup**: `stop()` now kills the OS process via `child_process: Mutex<Option<Child>>` field. Previously only the tokio I/O task was aborted, leaving orphan `claude` processes.
3. **Serialized send_message**: A `send_lock: Mutex<()>` ensures only one `send_message()` executes at a time. Multiple chat batcher tasks share the `Arc<ClaudeCode>`, and without serialization they'd race on `result_notify`/`result_text`, causing result mixups or one task's timeout killing another's in-flight request.

## Bot Response Triggers

- **Owner-only mode (top-level gate)**: `classify_message` in `bot.rs` drops every message whose sender is not an owner (`OWNER_USER_ID` + `OWNER_IDS`, see `Config::is_owner`) before any trigger/proactive/monitored path runs. Non-owner messages are still stored (ingest stage) for history/RAG but never produce a response. This is the single enforcement point for **all** gating below AND for owner-only tools (browser control) — only owner turns ever reach the shared Claude subprocess. The gate fails closed: if no owner is configured (`OWNER_USER_ID`, `OWNER_IDS`, or the `ALLOWED_USERS` fallback), nobody is owner — DMs are dropped, group senders only get guest handling — and startup logs `owner_user_id_unset`. The trigger rules below only apply *after* a message passes the owner gate.
- **Guest replies (exception to the owner gate)**: when `GUEST_REPLIES_ENABLED=true` (default), human non-owners in groups/supergroups pass the gate but are routed to `AppState.guest_claude` — a separate `ClaudeCode` subprocess (own session file, `prompts/guest_system_prompt.txt`, generated `data/claude_guest_mcp.json` with only the telegram server and `MCP_TOOL_ALLOWLIST`). Restricted at three layers: `--tools` builtins, `--allowedTools` = `GUEST_ALLOWED_TOOLS`, and the MCP server's `MCP_TOOL_ALLOWLIST` (filters `tools/list`, rejects other calls). Guests get standard triggers + warm follow-up only (no cold trigger), no RAG, default rules; the bot sends the plain-text reply itself. Fails closed: missing prompt/config → `guest_claude = None` → guests dropped; a guest batch never falls back to the owner subprocess. Guest DMs stay dropped.
- **Private chat**: responds to the owner (owner gate already applied).
- **Groups — standard triggers**: responds when mentioned (`@bot_username`), replied to, or message contains an alias from `BOT_ALIASES` (case-insensitive substring).
- **Groups — active follow-up**: if Jarvis spoke within `ACTIVE_FOLLOWUP_SECS` (default 180s), any follow-up message is evaluated by `proactive::evaluate_proactive` (no cold-trigger cooldown — direct conversation).
- **Groups — LLM cold trigger**: when `PROACTIVE_COLD_TRIGGER_ENABLED=true` (default), every other group message is also evaluated by Claude. Directives like "javob qaytar" / "javob ber" / "jim bo'lma" trigger a RESPOND. Rate-limited by `PROACTIVE_COOLDOWN_SECS` (default 300s, global per chat) and a 10s per-chat evaluation debouncer. Both warm and cold paths share `try_proactive_trigger` in `bot.rs`.

## Jarvis Personality

- Adaptive tone: `detect_chat_tone()` analyzes last 10 messages for formal/casual signals
  - **Formal indicators**: hurmatli, assalomu, iltimos, marhamat, rahmat, sizga, sizning
  - **Casual indicators**: bro, aka, uka, haha, lol, xd, ))), emoji density
- **Formal groups**: professional, structured, "siz" form
- **Casual groups**: friendly, warm, humor when appropriate
- **Owner DMs**: direct, results-oriented, "Boss" address (natural, not forced)
- Clear recommendations, no hedging — decisive assistant style
- Privacy rules strictly enforced (see system prompt)

## Security Layer

`SecurityGate` in `src/security.rs`:
- **Spam filtering**: configurable substring patterns (t.me/joinchat, bit.ly, "free crypto", etc.)
- **DM rate limiting**: 20 messages per hour for non-owner, non-allowed users
- **Strike system**: spam triggers a strike; 3 strikes → auto-ban (persisted in `strikes` table)
- **System query blocking**: configurable keyword list replaces old hardcoded `is_system_or_arch_query()`
- **Owner bypass**: owner always passes all checks

## Focus Mode

`FocusManager` in `src/focus.rs`:
- Single-chat attention: bot concentrates on one conversation
- `should_process(chat_id, user_id)` → `Process | Queue | Bypass`
- Owner and allowed users always bypass focus restrictions
- Controlled via MCP tools: `switch_focus`, `get_focus_state`
- State persisted in `focus_state` DB table

## Memory System

`MemoryManager` in `src/memory.rs`:
- Persistent memories stored in `memories` DB table with path-based organization
- Path traversal protection: rejects `..`, absolute paths, null bytes
- Edit requires prior read in same session (tracked via `HashSet<String>`)
- CRUD: `create`, `read`, `edit`, `list`, `search`, `delete`
- Exposed via MCP tools

## Reminders

`ReminderService` in `src/reminders.rs`:
- Background checker runs every 30 seconds
- When reminder fires: sends message to target chat via bot API
- CRUD via MCP tools: `set_reminder`, `list_reminders`, `cancel_reminder`
- State in `reminders` DB table (tracks fired/cancelled status)

## Scheduled Tasks (Cron Jobs)

`ScheduledTaskService` in `src/scheduled_tasks.rs`:
- Background checker runs every 30 seconds, queries `scheduled_tasks` where `next_run_at <= now`
- When a task fires: sends the task prompt through the shared long-running Claude subprocess (`Arc<ClaudeCode>`)
- Claude response is sent to the target chat via bot API
- After execution, `next_run_at` is recomputed from the cron expression (timezone-aware via `chrono-tz`)
- Tasks execute sequentially (subprocess serializes calls internally via `send_lock`)
- `running` flag prevents double-fire if a task takes longer than the check interval
- Stale `running` flags are reset on service startup (crash recovery)
- Cron expressions: standard 5-field format (minute hour day month weekday), auto-converted to 7-field for the `cron` crate
- CRUD via MCP tools: `create_scheduled_task`, `list_scheduled_tasks`, `update_scheduled_task`, `delete_scheduled_task`
- State in `scheduled_tasks` DB table

## Health Monitoring & Observability

### Metrics (`src/metrics.rs`)
`BotMetrics` — 7 `AtomicU64` counters shared across all tasks:
- `messages_received`, `messages_responded`, `messages_dropped_stale`
- `claude_calls`, `claude_errors`
- `last_message_at`, `last_response_at` (epoch secs)

### Structured Logging (`src/bot.rs`)
All events use short greppable names for `journalctl` filtering:
- `msg_received`, `msg_skipped_stale`, `trigger_matched`, `msg_queued`
- `security_*` (blocked_system_query, rate_limited, spam_filtered, banned)
- `focus_gated`, `batch_start`, `claude_done` (with elapsed_ms), `claude_error`
- `webhook_info`, `webhook_last_error`

Example: `journalctl -u jarvis-bot | grep msg_received`

### `/health` HTTP Endpoint (`src/bot.rs`)
Uses `webhooks::axum_to_router()` to merge a `/health` GET route into the webhook axum server.
Returns JSON: `status` (ok/degraded), `uptime_secs`, `claude` (alive, last_heartbeat_secs_ago), `counters`.
Available at `http://localhost:8083/health` in webhook mode.

### Periodic Heartbeats (`src/main.rs`)
Background task records heartbeat to `heartbeats` DB table every 60s.
Details include `claude_alive` and all metric counters as JSON.
Reuses existing `db.record_heartbeat()` (auto-prunes to 1000 records).
MCP tool `get_heartbeats` now returns real data.

### Owner Startup Notification (`src/main.rs`)
On startup, sends Telegram message to owner with:
- Restart type: "Clean restart" (last heartbeat was `clean_shutdown`) or "Crash restart" (with age)
- Mode (webhook/long-polling)

On clean shutdown (after `run_bot()` returns), records a `clean_shutdown` heartbeat.

### Claude Subprocess Event Forwarding (`src/claude_code.rs` + `src/main.rs`)
Two-pass stdout parsing detects context compaction events:
- `subtype: compact_boundary`, or any `type`/`subtype` containing "compress" → `SubprocessEvent::Compression` → owner DM (`compaction_summary`: trigger, pre/post tokens, duration)
- Known harmless event types silently ignored: `user` (input echo), `rate_limit_event` (API rate info)
- Events forwarded via `mpsc::UnboundedSender<SubprocessEvent>` channel
- `run_claude_event_receiver()` in main.rs sends detected events to owner's Telegram DM

### Webhook Verification (`src/bot.rs`)
After webhook setup, calls `bot.get_webhook_info()` and logs:
- Webhook URL and pending update count (`webhook_info`)
- Last error message if any (`webhook_last_error`)
- Warning if URL is not set (`webhook_url_not_set`)

## Self-Edit & Self-Restart

Jarvis can change its own code, ship it, and restart into it — fully autonomous, guarded by gates + automatic rollback. The main Claude subprocess still has no Bash/Edit; all code work happens in a separate, sandboxed job.

**Flow** (`self_improve` MCP tool → `bin/self-improve.sh improve <job-id> <task>`, detached process group, log in `data/self_improve/<job>/log`):
1. Preflight: one job at a time (`flock`), refuses while a previous deploy is in probation, needs `bwrap` + `socat` and a working bwrap smoke run
2. Git: `fetch` the GitHub clone at `SELF_IMPROVE_REPO` (default `~/jarvis`); worktree `SELF_IMPROVE_WORKTREE` (default `~/jarvis-self`) checked out at the running sha (`bin/VERSION`, fallback `origin/main`) on branch `jarvis/<job-id>`
3. Agent: `claude -p --permission-mode dontAsk` with a generated `settings.json` — Claude Code sandbox on (`failIfUnavailable`, no unsandboxed escape), Read/Edit denied on secrets (`/opt/jarvis`, `.env`, `~/.ssh`, `~/.claude`, …), Edit/Write denied on protected paths, network limited to crates.io, no hooks, empty strict MCP config, `--max-turns`/`--max-budget-usd`/`timeout` caps
4. Checks: worktree `.git` untouched; no changes → `noop`; any change under `src/bin/guard.rs`, `build.rs`, `deploy/`, `.github/`, `.claude/`, `.mcp.json`, `.cargo/`, `rust-toolchain`, `.gitmodules` → fail; commit as `Jarvis <jarvis@users.noreply.github.com>`
5. Gates (inside `bwrap`: read-only `/`, tmpfs `$HOME` + prod dir, only repo/worktree/target/cargo bound): `cargo build --release --locked`, `cargo test --release --locked`, `--version` == commit sha, `mcp-telegram` smoke test (initialize + tools/list must include the self-edit tools and `bot_send_message`)
6. Push `jarvis/<job-id>` (deploy key; the owner merges to `main`)
7. Install: `*.new` → `*.prev` backups → `data/deploy_pending` JSON (sha, branch, job, files) → atomic `mv` → `data/restart_requested`

`self_deploy <branch>` runs the same script in `deploy` mode: steps 5 and 7 on `origin/<branch>`, no agent, no push.

**Restart**: `run_self_update_loop` (main.rs, every 3s) DMs queued `data/notices/*.txt` to the owner; when `restart_requested` exists and Claude has been idle for two checks it stops the subprocess, records `clean_shutdown`, exits 0 → systemd restarts.

**Guard** (`src/bin/guard.rs`, installed root-owned at `/usr/local/bin/jarvis-guard`, out of the agent's reach): systemd runs `jarvis-guard --health-url http://127.0.0.1:8081/health --data-dir /opt/jarvis/data -- /opt/jarvis/bin/telegram-codex-jarvis`. With no `deploy_pending` it just runs the bot and exits with its code. With one, the child must stay alive for `--probation-secs` (45) and `/health` must answer 200 at least once; otherwise it kills the child, renames every `.prev` back (removes files that had none), notices the owner, marks the job `rolled back: …`, exits 1 → systemd starts the old binaries. Long-polling mode has no `/health` → drop `--health-url` (liveness only).

**Known limits**: a build that crashes *after* probation crash-loops until `StartLimitBurst` (no rollback); a running job dies if the service restarts mid-job (`KillMode=control-group`); `origin/main` must already contain the self-edit code.

## Environment (.env)

See `.env.example` for all vars. Key ones:
- `TELOXIDE_TOKEN` — bot token
- `CLAUDE_CODE_BIN` — path to claude CLI binary (default: `claude`; legacy: `CODEX_CLI_BIN`)
- `CLAUDE_CODE_MODEL` — model for claude CLI (legacy: `CODEX_MODEL`)
- `CLAUDE_CODE_TIMEOUT_SECS` — subprocess timeout (default 180; legacy: `CODEX_TIMEOUT_SECS`)
- `CLAUDE_CODE_SESSION_FILE` — session ID persistence path (default: `data/claude_session_id`)
- `CLAUDE_CODE_MCP_CONFIG` — MCP config passed with `--strict-mcp-config` (default: `.mcp.json`; empty or missing file = default MCP discovery)
- `CLAUDE_CODE_ALLOWED_TOOLS` — tool allowlist (default: `mcp__telegram__*,WebSearch,WebFetch` — built-in Claude Code web tools included; legacy: `CODEX_ALLOWED_TOOLS`)
- `FACEIT_API_KEY` — optional FACEIT Data API key (enables faceit_profile, faceit_matches, faceit_match tools)
- `GOOGLE_SERVICE_ACCOUNT_KEY` — optional path to Google Cloud service account JSON key file (enables sheets_* and calendar_* tools)
- `GOOGLE_CALENDAR_ID` — default calendar for calendar_* tools (calendar must be shared with the service account)
- `BRAVE_SEARCH_API_KEY` / `TAVILY_API_KEY` — keyed web_search backends (DDG fallback is blocked from datacenter IPs)
- `GROQ_API_KEY` / `OPENAI_API_KEY` — Whisper backends for transcribe_voice
- `DATABASE_URL` — SQLite path (default `sqlite://data/jarvis.db`)
- `ALLOWED_USERS` / `OWNER_USER_ID` / `OWNER_IDS` — access control. `OWNER_IDS` is a comma-separated owner set (all get identical owner rights via `Config::is_owner`); `OWNER_USER_ID` (or the first `OWNER_IDS` entry) is the primary owner and the only recipient of owner DMs/notices
- `BOT_ALIASES` — trigger words in groups (default: `jarvis,bot`)
- `WEBHOOK_URL` / `WEBHOOK_BIND` — webhook mode (empty = long polling)
- `CONTEXT_WINDOW_TOKENS` — auto-compaction threshold + dashboard context gauge (default 200000)
- `SELF_IMPROVE_ENABLED` — enables `self_improve`/`self_deploy` (default false). Optional: `SELF_IMPROVE_SCRIPT` (default `bin/self-improve.sh`), `SELF_IMPROVE_REPO`, `SELF_IMPROVE_WORKTREE`, `SELF_IMPROVE_TARGET_DIR`, `SELF_IMPROVE_MODEL` (default `CLAUDE_CODE_MODEL`), `SELF_IMPROVE_MAX_TURNS` (80), `SELF_IMPROVE_MAX_BUDGET_USD` (5), `SELF_IMPROVE_AGENT_TIMEOUT_SECS` (3600)
- All `CLAUDE_CODE_*` vars fall back to legacy `CODEX_*` equivalents for backward compatibility

## Deployment

- **Platform**: native Ubuntu server with systemd. Full runbook: `deploy/README.md`.
- **Bootstrap**: `bash deploy/setup-ubuntu.sh` (apt deps, rust, systemd units,
  sudoers, build). Idempotent.
- **Dev dir**: source checkout (any user/path) — cargo build. `deploy.sh`/`jarvis.sh`
  take `JARVIS_DEV`/`JARVIS_PROD` env overrides (no hardcoded home).
- **Prod dir**: `/opt/jarvis` — `bin/ data/ prompts/ .env .mcp.json`
  (what systemd runs; `WorkingDirectory`).
- **Service**: systemd unit `deploy/jarvis-bot.service` → `ExecStart=/usr/local/bin/jarvis-guard … -- /opt/jarvis/bin/telegram-codex-jarvis`,
  `Restart=always`, `WorkingDirectory=/opt/jarvis`. Crash-loop guard via
  `StartLimitBurst`; deploy probation + rollback via `jarvis-guard` (replaces the old `jarvis-wrapper` binary).
- **claude auth (headless)**: `ANTHROPIC_API_KEY` in `.env` (API billing) OR
  `claude setup-token` as the service user (subscription; `HOME` set in unit).
- **Webhook ingress**: bot is proxy-agnostic (`WEBHOOK_URL` + `WEBHOOK_BIND`).
  Prod = Caddy (`deploy/Caddyfile`, auto-TLS on 443) if public IP+DNS; else
  cloudflared if behind NAT; else long polling (empty `WEBHOOK_URL`). Telegram
  webhooks only on ports 443/80/88/8443.
- **sudo**: `deploy/jarvis.sudoers` → NOPASSWD on only the jarvis-bot
  systemctl/journalctl verbs (replaces the old `echo '1' | sudo -S`).
- **Logging**: `sudo journalctl -u jarvis-bot -f`
- **MCP server**: `.mcp.json` points to `/opt/jarvis/bin/mcp-telegram` with `cwd: /opt/jarvis`
  (template: `.mcp.json.example`).

### Production layout (`/opt/jarvis`)

```
/opt/jarvis/
  .env                 — production config
  bin/
    telegram-codex-jarvis  — main bot binary
    mcp-telegram           — MCP server binary
    self-improve.sh        — self-edit job script
    VERSION                — sha of the installed build (self-improve base)
    *.prev                 — previous binaries kept for rollback
  data/
    jarvis.db              — SQLite database
    claude_session_id      — Claude Code session persistence
    self_improve/<job>/    — per-job state, task, log, summary.md, agent.json
    notices/               — pending owner DMs from jobs/guard
    deploy_pending         — present only while a deploy is in probation
  prompts/
    system_prompt.txt      — Jarvis personality (Uzbek)
    user_rules.txt         — response style rules
```

## Building & Deploying

```bash
# Quick deploy (build + copy + restart):
cd ~/projects/bots/jarvis
./deploy.sh

# Manual steps if needed:
cargo build --release
cp target/release/telegram-codex-jarvis /opt/jarvis/bin/
cp target/release/mcp-telegram /opt/jarvis/bin/
cp prompts/* /opt/jarvis/prompts/
sudo systemctl restart jarvis-bot

# Build prerequisites:
sudo apt install build-essential pkg-config libssl-dev
```

## Known Quirks

- **LLM backend**: uses Claude Code CLI with streaming JSON I/O; env vars support both `CLAUDE_CODE_*` and legacy `CODEX_*` for backward compatibility
- **CLAUDECODE env var**: must be removed when spawning `claude` subprocess to avoid nested session detection (handled in `claude_code.rs`)
- **Message batching**: channel-based debounce in `bot.rs` — rapid messages are collected for 1s then answered as one. Handler returns immediately, a per-chat `chat_batcher_task` sleeps 1s after first message, drains follow-ups, responds once.
- **Serialized subprocess access**: `send_message()` uses a `send_lock` mutex so concurrent batcher tasks queue up rather than racing. This matches the subprocess's inherently serial turn-taking. The `stdin_tx` lock is scoped to a block to prevent deadlocks with `stop()`.
- **Tone detection**: `detect_chat_tone()` in `bot.rs` analyzes last 10 messages for formal/casual word signals and emoji density. Only adjusts system prompt when signal difference > 2.
- **RAG is lexical only**: no embeddings, just token overlap scoring over the last `RAG_CANDIDATE_LIMIT` messages
- **Memory edit protection**: `MemoryManager` requires `read()` before `edit()` in the same session (tracked per MCP server instance, resets on restart)
- **SQLite WAL mode**: enabled on init for concurrent read performance
- **Media vision**: Photos/stickers sent as base64 to Claude via `send_message_with_image()`. Videos/GIFs use thumbnails only. Max download: 5MB images, 64KB text files.
- **Voice messages**: Voice/video-note messages are stored as metadata only (`[Ovozli xabar: Ns]`) — no automatic transcription, but the `transcribe_voice` MCP tool can transcribe on demand by file_id when GROQ_API_KEY/OPENAI_API_KEY is set.

## File Layout (dev)

```
~/projects/bots/jarvis/        — development directory
  .env / .env.example          — dev config
  .mcp.json                    — MCP config (points to /opt/jarvis)
  Cargo.toml / Cargo.lock
  CLAUDE.md                    — project documentation
  deploy.sh                    — build + deploy to /opt/jarvis
  prompts/
    system_prompt.txt          — Jarvis personality and privacy rules (Uzbek)
    user_rules.txt             — response style rules
  src/
    lib.rs                     — module exports
    main.rs                    — entrypoint, heartbeat task, owner startup notification
    bot.rs                     — teloxide dispatcher, message processing, tone detection, /health endpoint, structured logging
    dashboard.rs               — ops dashboard axum routes (/dash + /api/*), X-Dash-Auth gate
    dashboard.html             — self-contained dashboard SPA (embedded via include_str!)
    claude_code.rs             — streaming Claude Code subprocess manager
    config.rs                  — environment config parsing
    db.rs                      — SQLite layer (16 tables)
    self_update.rs             — self-edit paths, notices, restart flag, job spawner
    metrics.rs                 — BotMetrics: atomic counters for observability
    rag.rs                     — lexical RAG search
    focus.rs                   — single-chat focus mode
    memory.rs                  — persistent memory CRUD
    reminders.rs               — time-based reminder service
    scheduled_tasks.rs         — recurring cron job service (Claude oneshot execution)
    security.rs                — spam/rate/ban security gate
    proactive.rs               — proactive group-conversation evaluation
    chat_profiles.rs           — per-chat style profile analysis
    formatting.rs              — Telegram-HTML formatting helpers
    faceit/mod.rs              — FACEIT CS2 API client (HTTP API only)
    google_sheets.rs           — Google Sheets API client (service account auth, 10 MCP tools)
    gmail.rs                   — Gmail IMAP client
    smtp.rs                    — SMTP outbound email sender
    mcp.rs                     — MCP JSON-RPC handler (~87 tools, includes web_fetch/web_search)
    mcp_tools/                 — one module per MCP tool
    bin/mcp_server.rs          — MCP server binary
    bin/guard.rs               — jarvis-guard: deploy probation + rollback
  build.rs                     — bakes the git sha into JARVIS_GIT_SHA
  deploy/self-improve.sh       — self-edit job: sandboxed agent, gates, push, install
```
