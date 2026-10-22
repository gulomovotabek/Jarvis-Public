#!/bin/bash
set -euo pipefail

# One-time Ubuntu server bootstrap for Jarvis. Idempotent — safe to re-run.
# Run from the source checkout as a sudo-capable user:
#   bash deploy/setup-ubuntu.sh
#
# Overrides (env):
#   JARVIS_USER  — service user (default: current $USER)
#   JARVIS_PROD  — runtime dir (default: /opt/jarvis)
JARVIS_USER="${JARVIS_USER:-$USER}"
PROD="${JARVIS_PROD:-/opt/jarvis}"
DEV="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> Service user: $JARVIS_USER   Prod dir: $PROD   Source: $DEV"

echo "==> Installing apt packages..."
# build-essential + pkg-config: Rust native builds (ring/rustls).
# libssl-dev: native-tls (Gmail IMAP). No cmake/clang/ffmpeg/xvfb — the
# whisper, demoparser, and headed-browser features were all removed.
sudo apt-get update
sudo apt-get install -y \
    build-essential pkg-config libssl-dev curl ca-certificates \
    git python3 bubblewrap socat

if [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo 0)" = 1 ] && [ ! -f /etc/apparmor.d/bwrap ]; then
    echo "==> AppArmor userns profile for bubblewrap (self-improve sandbox)..."
    sudo tee /etc/apparmor.d/bwrap >/dev/null <<'EOF'
abi <abi/4.0>,
include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,

  include if exists <local/bwrap>
}
EOF
    sudo apparmor_parser -r /etc/apparmor.d/bwrap
fi

echo "==> Rust toolchain..."
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

echo "==> Node.js + npm (only needed for the claude CLI)..."
if ! command -v node >/dev/null 2>&1; then
    curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -
    sudo apt-get install -y nodejs
fi

echo "==> claude CLI (Claude Code)..."
if ! command -v claude >/dev/null 2>&1; then
    sudo npm install -g @anthropic-ai/claude-code
fi
echo "    AUTH (do this as $JARVIS_USER after bootstrap): set ANTHROPIC_API_KEY in"
echo "    $PROD/.env, OR run 'claude setup-token' so ~/.claude/.credentials.json exists."

echo "==> Prod directory layout..."
sudo mkdir -p "$PROD"/{bin,data,prompts}
sudo chown -R "$JARVIS_USER:$JARVIS_USER" "$PROD"

echo "==> Copying prompts..."
cp "$DEV/prompts/"* "$PROD/prompts/"

echo "==> Seeding config files (only if absent)..."
[ -f "$PROD/.env" ]      || { cp "$DEV/.env.example" "$PROD/.env"; echo "    Created $PROD/.env — EDIT IT (token, ANTHROPIC_API_KEY, etc.)"; }
[ -f "$PROD/.mcp.json" ] || cp "$DEV/.mcp.json.example" "$PROD/.mcp.json"

echo "==> Installing systemd unit (substituting user)..."
sed "s/__USER__/$JARVIS_USER/g" "$DEV/deploy/jarvis-bot.service" | sudo tee /etc/systemd/system/jarvis-bot.service >/dev/null

echo "==> Installing sudoers drop-in..."
sed "s/__USER__/$JARVIS_USER/g" "$DEV/deploy/jarvis.sudoers" | sudo tee /etc/sudoers.d/jarvis >/dev/null
sudo chmod 0440 /etc/sudoers.d/jarvis
sudo visudo -cf /etc/sudoers.d/jarvis

echo "==> Building release binaries..."
( cd "$DEV" && cargo build --release )
install -m 0755 "$DEV/target/release/telegram-codex-jarvis" "$PROD/bin/"
install -m 0755 "$DEV/target/release/mcp-telegram" "$PROD/bin/"
install -m 0755 "$DEV/deploy/self-improve.sh" "$PROD/bin/"
"$PROD/bin/telegram-codex-jarvis" --version >"$PROD/bin/VERSION"
sudo install -m 0755 -o root -g root "$DEV/target/release/jarvis-guard" /usr/local/bin/jarvis-guard

echo "==> Enabling service..."
sudo systemctl daemon-reload
sudo systemctl enable jarvis-bot.service

echo ""
echo "Bootstrap complete. Next:"
echo "  1. Edit $PROD/.env (TELOXIDE_TOKEN, ANTHROPIC_API_KEY or claude login, webhook)."
echo "  2. Start the bot:   sudo systemctl start jarvis-bot"
echo "  3. Watch logs:      journalctl -u jarvis-bot -f"
echo "  4. (webhook) configure Caddy or cloudflared — see deploy/README.md"
echo "  5. (optional) self-edit: deploy key + clone + SELF_IMPROVE_ENABLED — see deploy/README.md"
