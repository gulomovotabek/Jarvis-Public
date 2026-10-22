# Jarvis — Architect Review & Work Plan

Date: 2026-04-11
Scope: full source review of `src/` with emphasis on `mcp.rs`, `bot.rs`, `db.rs`, `claude_code.rs`, `faceit.rs`.

## Verdict

The bot works, but three structural problems are compounding:

1. `mcp.rs` has tool definitions in two places (handler match + hand-rolled JSON schemas) — every new tool touches three files and nothing enforces they agree.
2. `bot.rs::process_message` tangles eight concerns in one 200+ line function — unreadable, untestable.
3. Zero tests — every refactor is a gamble.

Everything else is secondary. Kill these three and the next 50 features are cheap.

---

## Security — fix now

### S1. SQL injection in `query` MCP tool — CRITICAL
**Where:** `src/db.rs` `execute_select_query` (~line 1462)
**Problem:** Keyword blocklist (`DROP`, `DELETE`, etc.) is bypassed by CTEs, stacked statements, comments. Blocklists for SQL never work long-term, and Claude will eventually route around them.
**Fix:** Parse with `sqlparser` crate; allow only a single `Query` node with no writes. Or restrict to named views.

### S2. Path traversal in `download_file` — HIGH
**Where:** `src/mcp.rs` `download_file` handler (~line 2246)
**Problem:** User-supplied filename concatenated into path without validation. Claude can write `../../etc/...`.
**Fix:** `Path::new(name).file_name()` + reject empty/weird chars, mirror `memory.rs` validation.

### S3. Error messages leak internals to Telegram users — HIGH
**Where:** `src/mcp.rs` ~line 92 — errors serialized with `{e:#}` (full anyhow context chain)
**Problem:** DB paths, SQL fragments, possibly keys reach user-visible messages.
**Fix:** Split `UserError` (safe to render) vs `InternalError` (logged only, generic message to user).

### S4. `send_message` lock has no timeout — HIGH
**Where:** `src/claude_code.rs` `send_lock`
**Problem:** One stuck Claude turn blocks every batcher forever. Relocates the same failure mode the deadlock fix was supposed to eliminate.
**Fix:** `tokio::time::timeout(CLAUDE_TIMEOUT, send_lock.lock())` — fail fast.

### S5. `download_file` size check is racy — MEDIUM
**Where:** `src/mcp.rs` ~line 2239
**Problem:** Checks `Content-Length` header (lie-able), then `bytes.len()` after reading entire body into memory.
**Fix:** Stream to disk with running byte counter; abort mid-stream on overflow.

### S6. Memory edit-protection is process-global — MEDIUM
**Where:** `src/memory.rs` `HashSet<String> read_paths`
**Problem:** Two concurrent MCP callers share the same read set — A reads, B edits without reading.
**Fix:** Key read tracking by session/request ID, or switch to content-hash CAS (`edit(path, expected_hash, new)`).

---

## Structural — highest leverage

### A1. `mcp.rs` tool dispatch + schema duplication
**Where:** `src/mcp.rs` ~1100-line `call_tool` match + separate ~850-line schema block
**Problem:** 73 tools defined in 2-3 places. Parameter extraction pattern `args.get("x").and_then(|v| v.as_str()).context(...)` copy-pasted 70+ times. Schema drift is silent.
**Fix:** Introduce a `Tool` trait:
```rust
trait Tool: Send + Sync {
    type Params: JsonSchema + DeserializeOwned;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn run(&self, params: Self::Params, ctx: &Ctx) -> Result<String>;
}
```
Register in `HashMap<&str, Arc<dyn ErasedTool>>` at startup. Use `schemars` to generate `inputSchema` from the params struct — single source of truth. Deletes ~1500 lines, new tools become ~30-line files.

### A2. `bot.rs::process_message` pipeline extraction
**Where:** `src/bot.rs` ~line 188-403
**Problem:** One function handles ingest, upsert, media extract, monitor check, security gate, focus gate, admin commands, batcher dispatch, proactive spawn.
**Fix:** Typed state pipeline — `Raw → Enriched → Gated → Dispatched`. Each stage is a pure function returning `Result<Next, Drop(reason)>`. Gates become unit-testable without teloxide.

### A3. Zero tests — blocks safe refactoring
**Problem:** No `#[test]` or `#[tokio::test]` anywhere in the repo.
**Fix:** Five tests that unlock everything else:
1. Tool params struct ↔ schema roundtrip (catches drift on every `cargo test`).
2. Security gate truth table (spam/rate/strike/owner bypass).
3. Memory path-traversal rejection corpus.
4. Batcher window behavior under rapid sends (with fake clock).
5. SQL injection corpus against the `query` tool.

---

## Optimizations — do during normal work

### O1. Proactive trigger spawns unbounded tasks
`src/bot.rs:~316` — `tokio::spawn` per group message with no backpressure. Use `Arc<Semaphore>` per chat, `try_acquire()` + drop on full.

### O2. No tracing spans
`#[instrument]` with a `batch_id` field → end-to-end causality from Telegram message → batch → Claude turn → MCP tool call. Currently impossible to correlate.

### O3. SQLite pool uses defaults
`src/db.rs:~86` — add `.max_connections(25).acquire_timeout(5s)`. Under load, acquisition blocks silently. WAL still serializes writes, so consider one-writer / many-readers split.

### O4. Claude subprocess stderr is discarded
`src/claude_code.rs:~349` — forward stderr as `SubprocessEvent::Stderr` to owner DM, same path compression events already use. Currently flying blind on subprocess failures.

### O5. Feature flags
`Cargo.toml` — FACEIT, Sheets, Playwright, whisper all always compiled. `[features] faceit / sheets / voice` with `#[cfg]` gates saves build time + binary size.

### O6. `faceit.rs` at 60KB
Split: `faceit/api.rs` (HTTP), `faceit/demo.rs` (parser), `faceit/browser.rs` (Playwright shim). Each becomes testable with HTTP mocks.

### O7. `db.rs` `seed_monitored_groups` N+1
`src/db.rs:~1159` — one INSERT per entry in a loop. Batch into a single multi-row `INSERT OR IGNORE`.

### O8. Metrics are only 7 counters
`src/metrics.rs` — no histograms (latency), no gauges (queue depth), no per-tool call counts. Add `claude_request_duration_secs` histogram, `tool_calls_by_name` counter vec.

### O9. Playwright subprocess has no timeout
`src/faceit.rs:~310` — `tokio::process::Command` spawn with no `tokio::time::timeout` wrapper. Hang blocks the caller indefinitely.

### O10. JSON-RPC error codes are imprecise
`src/mcp.rs:~100` — returns `-32601` for both missing methods and tool-not-found. Distinguish protocol errors from tool errors.

### O11. Focus + Security double-check
`src/bot.rs` — gates run independently. Pass a `GateDecision` between them to avoid redundant work.

### O12. Batcher task silent panic risk
`src/bot.rs:~434` — `rx.recv().await` without error context. Log on channel close, add graceful shutdown signal.

---

## One-week execution plan

### Day 1 — Security (all small, independent PRs)
- [ ] S1: SQL parser for `query` tool
- [ ] S2: `download_file` filename sanitization
- [ ] S3: `UserError` / `InternalError` split
- [ ] S4: `send_lock` timeout

### Day 2–3 — `Tool` trait proof of concept (A1)
- [ ] Define `Tool` trait + `ErasedTool` type-erased wrapper
- [ ] Add `schemars` dep
- [ ] Migrate 2 tools (`now`, `search_messages`) as proof
- [ ] Add schema-drift test
- [ ] Keep old match as fallback for unmigrated tools

### Day 4 — Finish tool migration
- [ ] Migrate remaining 71 tools in batches (messaging → memory → sheets → faceit → admin)
- [ ] Delete hand-rolled schema block
- [ ] `mcp.rs` drops from 110KB to <30KB

### Day 5 — `process_message` pipeline (A2) + tests (A3)
- [ ] Extract pipeline stages
- [ ] Write five unlocking tests
- [ ] Add `#[instrument]` spans on pipeline stages (gets O2 for free)

After this week: O1–O12 happen during normal feature work. The debt compounds on A1 and A3 — ship those first.
