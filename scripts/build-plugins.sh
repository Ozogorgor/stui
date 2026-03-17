#!/usr/bin/env bash
# scripts/build-plugins.sh — compile all stui WASM plugins and install them.
#
# Usage:
#   ./scripts/build-plugins.sh                  # build all, install to ~/.stui/plugins/
#   ./scripts/build-plugins.sh prowlarr          # build only prowlarr-provider
#   ./scripts/build-plugins.sh --no-install      # build only, don't copy to ~/.stui/
#   PLUGIN_DIR=/custom/path ./scripts/build-plugins.sh
#
# Requirements:
#   rustup target add wasm32-wasip1
#   cargo (Rust 1.78+)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
PLUGIN_DIR="${PLUGIN_DIR:-$HOME/.stui/plugins}"
TARGET="wasm32-wasip1"
INSTALL=true
FILTER=""

# ── Parse args ────────────────────────────────────────────────────────────────
for arg in "$@"; do
    case "$arg" in
        --no-install) INSTALL=false ;;
        --help|-h)
            echo "Usage: $0 [plugin-name] [--no-install]"
            echo ""
            echo "Available plugins:"
            echo "  prowlarr          Search torrents via Prowlarr"
            echo "  opensubtitles     Subtitle search/download via OpenSubtitles.com"
            exit 0
            ;;
        *) FILTER="$arg" ;;
    esac
done

# ── Ensure WASM target is installed ──────────────────────────────────────────
if ! rustup target list --installed 2>/dev/null | grep -q "$TARGET"; then
    echo "▶ Installing Rust target $TARGET..."
    rustup target add "$TARGET"
fi

# ── Plugin definitions ────────────────────────────────────────────────────────
declare -A PLUGINS=(
    ["prowlarr"]="prowlarr-provider"
    ["opensubtitles"]="opensubtitles-provider"
)

build_plugin() {
    local short_name="$1"
    local crate_name="$2"
    local plugin_dir="$ROOT/plugins/$crate_name"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "▶ Building $crate_name → $TARGET"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    (
        cd "$plugin_dir"
        cargo build \
            --release \
            --target "$TARGET" \
            -Z build-std=std,panic_abort \
            -Z build-std-features=panic_immediate_abort \
            2>&1 || \
        cargo build \
            --release \
            --target "$TARGET"
        # ↑ Fallback without -Z flags if nightly features aren't available
    )

    # Locate the .wasm output
    local wasm_file
    wasm_file=$(find "$plugin_dir/target/$TARGET/release" \
        -name "*.wasm" -not -name "*.d" | head -1)

    if [[ -z "$wasm_file" ]]; then
        echo "✗ Build succeeded but no .wasm file found!" >&2
        return 1
    fi

    local wasm_size
    wasm_size=$(du -sh "$wasm_file" | cut -f1)
    echo "✓ Built: $wasm_file ($wasm_size)"

    if [[ "$INSTALL" == "true" ]]; then
        local dest="$PLUGIN_DIR/$crate_name"
        mkdir -p "$dest"
        cp "$wasm_file" "$dest/plugin.wasm"
        cp "$plugin_dir/plugin.toml" "$dest/plugin.toml"
        echo "✓ Installed to $dest/"
        echo "  plugin.wasm  $(du -sh "$dest/plugin.wasm" | cut -f1)"
        echo "  plugin.toml"
    fi
}

# ── Build ─────────────────────────────────────────────────────────────────────
built=0
for short_name in "${!PLUGINS[@]}"; do
    crate_name="${PLUGINS[$short_name]}"
    if [[ -z "$FILTER" || "$FILTER" == "$short_name" || "$FILTER" == "$crate_name" ]]; then
        build_plugin "$short_name" "$crate_name"
        ((built++)) || true
    fi
done

if [[ "$built" -eq 0 ]]; then
    echo "✗ No plugin matched '${FILTER}'. Run '$0 --help' for available plugins." >&2
    exit 1
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Done — $built plugin(s) built${INSTALL:+ and installed to $PLUGIN_DIR}"
echo ""

if [[ "$INSTALL" == "true" ]]; then
    echo "Next steps:"
    echo "  1. Set your API keys:"
    echo "     export PROWLARR_API_KEY=<from Prowlarr → Settings → General>"
    echo "     export PROWLARR_URL=http://localhost:9696"
    echo "     export OS_API_KEY=<from opensubtitles.com/en/consumers>"
    echo "     export OS_USERNAME=<your username>   # for download quota"
    echo "     export OS_PASSWORD=<your password>"
    echo ""
    echo "  2. Start aria2c (for download/stream):"
    echo "     aria2c --enable-rpc --rpc-secret=mystui --daemon"
    echo "     export ARIA2_SECRET=mystui"
    echo ""
    echo "  3. Launch stui:"
    echo "     ./dist/stui"
fi
