#!/usr/bin/env bash
# scripts/refresh-anime-bridge.sh — fetch the latest Fribb anime-lists snapshot
# and overwrite the committed bundled copy. Run periodically (or whenever you
# want a fresh dataset baked into the binary).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."
DEST="$ROOT/runtime/data/anime-bridge-snapshot.json.gz"

mkdir -p "$(dirname "$DEST")"
curl -fsSL https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json \
    | gzip -9 \
    > "$DEST"

echo "snapshot refreshed at $DEST ($(du -h "$DEST" | cut -f1))"
echo "commit if desired: git add runtime/data/anime-bridge-snapshot.json.gz"
