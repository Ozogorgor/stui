#!/usr/bin/env bash
# scripts/build.sh — full stui build: runtime (Rust) + TUI (Go) + WASM plugins
#
# Usage:
#   ./scripts/build.sh                   # runtime + TUI + all metadata plugins
#   ./scripts/build.sh --no-plugins      # skip plugin build
#   ./scripts/build.sh --wasm-host       # runtime with full WASM execution support
#   ./scripts/build.sh --clean           # nuke caches first (cargo clean + go clean -cache)
#   ./scripts/build.sh tmdb              # pass plugin filter through to build-plugins.sh
#   ARIA2_SECRET=x ./scripts/build.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
mkdir -p "$DIST"
# Respect CARGO_TARGET_DIR so the binary we copy into dist/ is the one cargo
# actually just wrote — not a stale artifact in $ROOT/target/ from before the
# shared cache was configured.
CARGO_TGT="${CARGO_TARGET_DIR:-$ROOT/target}"

FEATURES=""
BUILD_PLUGINS=true
CLEAN=false
PLUGIN_ARGS=()

for arg in "$@"; do
    case "$arg" in
        --wasm-host)   FEATURES="--features wasm-host" ;;
        --no-plugins)  BUILD_PLUGINS=false ;;
        --clean)       CLEAN=true ;;
        --help|-h)
            echo "Usage: $0 [--wasm-host] [--no-plugins] [--clean] [plugin-name]"
            echo ""
            echo "By default, all metadata WASM plugins are compiled and installed."
            echo "Pass a plugin name to build only that plugin (forwarded to build-plugins.sh)."
            echo "--clean wipes cargo's target/ and Go's build cache for a true scratch build."
            exit 0
            ;;
        *)  PLUGIN_ARGS+=("$arg") ;;
    esac
done

# ── Optional pre-build clean ─────────────────────────────────────────────────
# `--clean` is for "I don't trust incremental compilation, give me a fresh
# build" situations: stale codegen, corrupt fingerprints, or after large
# refactors. It costs minutes but rules out caching as a suspect.
if [[ "$CLEAN" == "true" ]]; then
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "▶ Cleaning build caches…"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    cd "$ROOT" && cargo clean
    cd "$ROOT/tui" && go clean -cache
    echo "✓  caches cleared"
    echo ""
fi

# ── Runtime (Rust) ────────────────────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "▶ Building stui-runtime (Rust)…"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$ROOT"
if [[ -n "$FEATURES" ]]; then
    cargo build --release -p stui-runtime $FEATURES
else
    cargo build --release -p stui-runtime
fi
cp "$CARGO_TGT/release/stui-runtime" "$DIST/stui-runtime"
echo "✓  dist/stui-runtime  ($(du -h "$DIST/stui-runtime" | cut -f1))"

# ── TUI (Go) ──────────────────────────────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "▶ Building stui TUI (Go)…"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cd "$ROOT/tui"
go mod tidy
go build -ldflags="-s -w" -o "$DIST/stui" ./cmd/stui
echo "✓  dist/stui  ($(du -h "$DIST/stui" | cut -f1))"

# ── Plugins ───────────────────────────────────────────────────────────────────
if [[ "$BUILD_PLUGINS" == "true" ]]; then
    echo ""
    "$ROOT/scripts/build-plugins.sh" "${PLUGIN_ARGS[@]+"${PLUGIN_ARGS[@]}"}"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Build complete"
echo ""
echo "Run:  $DIST/stui"
echo "      $DIST/stui --no-runtime   # UI-only (no downloads)"
echo ""
echo "aria2c daemon (needed for downloads):"
echo '  aria2c --enable-rpc --rpc-secret=mystui \
    --seed-time=0 --dir="$HOME/Downloads/stui" --daemon'
echo "  export ARIA2_SECRET=mystui"
