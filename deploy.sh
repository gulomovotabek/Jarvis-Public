#!/bin/bash
set -euo pipefail

# Build + copy to prod + restart. Paths are overridable via env so this works
# on any Ubuntu box regardless of the dev user's home.
#   JARVIS_DEV  — source checkout (default: this script's directory)
#   JARVIS_PROD — runtime dir systemd runs from (default: /opt/jarvis)
DEV="${JARVIS_DEV:-$(cd "$(dirname "$0")" && pwd)}"
PROD="${JARVIS_PROD:-/opt/jarvis}"

echo "Building release ($DEV)..."
cargo build --release --manifest-path "$DEV/Cargo.toml"

echo "Copying binaries..."
install -m 0755 "$DEV/target/release/telegram-codex-jarvis" "$PROD/bin/"
install -m 0755 "$DEV/target/release/mcp-telegram" "$PROD/bin/"

echo "Copying prompts..."
cp "$DEV/prompts/"* "$PROD/prompts/"

# .mcp.json is environment-specific (gitignored). Only seed it from the example
# if prod doesn't have one yet — never clobber a hand-tuned prod config.
if [ ! -f "$PROD/.mcp.json" ] && [ -f "$DEV/.mcp.json.example" ]; then
    echo "Seeding .mcp.json from example..."
    cp "$DEV/.mcp.json.example" "$PROD/.mcp.json"
fi

echo "Restarting service..."
sudo systemctl restart jarvis-bot

sleep 2
sudo systemctl status jarvis-bot --no-pager | head -10

echo ""
echo "Deploy complete."
