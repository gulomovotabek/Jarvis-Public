#!/bin/bash
set -euo pipefail

# Lifecycle helper for the jarvis-bot systemd service on Ubuntu.
# sudo is plain (no piped password) — install deploy/jarvis.sudoers so the
# deploy user has NOPASSWD on exactly the jarvis-bot systemctl/journalctl verbs.
#
# Path overrides (env): JARVIS_DEV, JARVIS_PROD.
SERVICE="jarvis-bot"
DEV="${JARVIS_DEV:-$(cd "$(dirname "$0")" && pwd)}"
PROD="${JARVIS_PROD:-/opt/jarvis}"

usage() {
    echo "Usage: $0 {start|stop|restart|status|logs|deploy}"
    echo ""
    echo "  start    - Start the bot"
    echo "  stop     - Stop the bot"
    echo "  restart  - Restart the bot"
    echo "  status   - Show bot status"
    echo "  logs     - Tail live logs (Ctrl+C to exit)"
    echo "  deploy   - Build, copy to prod, restart (delegates to deploy.sh)"
    exit 1
}

kill_stale_mcp() {
    local pids
    pids=$(fuser "$PROD/bin/mcp-telegram" 2>/dev/null | tr -d ' ') || true
    if [ -n "$pids" ]; then
        echo "Killing stale mcp-telegram (PID $pids)..."
        kill $pids 2>/dev/null || true
        sleep 1
    fi
}

cmd_start() {
    echo "Starting $SERVICE..."
    sudo systemctl start "$SERVICE"
    sleep 2
    sudo systemctl status "$SERVICE" --no-pager | head -10
}

cmd_stop() {
    echo "Stopping $SERVICE..."
    sudo systemctl stop "$SERVICE"
    kill_stale_mcp
    echo "Stopped."
}

cmd_restart() {
    echo "Restarting $SERVICE..."
    sudo systemctl restart "$SERVICE"
    sleep 2
    sudo systemctl status "$SERVICE" --no-pager | head -10
}

cmd_status() {
    sudo systemctl status "$SERVICE" --no-pager
}

cmd_logs() {
    sudo journalctl -u "$SERVICE" -f
}

cmd_deploy() {
    JARVIS_DEV="$DEV" JARVIS_PROD="$PROD" "$DEV/deploy.sh"
}

[ $# -eq 0 ] && usage

case "$1" in
    start)   cmd_start   ;;
    stop)    cmd_stop     ;;
    restart) cmd_restart  ;;
    status)  cmd_status   ;;
    logs)    cmd_logs     ;;
    deploy)  cmd_deploy   ;;
    *)       usage        ;;
esac
