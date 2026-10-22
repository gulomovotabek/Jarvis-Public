# Telegram Codex Jarvis

Rust Telegram bot that stores messages in SQLite and currently answers through a long-running Claude Code CLI subprocess.

## Instruction scope

- `CLAUDE.md` is the detailed architecture and operations reference. Before non-trivial work, read only the sections relevant to the files being changed.
- Mentions of Claude, `claude_code.rs`, `CLAUDE_CODE_*`, `claude --print`, and Claude event formats describe the product's current implementation. Do not mechanically rename them to Codex.
- Verify implementation details in the source before relying on the documentation; the code is authoritative when they differ.

## Architecture map

- `src/main.rs`: process wiring, background tasks, heartbeat, startup notification.
- `src/bot.rs`: Telegram dispatcher, ingestion, triggers, security/focus gates, media handling, `/health`, logging.
- `src/claude_code.rs`: streaming subprocess, session persistence, serialization, timeout and child cleanup.
- `src/mcp.rs` and `src/mcp_tools/`: stdio MCP server and tool dispatch.
- `src/db.rs`: SQLite schema and queries; WAL mode.
- `src/security.rs`, `src/focus.rs`, `src/proactive.rs`: access and response gating.
- `src/memory.rs`, `src/reminders.rs`, `src/scheduled_tasks.rs`: persistent assistant state and jobs.
- `src/dashboard.rs` and `src/dashboard.html`: authenticated operations dashboard.
- Integration modules include FACEIT, Google Sheets/Calendar, Gmail/SMTP, PDF, RSS, web, and transcription support.

## Critical invariants

- Preserve the owner-only top-level gate in `classify_message`; non-owner messages may be stored but must not reach response or privileged-tool paths. The gate fails closed when `OWNER_USER_ID` is unset.
- Keep `send_message()` serialized with `send_lock`. Scope the `stdin_tx` guard so timeout recovery can acquire the lock and stop the child without deadlocking.
- `stop()` must terminate the OS child, not only abort Tokio I/O tasks.
- Memory edits require a prior read in the same MCP session. Keep path traversal checks for `..`, absolute paths, and null bytes.
- Scheduled tasks must remain single-run: retain the `running` guard, startup stale-flag recovery, and sequential subprocess access.
- Verify webhooks after setup and retain structured, greppable event names used by operations.
- Keep SQLite WAL mode and server-side read-only enforcement for the generic query tool.
- Treat `.env`, bot tokens, API keys, app passwords, service-account JSON, and dashboard auth as secrets. Never commit or print them.

## Development and verification

- Keep MCP tools in focused modules under `src/mcp_tools/` and register them through the existing dispatcher.
- Preserve feature gating in `Cargo.toml`; production currently enables all default integrations.
- Run `cargo fmt --check`, `cargo check`, and relevant tests after Rust changes. Use `cargo build --release --bin telegram-codex-jarvis --bin mcp-telegram` for a release-build verification when the change affects deployment.
- Do not deploy, restart production services, alter `/opt/jarvis`, or mutate production data unless the user explicitly requests it.

## Deployment

- Canonical runbook: `deploy/README.md`.
- Production is native Ubuntu with systemd; Windows/WSL service artifacts are obsolete.
- Production working directory is `/opt/jarvis`; the service is `jarvis-bot`.
- `deploy.sh` builds, copies artifacts, and restarts the service. Treat it as an external deployment action requiring explicit authorization.
- The backend supports Anthropic API-key or setup-token authentication. Preserve `CLAUDE_CODE_*` variables and their legacy `CODEX_*` fallbacks unless the requested change is an intentional backend migration.

## Known behavior

- Incoming messages are stored before response gating and indexed with lexical RAG; there are no embeddings.
- Media handling is size-limited; voice/video notes are metadata unless transcribed on demand.
- Rapid messages are debounced and batched per chat.
- Dashboard metrics and logs depend on stable counter and event names; update documentation and consumers together when changing them.
