# Ubuntu Deployment Runbook

Target: a plain Ubuntu server running systemd.

## Layout

| Path | Role |
|------|------|
| source checkout (any dir/user) | dev: `cargo build`, `git` |
| `/opt/jarvis` | runtime: `bin/ data/ prompts/ .env .mcp.json` |

`WorkingDirectory=/opt/jarvis` — every path in the code is relative to it
(`data/jarvis.db`, `prompts/*`). Same reason `.mcp.json` keeps `cwd: /opt/jarvis`.

## One-shot bootstrap

```bash
git clone <repo> ~/jarvis && cd ~/jarvis
bash deploy/setup-ubuntu.sh        # apt deps, rust, node (for claude CLI), unit, build
```

Overrides: `JARVIS_USER=jarvis JARVIS_PROD=/opt/jarvis bash deploy/setup-ubuntu.sh`.

Then edit `/opt/jarvis/.env` and start:

```bash
sudo systemctl start jarvis-bot
journalctl -u jarvis-bot -f
```

## claude CLI auth (the #1 thing that breaks headless)

The whole tree (bot → claude → mcp-telegram) runs as the service user. `claude`
reads creds from that user's `$HOME`. Pick one:

- **API billing**: set `ANTHROPIC_API_KEY=` in `/opt/jarvis/.env`. The subprocess
  inherits it (`claude_code.rs` only strips `CLAUDECODE`).
- **Subscription billing**: run `claude setup-token` (or `claude login`) **as the
  service user**, so `~/.claude/.credentials.json` exists. The unit sets
  `HOME=/home/<user>`. On Linux claude uses a file (not a desktop keyring), so
  this works without a graphical session.

## Ingress

Bot is ingress-agnostic: it only needs `WEBHOOK_URL` (public HTTPS) + `WEBHOOK_BIND`
(internal upstream). Telegram delivers webhooks only to ports **443/80/88/8443**.

- **Caddy (recommended for prod)** — box has a public IP + DNS A record, ports
  80/443 open. Auto Let's Encrypt TLS, one binary:
  ```bash
  sudo apt install -y caddy
  sudo cp deploy/Caddyfile /etc/caddy/Caddyfile   # edit domain + upstream port
  sudo systemctl reload caddy
  # .env: WEBHOOK_URL=https://jarvis.example.com/<secret-path>  WEBHOOK_BIND=0.0.0.0:8081
  ```
- **cloudflared** — box behind NAT / dynamic IP / no open inbound ports:
  ```bash
  cloudflared tunnel --url http://localhost:8081
  # or a named tunnel mapping jarvis.example.com -> http://localhost:8081
  ```
- **Long polling** — leave `WEBHOOK_URL` empty. No proxy, no public URL. Simplest.

## Day-to-day

```bash
./jarvis.sh status | logs | restart | deploy
./deploy.sh            # rebuild + copy + restart (paths via JARVIS_DEV/JARVIS_PROD)
```

`sudo` is plain — `deploy/jarvis.sudoers` grants the deploy user NOPASSWD on only
the jarvis-bot systemctl/journalctl verbs.

## Build prerequisites (installed by setup-ubuntu.sh)

```
build-essential pkg-config   # Rust native builds (ring/rustls)
libssl-dev                   # native-tls (Gmail IMAP)
nodejs + npm                 # only for the claude CLI (@anthropic-ai/claude-code)
rust stable
```

No cmake/clang/ffmpeg/xvfb/playwright — whisper, the demoparser, and the
headed-browser tooling were all removed.

## Pause / Resume (disable without losing context)

Stopping the service deletes nothing. All context lives in `/opt/jarvis` and
survives a pause:

| Path | What it holds |
|------|---------------|
| `data/jarvis.db` | full chat history + stored memories — SQLite, additive schema |
| `data/claude_session_id` | Claude Code session id; `--resume`d on next start so the conversation continues |
| `.env` (perms 600) | all secrets/config |
| `prompts/system_prompt.txt`, `prompts/user_rules.txt` | Jarvis personality + style |

### Disable

```bash
sudo systemctl stop jarvis-bot      # kills bot + its claude/mcp children
sudo systemctl disable jarvis-bot   # no auto-start on reboot
systemctl is-active jarvis-bot      # -> inactive
systemctl is-enabled jarvis-bot     # -> disabled
```

The Telegram webhook is left registered; while paused, Telegram retries the dead
upstream and drops updates older than ~24h — harmless. `/opt/jarvis` is untouched.

### Re-enable (same context)

```bash
# (optional) switch to pay-as-you-go API billing before starting:
#   edit /opt/jarvis/.env -> ANTHROPIC_API_KEY=sk-ant-...   (leave empty = claude subscription auth)
# (optional, only if claude auth expired) claude setup-token   # as the service user

sudo systemctl enable --now jarvis-bot
systemctl is-active jarvis-bot                 # -> active
journalctl -u jarvis-bot -f                    # watch startup
```

On start the bot re-sets the Telegram webhook from `WEBHOOK_URL`, `--resume`s the
saved Claude session, and reopens the same SQLite DB — history, memories, and
conversation continuity all intact. The owner gets a Telegram startup ping.

Smoke test: DM the bot `salom` → expect a reply (claude must call the
`bot_send_message` MCP tool; the bot discards claude's raw text by design).

### Lower call volume (to stay under the metered credit cap)

If re-enabling on subscription credit rather than an API key, cut `claude` calls:
- `PROACTIVE_COLD_TRIGGER_ENABLED=false` in `.env` — stops evaluating every other
  group message.
- Prune/disable scheduled tasks (MCP: `list_scheduled_tasks` then
  `update_scheduled_task` `enabled=false`, or `delete_scheduled_task`).
- Raise `PROACTIVE_COOLDOWN_SECS`.

### Full redeploy from a clean box

See **One-shot bootstrap** above (`deploy/setup-ubuntu.sh`). Restore context by
copying the saved `/opt/jarvis/data/` (DB + session id), `.env`, and `prompts/`
into the new `/opt/jarvis` before `systemctl start`.

## Dashboard

Live ops dashboard behind a reverse proxy, e.g. **https://status.example.com**
(HTTP basic auth).
Tabs: Overview (live counters, latency p50/p90/p99, context-fill gauge),
Messages (browse/paginate chat history), Usage (per-turn tokens/cost charts),
Tools (MCP tool call stats), Behavior (msgs/day, active-hours heatmap, chat
profiles, scheduled tasks).

**Serving:** routes (`/dash`, `/api/*`) live on the bot's own axum server
(`127.0.0.1:8081`, the same one serving the Telegram webhook) — code in
`src/dashboard.rs` + embedded `src/dashboard.html` (self-contained, inline SVG
charts, polls every 3s). No separate process.

**Two-layer auth:**
1. nginx vhost `status.example.com` (separate from the webhook vhost):
   `auth_basic` against `/etc/nginx/.htpasswd-jarvis`, then it injects
   `X-Dash-Auth: <secret>` on proxy.
2. Every dashboard handler re-checks that header against `DASHBOARD_AUTH_SECRET`
   in `.env`. So the routes 403 if reached via the webhook host
   (`jarvis.example.com` doesn't inject the header) — the webhook vhost is left
   untouched and dashboard routes stay invisible there.

**Config (`.env`):**
- `DASHBOARD_AUTH_SECRET` — shared secret; unset ⇒ routes return 404 (disabled).
- `CONTEXT_WINDOW_TOKENS` — denominator for the context-fill gauge (200000 for
  standard opus-4-8; set 1000000 if you move to a 1M-context model).

**Data captured for it:** `claude_turns` (per-turn tokens/latency, written
from `SubprocessEvent::TurnMetrics`), `claude_events` (compaction/rotation),
`tool_calls` (written by `mcp-telegram`). All additive tables, created in
`db.init()`.

**Cost is token-derived.** The CLI's reported `cost_usd` is **ignored**.
`dash_turns` computes cost at read time from token counts at official Opus 4.8
rates ($/1M: input 5, output 25, cache-write 6.25, cache-read 0.5) — an
official-rate **estimate**, not your actual bill. If you switch models, update
the rate constants in `Database::dash_turns` (`src/db.rs`). When prompt caching
misses, **cache-write dominates cost** — the Usage tab surfaces it.

**Rotate the basic-auth password:**
```bash
printf 'admin:%s\n' "$(openssl passwd -apr1 'NEWPASS')" > /etc/nginx/.htpasswd-jarvis
systemctl reload nginx
```
**Rotate the header secret:** change `DASHBOARD_AUTH_SECRET` in `/opt/jarvis/.env`
AND the `proxy_set_header X-Dash-Auth` in the nginx vhost to the same value, then
`systemctl restart jarvis-bot && systemctl reload nginx`.
