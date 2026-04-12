#!/usr/bin/env bash
# scripts/aria2c-start.sh — start aria2c as a daemon configured for stui.
#
# This script starts aria2c with the RPC interface enabled and sane defaults
# for stui's download-then-stream workflow.
#
# Usage:
#   ./scripts/aria2c-start.sh               # use default secret from env or prompt
#   ARIA2_SECRET=mysecret ./scripts/aria2c-start.sh
#   ./scripts/aria2c-start.sh --stop        # stop the running daemon
#   ./scripts/aria2c-start.sh --status      # show running downloads
#
# After starting, set in your shell (or add to ~/.config/stui/config.toml):
#   export ARIA2_SECRET=<the-secret-you-chose>
#   export ARIA2_URL=http://127.0.0.1:6800/jsonrpc
#   export ARIA2_DIR=$HOME/Downloads/stui

set -euo pipefail

ARIA2_PORT="${ARIA2_PORT:-6800}"
ARIA2_DIR="${ARIA2_DIR:-$HOME/Downloads/stui}"
ARIA2_SECRET="${ARIA2_SECRET:-}"
PIDFILE="/tmp/stui-aria2c.pid"
LOGFILE="${XDG_CACHE_HOME:-$HOME/.cache}/stui/aria2c.log"

mkdir -p "$(dirname "$LOGFILE")" "$ARIA2_DIR"

# ── Parse args ────────────────────────────────────────────────────────────────
case "${1:-}" in
    --stop)
        if [[ -f "$PIDFILE" ]]; then
            pid=$(cat "$PIDFILE")
            kill "$pid" 2>/dev/null && echo "✓ aria2c stopped (pid $pid)" \
                                    || echo "✗ aria2c was not running"
            rm -f "$PIDFILE"
        else
            echo "aria2c is not running (no pidfile)"
        fi
        exit 0
        ;;
    --status)
        if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
            echo "✓ aria2c running (pid $(cat "$PIDFILE"))"
            echo "  RPC: http://127.0.0.1:$ARIA2_PORT/jsonrpc"
            echo "  Log: $LOGFILE"
        else
            echo "✗ aria2c is not running"
        fi
        exit 0
        ;;
    --help|-h)
        grep '^#' "$0" | sed 's/^# \?//'
        exit 0
        ;;
esac

# ── Check aria2c is installed ─────────────────────────────────────────────────
if ! command -v aria2c &>/dev/null; then
    echo "✗ aria2c not found. Install it:"
    echo "  Arch:   sudo pacman -S aria2"
    echo "  Debian: sudo apt install aria2"
    echo "  macOS:  brew install aria2"
    exit 1
fi

# ── Stop existing instance ────────────────────────────────────────────────────
if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
    echo "→ Stopping existing aria2c (pid $(cat "$PIDFILE"))..."
    kill "$(cat "$PIDFILE")"
    sleep 0.5
fi

# ── Prompt for secret if not set ─────────────────────────────────────────────
if [[ -z "$ARIA2_SECRET" ]]; then
    # Generate a random secret if not provided
    ARIA2_SECRET=$(head -c 16 /dev/urandom | base64 | tr -d '+/=' | head -c 16)
    echo "→ Generated RPC secret: $ARIA2_SECRET"
    echo "  Add to your shell: export ARIA2_SECRET=$ARIA2_SECRET"
fi

# ── Launch aria2c ─────────────────────────────────────────────────────────────
echo "→ Starting aria2c..."
echo "  Port:      $ARIA2_PORT"
echo "  Downloads: $ARIA2_DIR"
echo "  Log:       $LOGFILE"

aria2c \
    --enable-rpc \
    --rpc-listen-port="$ARIA2_PORT" \
    --rpc-secret="$ARIA2_SECRET" \
    --rpc-allow-origin-all \
    --rpc-listen-all=false \
    `# Download behaviour` \
    --dir="$ARIA2_DIR" \
    --continue=true \
    --max-concurrent-downloads=5 \
    --max-connection-per-server=4 \
    --split=4 \
    --min-split-size=1M \
    `# BitTorrent` \
    --enable-dht=true \
    --enable-peer-exchange=true \
    --bt-enable-lpd=true \
    --bt-max-peers=55 \
    --seed-time=0 \
    --seed-ratio=0.0 \
    --follow-torrent=true \
    `# Daemon mode` \
    --daemon=true \
    --log="$LOGFILE" \
    --log-level=notice \
    --save-session="$ARIA2_DIR/.aria2-session.gz" \
    --save-session-interval=60 \
    --input-file="$ARIA2_DIR/.aria2-session.gz" \
    2>/dev/null || true

# Grab the PID
sleep 0.3
pgrep -f "aria2c.*rpc-listen-port=$ARIA2_PORT" > "$PIDFILE" 2>/dev/null || true

if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
    echo ""
    echo "✓ aria2c started (pid $(cat "$PIDFILE"))"
    echo ""
    echo "Configure stui:"
    echo "  export ARIA2_URL=http://127.0.0.1:$ARIA2_PORT/jsonrpc"
    echo "  export ARIA2_SECRET=$ARIA2_SECRET"
    echo "  export ARIA2_DIR=$ARIA2_DIR"
else
    echo "✗ aria2c failed to start. Check log: $LOGFILE" >&2
    exit 1
fi
