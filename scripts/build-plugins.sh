#!/usr/bin/env bash
# scripts/build-plugins.sh — compile all stui WASM plugins and install them.
#
# Usage:
#   ./scripts/build-plugins.sh                  # build all, install to ~/.stui/plugins/
#   ./scripts/build-plugins.sh kitsunekko        # build only kitsunekko
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
            echo "  anilist           Anime/manga metadata (AniList)"
            echo "  discogs           Music metadata (Discogs)"
            echo "  imdb              Movie/TV metadata (IMDB)"
            echo "  javdb             Japanese adult video metadata"
            echo "  kitsu             Anime metadata (Kitsu)"
            echo "  lastfm            Music scrobbling metadata (Last.fm)"
            echo "  listenbrainz      Music listen metadata (ListenBrainz)"
            echo "  omdb              Movie/TV metadata (OMDb)"
            echo "  r18               Japanese adult video metadata (R18)"
            echo "  tmdb              Movie/TV metadata (TMDB)"
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
    ["anilist"]="anilist-provider"
    ["discogs"]="discogs-provider"
    ["imdb"]="imdb-provider"
    ["javdb"]="javdb"
    ["kitsu"]="kitsu"
    ["lastfm"]="lastfm-provider"
    ["listenbrainz"]="listenbrainz-provider"
    ["omdb"]="omdb-provider"
    ["r18"]="r18"
    ["tmdb"]="tmdb-provider"
)

build_plugin() {
    local short_name="$1"
    local crate_name="$2"
    local plugin_dir="$ROOT/plugins/$crate_name"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "▶ Building $crate_name → $TARGET"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Build from workspace root so output lands in $ROOT/target/ (not per-crate target/).
    # Try nightly -Z flags first; fall back to plain stable build.
    (
        cd "$ROOT"
        cargo build \
            --release \
            --target "$TARGET" \
            -p "$crate_name" \
            -Z build-std=std,panic_abort \
            -Z build-std-features=panic_immediate_abort \
            2>/dev/null || \
        cargo build \
            --release \
            --target "$TARGET" \
            -p "$crate_name"
    )

    # Workspace builds output to $ROOT/target/, not the plugin's own target/.
    # Cargo converts hyphens to underscores in the filename.
    local wasm_name="${crate_name//-/_}.wasm"
    local wasm_file="$ROOT/target/$TARGET/release/$wasm_name"

    if [[ ! -f "$wasm_file" ]]; then
        echo "✗ Build succeeded but $wasm_name not found in target/$TARGET/release/" >&2
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
if [[ "$INSTALL" == "true" ]]; then
    echo "✓ Done — $built plugin(s) built and installed to $PLUGIN_DIR"
else
    echo "✓ Done — $built plugin(s) built (not installed)"
fi
echo ""

if [[ "$INSTALL" == "true" ]]; then
    echo "Next steps:"
    echo "  1. Start aria2c (for download/stream):"
    echo "     aria2c --enable-rpc --rpc-secret=mystui --daemon"
    echo "     export ARIA2_SECRET=mystui"
    echo ""
    echo "  2. Launch stui:"
    echo "     ./dist/stui"
fi
