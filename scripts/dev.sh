#!/usr/bin/env bash
# Run the TUI in UI-only dev mode (no Rust runtime needed)
set -e
cd "$(dirname "$0")/../tui"
go run ./cmd/stui --no-runtime "$@"
