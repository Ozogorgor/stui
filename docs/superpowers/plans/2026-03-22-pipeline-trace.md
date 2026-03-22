# Pipeline Trace Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a structured live trace to stderr that shows search → resolve → bench → rank decisions and failures, activated by `STUI_TRACE=1` env var or an IPC `set_trace` message from the Go TUI when `-v` is passed.

**Architecture:** A new `TraceEmitter` struct in `runtime/src/engine/trace.rs` holds an `AtomicBool` enabled flag and a `Mutex<Box<dyn Write + Send>>` writer (stderr in production, injected buffer in tests). It is constructed as `Arc<TraceEmitter>` in `main.rs`, passed to `handle_line()` alongside existing subsystems, forwarded to `pipeline/search.rs` and `pipeline/resolve.rs` where it emits live `[trace] stage: detail` lines. A new `Request::SetTrace` IPC variant lets the Go TUI flip the flag at runtime; the `STUI_TRACE=1` env var activates it at startup.

**Tech Stack:** Rust (`std::sync::{atomic::AtomicBool, Mutex}`, `std::io::Write`), Go (IPC client `requests.go`), Newline-delimited JSON IPC.

---

## Chunk 1: TraceEmitter struct + unit tests

### Task 1: Create `trace.rs` with unit tests

**Files:**
- Create: `runtime/src/engine/trace.rs`
- Modify: `runtime/src/engine/mod.rs`

- [ ] **Step 1: Write the failing tests first**

Create `runtime/src/engine/trace.rs` with the stub struct and full test module:

```rust
//! Live-to-stderr structured pipeline trace.
//!
//! `TraceEmitter` writes `[trace] stage: detail` lines to stderr (or an
//! injected writer in tests) as each pipeline stage completes.
//! All methods are no-ops when the emitter is disabled.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct TraceEmitter {
    enabled: AtomicBool,
    // Mutex required: Write::write takes &mut self; Mutex gives interior mutability.
    writer: Mutex<Box<dyn Write + Send>>,
}

impl TraceEmitter {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            writer: Mutex::new(Box::new(std::io::stderr())),
        }
    }

    /// Construct with an injected writer — used in tests to capture output.
    /// Starts disabled; caller must call `.enable()` separately.
    pub fn with_writer(w: Box<dyn Write + Send>) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            writer: Mutex::new(w),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn emit(&self, line: &str) {
        if !self.is_enabled() {
            return;
        }
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "[trace] {}", line);
        }
    }

    // ── Stage helpers ──────────────────────────────────────────────────────

    pub fn search(&self, n_providers: usize, elapsed_ms: u64) {
        todo!()
    }

    pub fn resolve(&self, n_streams: usize) {
        todo!()
    }

    pub fn bench(&self, n_tested: usize) {
        todo!()
    }

    pub fn rank(&self, position: usize, score: f64) {
        todo!()
    }

    pub fn fallback(&self, reason: &str) {
        todo!()
    }

    pub fn provider_error(&self, name: &str, reason: &str) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buf_emitter() -> (TraceEmitter, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let buf_clone = std::sync::Arc::clone(&buf);
        struct BufWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for BufWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let emitter = TraceEmitter::with_writer(Box::new(BufWriter(buf_clone)));
        (emitter, buf)
    }

    fn read_buf(buf: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn disabled_by_default_no_output() {
        let (emitter, buf) = make_buf_emitter();
        emitter.search(3, 100);
        emitter.resolve(12);
        emitter.bench(8);
        emitter.rank(1, 0.82);
        emitter.fallback("timeout");
        emitter.provider_error("prov", "http 503");
        assert_eq!(read_buf(&buf), "", "disabled emitter must produce no output");
    }

    #[test]
    fn enable_sets_flag() {
        let emitter = TraceEmitter::new();
        assert!(!emitter.is_enabled());
        emitter.enable();
        assert!(emitter.is_enabled());
    }

    #[test]
    fn search_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.search(3, 120);
        assert_eq!(read_buf(&buf).trim(), "[trace] search: 3 providers (120ms)");
    }

    #[test]
    fn resolve_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.resolve(12);
        assert_eq!(read_buf(&buf).trim(), "[trace] resolve: 12 streams");
    }

    #[test]
    fn bench_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.bench(8);
        assert_eq!(read_buf(&buf).trim(), "[trace] bench: 8 tested");
    }

    #[test]
    fn rank_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.rank(4, 0.82);
        assert_eq!(read_buf(&buf).trim(), "[trace] rank: picked #4 (score 0.82)");
    }

    #[test]
    fn fallback_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.fallback("timeout");
        assert_eq!(read_buf(&buf).trim(), "[trace] fallback: timeout");
    }

    #[test]
    fn provider_error_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.provider_error("yts", "http 503");
        assert_eq!(read_buf(&buf).trim(), "[trace] provider: yts failed (http 503)");
    }

    #[test]
    fn no_streams_fallback_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.fallback("no streams after bench");
        assert_eq!(read_buf(&buf).trim(), "[trace] fallback: no streams after bench");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail with `todo!()`**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo test engine::trace 2>&1 | head -40
```

Expected: panics on `todo!()` for all stage helper tests. `disabled_by_default_no_output` and `enable_sets_flag` should pass (they don't call stage helpers on enabled emitters).

- [ ] **Step 3: Implement the stage helpers**

Replace all `todo!()` stubs:

```rust
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
```

- [ ] **Step 4: Export from `engine/mod.rs`**

In `runtime/src/engine/mod.rs`, add after the existing `pub mod pipeline;` line:

```rust
pub mod trace;
#[allow(unused_imports)]
pub use trace::TraceEmitter;
```

- [ ] **Step 5: Run all unit tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo test engine::trace -- --nocapture
```

Expected: all 8 tests pass, zero failures.

- [ ] **Step 6: Verify it compiles**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo check 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui
git add runtime/src/engine/trace.rs runtime/src/engine/mod.rs
git commit -m "feat: add TraceEmitter with unit tests"
```

---

## Chunk 2: IPC wiring + pipeline emit points

### Task 2: Add `SetTrace` IPC request + Rust handler

**Files:**
- Modify: `runtime/src/ipc/v1/mod.rs` (add `SetTrace` variant)
- Modify: `runtime/src/ipc/mod.rs` (re-export new type)
- Modify: `runtime/src/main.rs` (create `Arc<TraceEmitter>`, pass to `handle_line`, handle `Request::SetTrace`)

- [ ] **Step 1: Add `SetTrace` variant to `Request` enum**

In `runtime/src/ipc/v1/mod.rs`, add after `Request::SetStreamPolicy`:

```rust
/// Enable or disable the pipeline trace (stderr output for debugging).
/// Sent by the TUI when `-v` / `--debug` is passed.
SetTrace {
    enabled: bool,
},
```

- [ ] **Step 2: Re-export from `ipc/mod.rs`**

In `runtime/src/ipc/mod.rs`, the `SetTrace` variant is inline on the `Request` enum so no separate struct needs exporting. Verify the `Request` enum is already re-exported:

```bash
grep "Request," /home/ozogorgor/Projects/Stui_Project/stui/runtime/src/ipc/mod.rs
```

Expected: `Request,` is present in the re-export list.

- [ ] **Step 3: Create `Arc<TraceEmitter>` in `main.rs` and activate from env var**

In `runtime/src/main.rs`, find the import block and add:

```rust
use crate::engine::TraceEmitter;
```

After `let bench = StreamBenchmarker::new();` (around line 161), add:

```rust
let trace = {
    use std::sync::Arc;
    let t = Arc::new(TraceEmitter::new());
    if std::env::var("STUI_TRACE").is_ok() {
        t.enable();
    }
    t
};
```

- [ ] **Step 4: Pass `trace` into `handle_line()`**

Find `handle_line` function signature (around line 561). Add `trace: &std::sync::Arc<TraceEmitter>` as a parameter after `bench`:

```rust
async fn handle_line(
    engine: &Arc<Engine>,
    catalog: &Arc<Catalog>,
    health: &Arc<HealthRegistry>,
    config: &Arc<ConfigManager>,
    player: &player::PlayerBridge,
    mpd: Option<&MpdBridge>,
    watch_history: &Arc<watchhistory::WatchHistoryStore>,
    media_cache: &Arc<mediacache::MediaCacheStore>,
    bench: &StreamBenchmarker,
    trace: &std::sync::Arc<TraceEmitter>,
    line: &str,
) -> Response {
```

Update every `handle_line(...)` call site in `main.rs` to pass `&trace` as the new argument (search for `handle_line(` — there should be 2-3 call sites).

- [ ] **Step 5: Handle `Request::SetTrace` in `handle_line()`**

In the `match request { ... }` block, add before `Request::Shutdown`:

```rust
Request::SetTrace { enabled } => {
    if enabled {
        trace.enable();
    }
    Response::Ok
}
```

- [ ] **Step 6: Verify it compiles**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo check 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui
git add runtime/src/ipc/v1/mod.rs runtime/src/ipc/mod.rs runtime/src/main.rs
git commit -m "feat: add SetTrace IPC request and TraceEmitter wiring in main"
```

---

### Task 3: Emit trace points in `pipeline/search.rs` and `pipeline/resolve.rs`

**Files:**
- Modify: `runtime/src/pipeline/search.rs`
- Modify: `runtime/src/pipeline/resolve.rs`
- Modify: `runtime/src/main.rs` (pass `trace` to both pipeline functions)

- [ ] **Step 1: Add `trace` parameter to `run_search` and emit the search trace**

In `runtime/src/pipeline/search.rs`, update the function signature to accept `trace`:

```rust
use std::sync::Arc;
use crate::engine::TraceEmitter;
```

Change `run_search` signature:

```rust
pub async fn run_search(
    engine: &Arc<Engine>,
    catalog: &Arc<Catalog>,
    trace: &Arc<TraceEmitter>,
    r: SearchRequest,
) -> Response {
```

Add timing and emit. Replace the existing body:

```rust
pub async fn run_search(
    engine: &Arc<Engine>,
    catalog: &Arc<Catalog>,
    trace: &Arc<TraceEmitter>,
    r: SearchRequest,
) -> Response {
    let t0 = std::time::Instant::now();

    let results = engine.search(
        &r.id,
        &r.query,
        &r.tab,
        r.provider.as_deref(),
        r.limit.unwrap_or(50),
        r.offset.unwrap_or(0),
    ).await;

    let elapsed_ms = t0.elapsed().as_millis() as u64;

    if let Response::SearchResult(ref sr) = results {
        if sr.items.is_empty() {
            let fallback = catalog_search(catalog, &r.id, &r.query, &r.tab).await;
            if let Response::SearchResult(ref fr) = fallback {
                // Count providers queried: use 0 as "catalog fallback" signal
                trace.search(0, elapsed_ms);
                trace.resolve(fr.items.len());
            }
            return fallback;
        }
        // Count the number of distinct providers that returned results
        let n_providers = {
            use std::collections::HashSet;
            sr.items.iter().map(|e| e.provider.as_str()).collect::<HashSet<_>>().len()
        };
        trace.search(n_providers, elapsed_ms);
        trace.resolve(sr.items.len());
    } else {
        trace.fallback("search error");
    }

    results
}
```

- [ ] **Step 2: Add `trace` parameter to `run_get_streams` and emit bench/rank/fallback**

In `runtime/src/pipeline/resolve.rs`, add to imports:

```rust
use crate::engine::TraceEmitter;
```

Change `run_get_streams` signature:

```rust
pub async fn run_get_streams(
    engine: &Arc<Engine>,
    _catalog: &Arc<Catalog>,
    config: &Arc<ConfigManager>,
    health: &Arc<HealthRegistry>,
    bench: &StreamBenchmarker,
    trace: &Arc<TraceEmitter>,
    r: GetStreamsRequest,
) -> Response {
```

Add emit calls inside the function body. After `for provider in providers { ... }` loop (after all streams are collected, around where `if all_streams.is_empty()` check is), add:

```rust
    // Emit per-provider errors; detect timeout errors separately
    for err in &errors {
        // err format is "provider_name: error_message"
        if let Some((name, msg)) = err.split_once(": ") {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("timeout") || msg_lower.contains("timed out") {
                trace.fallback("timeout");
            } else {
                trace.provider_error(name, msg);
            }
        }
    }

    if all_streams.is_empty() {
        trace.fallback("no streams after resolve");
        return Response::StreamsResult(StreamsResponse {
            id: r.id,
            entry_id: r.entry_id,
            streams: vec![],
        });
    }
```

After `bench.probe_all`:

```rust
    if benchmark_enabled {
        let t_bench = std::time::Instant::now();
        all_streams = bench.probe_all(&all_streams).await;
        let _ = t_bench; // elapsed available if needed
        trace.bench(all_streams.len());
    }
```

Before the final `Response::StreamsResult(...)` return, after ranking:

```rust
    if streams.is_empty() {
        trace.fallback("no streams after bench");
    } else {
        // streams[0] is the best-ranked; position = 1 (1-indexed)
        let best_score = candidates.first().map(|c| c.score.total() as f64).unwrap_or(0.0);
        trace.rank(1, best_score / 100.0); // normalise to 0..1 range
    }
```

Full updated function with all emit points:

```rust
pub async fn run_get_streams(
    engine: &Arc<Engine>,
    _catalog: &Arc<Catalog>,
    config: &Arc<ConfigManager>,
    health: &Arc<HealthRegistry>,
    bench: &StreamBenchmarker,
    trace: &Arc<TraceEmitter>,
    r: GetStreamsRequest,
) -> Response {
    let cfg = config.snapshot().await;
    let benchmark_enabled = cfg.streaming.benchmark_streams;
    let health_map = health.all_reliability_scores();

    let reg = engine.registry().read().await;
    let providers = reg.find_stream_providers();

    let mut all_streams: Vec<crate::providers::Stream> = vec![];
    let mut errors = vec![];

    for provider in providers {
        match engine.resolve_raw(&r.entry_id, &provider.manifest.plugin.name).await {
            Ok(result) => {
                let quality_label = result.quality.clone().unwrap_or_else(|| "Unknown".to_string());
                let stream = crate::providers::Stream {
                    id: result.stream_url.clone(),
                    name: quality_label.clone(),
                    url: result.stream_url,
                    mime: None,
                    quality: crate::providers::StreamQuality::from_label(&quality_label),
                    provider: provider.manifest.plugin.name.clone(),
                    protocol: Some("https".to_string()),
                    seeders: None,
                    bitrate_kbps: None,
                    codec: None,
                    resolution: None,
                    hdr: crate::providers::HdrFormat::None,
                    size_bytes: None,
                    latency_ms: None,
                    speed_mbps: None,
                    audio_channels: None,
                    language: None,
                };
                all_streams.push(stream);
            }
            Err(e) => {
                let provider_name = provider.manifest.plugin.name.clone();
                errors.push(format!("{}: {}", provider_name, e));
            }
        }
    }
    drop(reg);

    // Emit per-provider errors (and detect timeout errors)
    for err in &errors {
        if let Some((name, msg)) = err.split_once(": ") {
            // Emit a timeout fallback if the error message indicates a timeout
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("timeout") || msg_lower.contains("timed out") {
                trace.fallback("timeout");
            } else {
                trace.provider_error(name, msg);
            }
        }
    }

    if all_streams.is_empty() {
        trace.fallback("no streams after resolve");
        return Response::StreamsResult(StreamsResponse {
            id: r.id,
            entry_id: r.entry_id,
            streams: vec![],
        });
    }

    // Apply benchmarking if enabled
    if benchmark_enabled {
        all_streams = bench.probe_all(&all_streams).await;
        trace.bench(all_streams.len());
    }

    // Apply health-based re-ranking
    let candidates = if !health_map.is_empty() {
        use crate::quality::rank_with_health;
        rank_with_health(all_streams.clone(), &crate::quality::RankingPolicy::default(), Some(&health_map))
    } else {
        use crate::quality::rank;
        rank(all_streams.clone(), &crate::quality::RankingPolicy::default())
    };

    // Apply speed-based re-ranking if benchmarking enabled
    let candidates = if benchmark_enabled {
        use crate::quality::rank_with_health_and_speed;
        let mut speed_map: HashMap<String, f64> = HashMap::new();
        for stream in &all_streams {
            if let Some(speed) = stream.speed_mbps {
                speed_map.insert(stream.url.clone(), speed);
            }
        }
        if !speed_map.is_empty() {
            rank_with_health_and_speed(
                all_streams,
                &crate::quality::RankingPolicy::default(),
                if health_map.is_empty() { None } else { Some(&health_map) },
                Some(&speed_map),
            )
        } else {
            candidates
        }
    } else {
        candidates
    };

    let streams: Vec<StreamInfoWire> = candidates
        .iter()
        .map(|c| stream_to_wire(c.stream.clone(), c.score.total()))
        .collect();

    if streams.is_empty() {
        trace.fallback("no streams after bench");
    } else {
        let best_score = candidates.first().map(|c| c.score.total() as f64 / 100.0).unwrap_or(0.0);
        trace.rank(1, best_score);
    }

    Response::StreamsResult(StreamsResponse {
        id: r.id,
        entry_id: r.entry_id,
        streams,
    })
}
```

- [ ] **Step 3: Update the two call sites in `main.rs`**

Find the two lines in `handle_line` that call `run_search` and `run_get_streams`:

```rust
// Old:
Request::Search(r)    => pipeline::search::run_search(engine, catalog, r).await,
Request::GetStreams(r) => pipeline::resolve::run_get_streams(engine, catalog, config, health, bench, r).await,

// New:
Request::Search(r)    => pipeline::search::run_search(engine, catalog, trace, r).await,
Request::GetStreams(r) => pipeline::resolve::run_get_streams(engine, catalog, config, health, bench, trace, r).await,
```

- [ ] **Step 4: Verify it compiles**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo build 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 5: Manual smoke test with env var**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
STUI_TRACE=1 cargo run 2>&1 &
# send a set_trace + get_streams request via stdin:
echo '{"type":"ping"}' | STUI_TRACE=1 cargo run 2>&1 | head -5
```

Expected: stderr shows `[trace] ...` lines for any search/resolve calls made.

- [ ] **Step 6: Run all runtime tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui
git add runtime/src/pipeline/search.rs runtime/src/pipeline/resolve.rs runtime/src/main.rs
git commit -m "feat: emit pipeline trace points in search and resolve handlers"
```

---

## Chunk 3: Go TUI — send SetTrace on `-v`

### Task 4: Add `SetTrace` method to Go IPC client and call it from `ui.go`

**Files:**
- Modify: `tui/internal/ipc/requests.go` (add `SetTrace` method)
- Modify: `tui/internal/ui/ui.go` (add `Verbose bool` to `Options`, call `SetTrace` after handshake)
- Modify: `tui/cmd/stui/main.go` (pass `Verbose` in `opts`)

- [ ] **Step 1: Add `SetTrace` method to `requests.go`**

In `tui/internal/ipc/requests.go`, add at the end of the file:

```go
// SetTrace enables or disables the runtime's pipeline trace output (stderr).
// Call immediately after the handshake when -v is passed.
func (c *Client) SetTrace(enabled bool) {
    go func() {
        id := c.nextID()
        ch := c.sendWithID(id, map[string]any{
            "type":    "set_trace",
            "enabled": enabled,
        })
        <-ch // wait for Ok response; ignore it
    }()
}
```

- [ ] **Step 2: Add `Verbose bool` to `ui.Options`**

In `tui/internal/ui/ui.go`, find the `Options` struct (around line 38):

```go
type Options struct {
    RuntimePath string
    NoRuntime   bool
}
```

Add `Verbose bool`:

```go
type Options struct {
    RuntimePath string
    NoRuntime   bool
    Verbose     bool
}
```

- [ ] **Step 3: Call `SetTrace` after handshake in `ui.go`**

In `ui.go`, find the `runtimeStartedMsg` case in `Update()` (around line 292):

```go
case runtimeStartedMsg:
    m.client = msg.client
    m.state.RuntimeStatus = state.RuntimeReady
    m.state.StatusMsg = "Loading catalog\u2026"
    m.state.RuntimeVersion = msg.client.RuntimeVersion
    m.client.ListPlugins()
```

Add `SetTrace` call right after setting `m.client`:

```go
case runtimeStartedMsg:
    m.client = msg.client
    m.state.RuntimeStatus = state.RuntimeReady
    m.state.StatusMsg = "Loading catalog\u2026"
    m.state.RuntimeVersion = msg.client.RuntimeVersion
    if m.opts.Verbose {
        m.client.SetTrace(true)
    }
    m.client.ListPlugins()
```

- [ ] **Step 4: Pass `Verbose` from `main.go`**

In `tui/cmd/stui/main.go`, find the `opts` struct literal:

```go
opts := ui.Options{
    RuntimePath: *runtimePath,
    NoRuntime:   *noRuntime,
}
```

Add `Verbose`:

```go
opts := ui.Options{
    RuntimePath: *runtimePath,
    NoRuntime:   *noRuntime,
    Verbose:     *verbose,
}
```

- [ ] **Step 5: Build the Go TUI**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui
go build ./... 2>&1
```

Expected: no errors.

- [ ] **Step 6: Run Go tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui
go test ./... 2>&1
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui
git add tui/internal/ipc/requests.go tui/internal/ui/ui.go tui/cmd/stui/main.go
git commit -m "feat: send set_trace IPC message when -v flag is passed"
```
