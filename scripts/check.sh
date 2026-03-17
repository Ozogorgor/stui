#!/usr/bin/env bash
# scripts/check.sh — lint and format checks for the full stui codebase.
#
# Runs without modifying files by default. Pass --fix to apply auto-fixes.
#
# Usage:
#   ./scripts/check.sh           # check only (exits non-zero on any failure)
#   ./scripts/check.sh --fix     # auto-fix formatting issues where possible
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX=false
FAILED=()

for arg in "$@"; do
    case "$arg" in
        --fix)    FIX=true ;;
        --help|-h)
            echo "Usage: $0 [--fix]"
            echo ""
            echo "  --fix   Auto-apply formatting fixes (cargo fmt, gofmt -w)"
            exit 0
            ;;
    esac
done

# ── Helpers ───────────────────────────────────────────────────────────────────

sep() { echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"; }
ok()  { echo "✓  $1"; }
fail(){ echo "✗  $1"; FAILED+=("$1"); }
step(){ sep; echo "▶ $1"; sep; }

run() {
    local label="$1"; shift
    if "$@"; then
        ok "$label"
    else
        fail "$label"
    fi
}

# ── Rust ──────────────────────────────────────────────────────────────────────

step "Rust — runtime"
cd "$ROOT/runtime"

if [[ "$FIX" == "true" ]]; then
    run "cargo fmt" cargo fmt
else
    run "cargo fmt --check" cargo fmt --check
fi

# Clippy: deny warnings on the runtime crate itself, allow them in deps.
# Exclude the abi/host.rs wasmtime bindings which have known upstream issues.
run "cargo clippy" cargo clippy \
    --all-targets \
    -- \
    -D warnings \
    -A clippy::module_name_repetitions \
    -A clippy::missing_errors_doc

echo ""

# ── Go ────────────────────────────────────────────────────────────────────────

step "Go — tui"
cd "$ROOT/tui"

if [[ "$FIX" == "true" ]]; then
    run "gofmt -w" gofmt -w .
else
    # gofmt -l lists files that differ; non-empty output = failure
    UNFORMATTED="$(gofmt -l .)"
    if [[ -n "$UNFORMATTED" ]]; then
        echo "Files need formatting:"
        echo "$UNFORMATTED" | sed 's/^/    /'
        fail "gofmt check"
    else
        ok "gofmt check"
    fi
fi

run "go vet" go vet ./...

# staticcheck if available (optional but recommended)
if command -v staticcheck &>/dev/null; then
    run "staticcheck" staticcheck ./...
else
    echo "  (staticcheck not found — skipping; install with: go install honnef.co/go/tools/cmd/staticcheck@latest)"
fi

echo ""

# ── Summary ───────────────────────────────────────────────────────────────────

sep
if [[ ${#FAILED[@]} -eq 0 ]]; then
    echo "✓ All checks passed"
    echo ""
else
    echo "✗ ${#FAILED[@]} check(s) failed:"
    for f in "${FAILED[@]}"; do
        echo "    • $f"
    done
    echo ""
    if [[ "$FIX" == "false" ]]; then
        echo "  Run './scripts/check.sh --fix' to auto-fix formatting issues."
        echo ""
    fi
    exit 1
fi
