# Pipeline Trace — Design Spec

## Goal

Provide a structured, human-readable decision trace that answers "Why did STUI do that?" — emitted live to stderr as each pipeline stage completes, activated by a `-v` / `--debug` flag or `STUI_TRACE=1` env var.

## Background

STUI already has a `-v` flag that sets `log.LevelDebug` in the Go TUI, producing raw `slog` lines. This gives volume but no structured summary. The missing piece is a per-stage trace that shows:

```
[trace] search: 3 providers (120ms)
[trace] resolve: 12 streams
[trace] bench: 8 tested
[trace] rank: picked #4 (score 0.82)
[trace] fallback: triggered (timeout)
```

## Design

### New file: `runtime/src/engine/trace.rs`

A single `TraceEmitter` struct. Disabled by default; enabled via `enable()`. All emit helpers are no-ops when disabled, adding zero overhead on the hot path.

```rust
pub struct TraceEmitter {
    enabled: AtomicBool,
    // Mutex required for interior mutability: Write::write takes &mut self.
    // Box<dyn Write + Send> is the agreed writer type; Mutex provides the
    // shared-reference access needed by &self emit helpers.
    writer: Mutex<Box<dyn Write + Send>>,
}

impl TraceEmitter {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            writer: Mutex::new(Box::new(std::io::stderr())),
        }
    }

    /// Used in tests to capture output.
    pub fn with_writer(w: Box<dyn Write + Send>) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            writer: Mutex::new(w),
        }
    }

    pub fn enable(&self) { self.enabled.store(true, Ordering::Relaxed); }
    pub fn is_enabled(&self) -> bool { self.enabled.load(Ordering::Relaxed) }

    fn emit(&self, line: &str) {
        if !self.is_enabled() { return; }
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "[trace] {}", line);
        }
    }

    // ── Stage helpers ──────────────────────────────────────────────────────

    pub fn search(&self, n_providers: usize, elapsed_ms: u64) {
        self.emit(&format!("search: {} providers ({}ms)", n_providers, elapsed_ms));
    }

    pub fn resolve(&self, n_streams: usize) {
        self.emit(&format!("resolve: {} streams", n_streams));
    }

    pub fn bench(&self, n_tested: usize) {
        self.emit(&format!("bench: {} tested", n_tested));
    }

    pub fn rank(&self, position: usize, score: f64) {
        self.emit(&format!("rank: picked #{} (score {:.2})", position, score));
    }

    pub fn fallback(&self, reason: &str) {
        self.emit(&format!("fallback: {}", reason));
    }

    pub fn provider_error(&self, name: &str, reason: &str) {
        self.emit(&format!("provider: {} failed ({})", name, reason));
    }
}
```

### `Pipeline` changes (`runtime/src/engine/pipeline.rs`)

`TraceEmitter` is wrapped in `Arc` so the IPC handler can activate it without `&mut Pipeline`:

```rust
pub trace: Arc<TraceEmitter>,
```

Constructed in `Pipeline::new()`:

```rust
let trace = Arc::new(TraceEmitter::new());
if std::env::var("STUI_TRACE").is_ok() {
    trace.enable();
}
```

### Emit points in `resolve_streams_with_benchmark`

Five emit calls, each fired as its stage completes:

| Call | Fired after |
|---|---|
| `self.trace.search(n, elapsed_ms)` | `catalog.search()` returns |
| `self.trace.resolve(n)` | provider results flattened to stream list |
| `self.trace.bench(n)` | `bench.probe_all()` returns |
| `self.trace.rank(pos, score)` | `rank_with_health_and_speed()` returns with ≥1 stream |
| `self.trace.fallback("no streams after bench")` | `rank_with_health_and_speed()` returns 0 streams |

Additional emit points at existing error branches:

| Call | Fired when |
|---|---|
| `self.trace.provider_error(name, reason)` | a provider returns an error during search/resolve |
| `self.trace.fallback(&format!("circuit open: {}", provider_name))` | `CircuitBreaker` rejects a provider; `provider_name` is the `&str` name passed to the circuit breaker call |
| `self.trace.fallback("timeout")` | a timeout error propagates to the top of resolve |

### Activation

Two paths, both call `pipeline.trace.enable()`:

1. **IPC message** — new `SetTrace { enabled: bool }` variant in the command enum. Go TUI sends this immediately after the handshake when `-v` is passed.
2. **Env var** — `STUI_TRACE=1` checked at `Pipeline::new()` startup via `std::env::var("STUI_TRACE")`.

### Output format

Each line is prefixed `[trace] ` to distinguish it from application stderr noise. Lines are emitted live as each stage completes (not buffered). Short reason strings (`"timeout"`, `"http 503"`, `"all filtered"`) are used — not full error chains.

## Data Flow

```
resolve_streams_with_benchmark()
    │
    ├─ catalog.search()
    │       └─ trace.search(n_providers, elapsed_ms)   ← live emit
    │
    ├─ flatten results
    │       └─ trace.resolve(n_streams)                ← live emit
    │
    ├─ bench.probe_all()
    │       └─ trace.bench(n_tested)                   ← live emit
    │
    ├─ rank_with_health_and_speed()
    │       ├─ if ≥1 stream: trace.rank(pos, score)    ← live emit
    │       └─ if 0 streams: trace.fallback("no streams after bench")
    │
    └─ error branches
            ├─ trace.provider_error(name, reason)    ← provider search/resolve error
            ├─ trace.fallback("circuit open: {name}") ← circuit breaker rejects provider
            └─ trace.fallback("timeout")              ← top-level timeout
```

## Testing

### Unit tests (`trace.rs`)

- **disabled by default** — calling all helpers on a new (non-enabled) emitter produces no output and does not panic
- **enabled flag** — `enable()` → `is_enabled()` returns true
- **format correctness** — enable, call each helper with known args via `with_writer`, assert line matches expected format

### Integration tests (`pipeline.rs`)

- **happy path trace** — stub catalog with 2 providers / 3 streams, construct emitter via `with_writer` then call `.enable()`, call `resolve_streams_with_benchmark`, assert output contains `search: 2 providers`, `resolve: 3 streams`, `bench:`, `rank:`
- **fallback on zero streams** — stub returns empty list after bench, assert `fallback: no streams after bench`
- **disabled by default** — no `enable()` call, assert output buffer is empty after a full resolve
- **provider error** — stub one provider to return an error, assert `provider: <name> failed (...)` appears in output
- **circuit open** — stub circuit breaker to reject a provider, assert `fallback: circuit open: <name>` appears
- **timeout fallback** — stub resolve to hit timeout path, assert `fallback: timeout` appears

## Files Touched

| File | Change |
|---|---|
| `runtime/src/engine/trace.rs` | **New** — `TraceEmitter` struct |
| `runtime/src/engine/mod.rs` | Export `trace` module |
| `runtime/src/engine/pipeline.rs` | Add `trace: Arc<TraceEmitter>` field; emit at 5+ pipeline points |
| `runtime/src/ipc/` | Add `SetTrace { enabled: bool }` command variant |
| `tui/cmd/stui/main.go` | Send `SetTrace` IPC message when `-v` flag is set |

## Configuration

Not wired into `stui.toml`. Activation is runtime-only via flag or env var.
