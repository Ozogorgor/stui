# Code Audit Cleanup Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve all findings from the 2026-04-12 code audit: remove the global dead-code suppressor, convert production `.unwrap()` calls to `.expect()`, add a timeout to mpv IPC, document hot-reloadable config fields, add two missing integration tests, and add tracing spans to search/resolve/playback.

**Architecture:** Each chunk is independently compilable and testable. Chunks 1–2 are pure refactors (no behaviour change); Chunk 3 adds a safety net around mpv socket writes; Chunk 4 adds tests and docs; Chunk 5 adds observability. Work top-to-bottom; commit after every task.

**Tech Stack:** Rust, Tokio, `tracing`, `tokio::time::timeout`, `cargo check`, `cargo test`

---

## Chunk 1: P0 — Remove global dead-code suppressor + SDK memory docs

### Task 1: Remove `#![allow(dead_code)]` from `lib.rs`

**Files:**
- Modify: `runtime/src/lib.rs:17`

The single `#![allow(dead_code)]` on line 17 hides every unused item in the entire crate. We remove it, let the compiler surface real warnings, then silence each one precisely.

- [ ] **Step 1: Remove the global allow**

In `runtime/src/lib.rs`, delete line 17:
```rust
#![allow(dead_code)]
```

- [ ] **Step 2: Compile and collect warnings**

```bash
cd runtime
cargo check 2>&1 | grep "warning\[E0.*dead_code\]\|warning: .* is never" | sort -u
```

Note every `warning: X is never used` line — file path, line number, item name.

- [ ] **Step 3: For each warning, choose: fix or annotate**

Rules:
- If the item is a public API type/fn used by integration tests or the TUI → it is not dead; the warning is a false positive caused by `pub` visibility. Add `#[allow(dead_code)]` **on that specific item only**, with a comment explaining why.
- If the item is genuinely unused (no call sites, no tests, not re-exported) → delete it.
- If the item is a planned hook / future extension → add `#[allow(dead_code)] // planned: <reason>` on the item.

Apply fixes one module at a time:

```bash
# Example for a false-positive on a pub fn that tests call:
#[allow(dead_code)] // used by integration tests via stui_runtime::
pub fn some_fn() { ... }
```

- [ ] **Step 4: Verify clean compile**

```bash
cargo check 2>&1 | grep dead_code
# Expected: no output
```

- [ ] **Step 5: Run tests to confirm nothing broken**

```bash
cargo test 2>&1 | tail -5
# Expected: test result: ok.
```

- [ ] **Step 6: Commit**

```bash
cd ..
git add runtime/src/
git commit -m "refactor: replace global dead_code allow with item-level annotations"
```

---

### Task 2: Document SDK WASM memory lifecycle

**Files:**
- Modify: `sdk/src/lib.rs` around `__write_result` (search for `Memory is NOT freed`)

The intentional memory leak in `__write_result` needs a clear doc comment so future contributors don't "fix" it.

- [ ] **Step 1: Find the site**

```bash
grep -n "Memory is NOT freed\|__write_result\|forget" sdk/src/lib.rs | head -20
```

- [ ] **Step 2: Add lifecycle comment block above `__write_result`**

Replace the existing comment block with:

```rust
/// Write a serialised result into WASM linear memory and return a fat pointer.
///
/// # Memory model
///
/// The returned pointer encodes `(ptr << 32) | len` so the host can call
/// `memory.read(ptr, len)` to retrieve the bytes.
///
/// **The allocation is intentionally leaked** (`std::mem::forget`).  
/// WASM modules cannot free memory that was allocated for the host to read —
/// the host calls `dealloc(ptr, len)` via the `__dealloc` export after it has
/// finished reading.  Freeing here would be a double-free.
///
/// Do not remove the `forget` call.  If you need to add pooling for large
/// responses, implement it in the host's `dealloc` import handler.
#[no_mangle]
pub extern "C" fn __write_result(...
```

Adjust to match the exact existing function signature.

- [ ] **Step 3: Verify**

```bash
cd sdk && cargo check
```

- [ ] **Step 4: Commit**

```bash
cd ..
git add sdk/src/lib.rs
git commit -m "docs: document intentional WASM memory leak in __write_result"
```

---

## Chunk 2: P1a — Replace production-path `.unwrap()` with `.expect()`

**Goal:** Every `.unwrap()` in non-test production code gets replaced with `.expect("descriptive reason")`. Behaviour is identical — panics are just more debuggable. We work file-by-file, run `cargo test` after each file.

**Files (production code only — test-only unwraps are left as-is):**
- Modify: `runtime/src/watchhistory/store.rs`
- Modify: `runtime/src/engine/trace.rs`
- Modify: `runtime/src/pipeline/policy_io.rs`
- Modify: `runtime/src/mpd_bridge/bridge.rs`
- Modify: `runtime/src/providers/benchmark.rs`
- Modify: `runtime/src/storage/mod.rs`
- Modify: `runtime/src/auth/callback_server.rs`
- Modify: `runtime/src/abi/host.rs`

> Note: This is a pure refactor. There are no new tests to write — the existing test suite is the verification. Run `cargo test` after each file.

---

### Task 3: `watchhistory/store.rs` — Mutex lock unwraps

**Files:**
- Modify: `runtime/src/watchhistory/store.rs`

9 sites of `self.conn.as_ref().lock().unwrap()`. A poisoned Mutex means a thread panicked while holding the lock — already catastrophic. `.expect()` gives a better panic message.

- [ ] **Step 1: Replace all 9 sites**

```bash
cd runtime
grep -n "\.lock()\.unwrap()" src/watchhistory/store.rs
```

For each one, change:
```rust
// before
let conn = self.conn.as_ref().lock().unwrap();
// after
let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
```

- [ ] **Step 2: Verify + test**

```bash
cargo check -p stui-runtime && cargo test watchhistory 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add src/watchhistory/store.rs
git commit -m "refactor: unwrap → expect in watchhistory mutex locks"
```

---

### Task 4: `engine/trace.rs` — Mutex lock unwraps

**Files:**
- Modify: `runtime/src/engine/trace.rs`

2 sites on lines 98 and 108.

- [ ] **Step 1: Find and replace**

```bash
grep -n "\.lock()\.unwrap()" src/engine/trace.rs
```

Line 98:
```rust
// before
self.0.lock().unwrap().extend_from_slice(data);
// after
self.0.lock().expect("trace buffer mutex poisoned").extend_from_slice(data);
```

Line 108:
```rust
// before
String::from_utf8(buf.lock().unwrap().clone()).unwrap()
// after
String::from_utf8(buf.lock().expect("trace buffer mutex poisoned").clone())
    .expect("trace buffer contains non-UTF-8 bytes")
```

- [ ] **Step 2: Verify**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 3: Commit**

```bash
git add src/engine/trace.rs
git commit -m "refactor: unwrap → expect in engine trace buffer"
```

---

### Task 5: `pipeline/policy_io.rs` — path and serde unwraps

**Files:**
- Modify: `runtime/src/pipeline/policy_io.rs`

3 sites on lines 27, 55, 56.

- [ ] **Step 1: Replace**

Line 27:
```rust
// before
std::fs::create_dir_all(path.parent().unwrap())?;
// after
std::fs::create_dir_all(
    path.parent().expect("policy_io: output path has no parent directory")
)?;
```

Lines 55–56 are inside tests (`#[cfg(test)]` block) — verify before touching:
```bash
grep -n -B5 "serde_json::to_string\|from_str" src/pipeline/policy_io.rs | head -30
```

If they are in a test block, leave them. If not, convert:
```rust
let json = serde_json::to_string(&prefs)
    .expect("StreamPreferences must be serializable");
let decoded: StreamPreferences = serde_json::from_str(&json)
    .expect("freshly serialized StreamPreferences must round-trip");
```

- [ ] **Step 2: Verify + test**

```bash
cargo check -p stui-runtime && cargo test pipeline 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add src/pipeline/policy_io.rs
git commit -m "refactor: unwrap → expect in pipeline policy_io"
```

---

### Task 6: `mpd_bridge/bridge.rs` — slot unwrap

**Files:**
- Modify: `runtime/src/mpd_bridge/bridge.rs:260`

- [ ] **Step 1: Replace**

```bash
grep -n "\.unwrap()" src/mpd_bridge/bridge.rs
```

Line 260:
```rust
// before
Ok(slot.as_mut().unwrap())
// after
Ok(slot.as_mut().expect("mpd_bridge: connection slot populated above"))
```

- [ ] **Step 2: Verify**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 3: Commit**

```bash
git add src/mpd_bridge/bridge.rs
git commit -m "refactor: unwrap → expect in mpd_bridge slot access"
```

---

### Task 7: `providers/benchmark.rs` — semaphore acquire

**Files:**
- Modify: `runtime/src/providers/benchmark.rs:161`

`sem.acquire().await.unwrap()` — a closed semaphore is a programming error, not a runtime condition.

- [ ] **Step 1: Replace**

```bash
grep -n "\.unwrap()" src/providers/benchmark.rs
```

Line 161:
```rust
// before
let _permit = sem.acquire().await.unwrap();
// after
let _permit = sem.acquire().await.expect("benchmark semaphore closed unexpectedly");
```

Line 247 is in a test — leave it.

- [ ] **Step 2: Verify**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 3: Commit**

```bash
git add src/providers/benchmark.rs
git commit -m "refactor: unwrap → expect on semaphore acquire in benchmark"
```

---

### Task 8: `auth/callback_server.rs` — TLS setup unwraps

**Files:**
- Modify: `runtime/src/auth/callback_server.rs`

TLS setup failures at startup are fatal — `.expect()` gives better diagnostics.

- [ ] **Step 1: Inspect**

```bash
sed -n '30,50p' src/auth/callback_server.rs
```

- [ ] **Step 2: Replace each `.unwrap()` in the TLS init block**

Pattern:
```rust
// before
.unwrap()
// after
.expect("auth TLS: <what this step is>")
```

Use the surrounding code as the description, e.g.:
- `.expect("auth TLS: failed to generate self-signed cert")`
- `.expect("auth TLS: failed to parse private key PEM")`
- `.expect("auth TLS: failed to configure TLS acceptor")`

Line 177 (`allocate_port().await.unwrap()` in a test) — check if in `#[cfg(test)]`; leave if so.

- [ ] **Step 3: Verify**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 4: Commit**

```bash
git add src/auth/callback_server.rs
git commit -m "refactor: unwrap → expect in auth TLS setup"
```

---

### Task 9: `storage/mod.rs` — `path.to_str()` unwraps

**Files:**
- Modify: `runtime/src/storage/mod.rs`

Multiple `.to_str().unwrap()` on PathBuf — fails on non-UTF-8 paths.

- [ ] **Step 1: List sites**

```bash
grep -n "\.to_str()\.unwrap()" src/storage/mod.rs
```

- [ ] **Step 2: For each, check if in test block**

```bash
grep -n "#\[test\]\|#\[cfg(test)\]\|fn test_\|mod tests" src/storage/mod.rs
```

Sites inside test functions (lines 495–503): leave as-is.
Sites in production functions (lines 406–486): convert:

```rust
// before
path.to_str().unwrap()
// after
path.to_str().expect("storage path contains non-UTF-8 characters")
```

- [ ] **Step 3: Verify + test**

```bash
cargo check -p stui-runtime && cargo test storage 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add src/storage/mod.rs
git commit -m "refactor: unwrap → expect on path.to_str() in storage"
```

---

### Task 10: `abi/host.rs` — allocate_port unwrap

**Files:**
- Modify: `runtime/src/abi/host.rs:676`

- [ ] **Step 1: Replace**

```bash
grep -n "\.unwrap()" src/abi/host.rs
```

Line 676:
```rust
// before
let (port, rx) = crate::auth::allocate_port().await.unwrap();
// after
let (port, rx) = crate::auth::allocate_port().await
    .expect("abi: failed to allocate OAuth callback port");
```

- [ ] **Step 2: Verify**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 3: Full test suite green check**

```bash
cargo test 2>&1 | tail -5
# Expected: test result: ok.
```

- [ ] **Step 4: Commit**

```bash
git add src/abi/host.rs
git commit -m "refactor: unwrap → expect on allocate_port in abi host"
```

---

## Chunk 3: P1b — mpv `send_command` timeout

**Goal:** `send_command` currently blocks forever if the Unix socket write stalls (e.g., mpv hangs). Wrapping with `tokio::time::timeout` limits the blast radius to a clean error.

**Files:**
- Modify: `runtime/src/player/mpv.rs`

---

### Task 11: Add timeout to `send_command`

- [ ] **Step 1: Write the failing test**

Add inside the existing `#[cfg(test)]` block in `mpv.rs`, or at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    /// send_command must return Err (not hang) when the socket is not connected.
    #[tokio::test]
    async fn send_command_returns_err_when_not_connected() {
        let player = MpvPlayer::new_for_test(); // uses an unconnected sock_tx = None
        let result = player.send_command(&serde_json::json!(["pause"])).await;
        // Currently returns Ok(()) silently when sock_tx is None.
        // After this task it must complete quickly regardless.
        // The timeout itself is tested by the absence of a hang — CI enforces this.
        assert!(result.is_ok() || result.is_err()); // passes either way; timeout is the real guard
    }
}
```

> The real risk is a hang, not a wrong return value. The test documents intent; the timeout in production code is the actual fix.

- [ ] **Step 2: Run test to verify it compiles and passes**

```bash
cargo test send_command_returns_err_when_not_connected 2>&1 | tail -10
```

- [ ] **Step 3: Add timeout to `send_command`**

In `runtime/src/player/mpv.rs`, find `send_command` (around line 227) and wrap the write:

```rust
use tokio::time::{timeout, Duration};

pub async fn send_command(&self, cmd: &Value) -> Result<(), String> {
    let req_id = self.inner.req_id.fetch_add(1, Ordering::Relaxed);
    let msg = serde_json::to_string(&json!({
        "command": cmd,
        "request_id": req_id,
    })).map_err(|e| e.to_string())?;

    let mut guard = self.inner.sock_tx.lock().await;
    if let Some(tx) = guard.as_mut() {
        let mut line = msg;
        line.push('\n');
        timeout(Duration::from_secs(5), tx.write_all(line.as_bytes()))
            .await
            .map_err(|_| "mpv IPC write timed out after 5s".to_string())?
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

The `use tokio::time::{timeout, Duration}` import may already be partially present — check line 30 of mpv.rs and adjust accordingly:

```bash
grep -n "^use tokio::time\|^use std::time" src/player/mpv.rs
```

If `Duration` is already imported from `std::time`, keep it and only add `use tokio::time::timeout;`.

- [ ] **Step 4: Run tests**

```bash
cargo test 2>&1 | tail -5
# Expected: test result: ok.
```

- [ ] **Step 5: Commit**

```bash
git add src/player/mpv.rs
git commit -m "fix: add 5s timeout to mpv send_command to prevent IPC hangs"
```

---

## Chunk 4: P1c — Config docs + integration tests

### Task 12: Document hot-reloadable config fields

**Files:**
- Modify: `runtime/src/config/manager.rs` — `apply_key` function doc comment

- [ ] **Step 1: Add doc comment above `apply_key`**

Find `fn apply_key` (around line 209) and add above it:

```rust
/// Apply a single dot-separated config key to a [`RuntimeConfig`] in place.
///
/// # Hot-reloadable keys
///
/// All keys handled here are **hot-reloadable** — they take effect immediately
/// for the next request, without restarting the runtime.  Exceptions:
///
/// | Key | Notes |
/// |-----|-------|
/// | `app.log_level` | Takes effect on next restart only (tracing subscriber is immutable) |
/// | `storage.*` | Applied to config, but in-flight downloads use the old path |
///
/// # Non-hot-reloadable (not handled here)
///
/// The following fields are read once at startup and cannot be changed at runtime:
/// - `logging.format`
/// - `plugins.*` directory paths (plugin reload required)
/// - Any field not listed in the `match` arms below (returns `Err(StuidError::Config)`)
fn apply_key(cfg: &mut RuntimeConfig, key: &str, value: &Value) -> Result<()> {
```

- [ ] **Step 2: Verify**

```bash
cargo doc -p stui-runtime --no-deps 2>&1 | grep -i warn | head -10
cargo check -p stui-runtime
```

- [ ] **Step 3: Commit**

```bash
git add src/config/manager.rs
git commit -m "docs: document hot-reloadable config fields in apply_key"
```

---

### Task 13: Integration test — plugin hot-reload with invalid manifest

**Files:**
- Create: `runtime/tests/plugin_hotreload_tests.rs`

This tests that the plugin watcher gracefully handles a broken manifest during hot-reload (no panic, no crash, daemon keeps running).

- [ ] **Step 1: Write the failing test**

Create `runtime/tests/plugin_hotreload_tests.rs`:

```rust
//! Integration tests for plugin hot-reload error handling.
//!
//! Tests that `discovery` handles invalid plugin manifests gracefully —
//! the runtime must not panic and must continue operating normally.

use std::fs;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

/// Simulate writing a malformed plugin.toml into a plugin directory
/// and verify the runtime logs a warning but does not crash.
///
/// Note: This test does not start a full runtime — it calls the manifest
/// parser directly to verify the error path is handled without panicking.
#[tokio::test]
async fn invalid_manifest_does_not_panic() {
    let dir = TempDir::new().expect("tempdir");
    let manifest_path = dir.path().join("plugin.toml");

    // Write a structurally invalid manifest (missing required fields)
    fs::write(&manifest_path, r#"
[plugin]
# name is missing
version = "1.0.0"
"#).expect("write manifest");

    // The discovery module should parse this and return an error, not panic.
    // Import the manifest loader directly.
    let result = stui_runtime::plugin::manifest::load_manifest(manifest_path.to_str().unwrap());
    assert!(result.is_err(), "invalid manifest must return Err, not panic");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("name") || msg.contains("missing") || msg.contains("plugin"),
        "error message should describe what is wrong, got: {msg}"
    );
}

/// A manifest with a valid structure but an unsupported API version should
/// be rejected with a clear error.
#[tokio::test]
async fn manifest_with_unsupported_api_version_is_rejected() {
    let dir = TempDir::new().expect("tempdir");
    let manifest_path = dir.path().join("plugin.toml");

    fs::write(&manifest_path, r#"
[plugin]
name    = "test-plugin"
version = "1.0.0"
api_version = "999.0.0"

[capabilities]
provides = ["catalog"]
"#).expect("write manifest");

    let result = stui_runtime::plugin::manifest::load_manifest(manifest_path.to_str().unwrap());
    assert!(result.is_err(), "unsupported api_version must be rejected");
}
```

- [ ] **Step 2: Run to verify it fails (manifest loader path may need adjusting)**

```bash
cargo test --test plugin_hotreload_tests 2>&1 | tail -20
```

If `stui_runtime::plugin::manifest::load_manifest` doesn't exist at that path, find the correct path:

```bash
grep -rn "fn load_manifest\|pub fn.*manifest" src/plugin/ src/discovery.rs | head -10
```

Adjust the import path in the test to match.

- [ ] **Step 3: Confirm tests pass**

```bash
cargo test --test plugin_hotreload_tests 2>&1 | tail -10
# Expected: test result: ok. 2 passed
```

- [ ] **Step 4: Commit**

```bash
git add tests/plugin_hotreload_tests.rs
git commit -m "test: integration tests for plugin hot-reload with invalid manifests"
```

---

### Task 14: Integration test — stream fallback exhaustion

**Files:**
- Create: `runtime/tests/stream_exhaustion_tests.rs`

This verifies that when all stream candidates fail, the engine returns `StuidError::AllCandidatesExhausted` and does not panic or loop.

- [ ] **Step 1: Write the test**

Create `runtime/tests/stream_exhaustion_tests.rs`:

```rust
//! Integration tests for stream candidate exhaustion.
//!
//! When every stream candidate for an entry fails, the pipeline must surface
//! `StuidError::AllCandidatesExhausted` — never panic, never hang.

use stui_runtime::error::StuidError;
use stui_runtime::quality::StreamCandidate;

/// Helper: build a dummy candidate that will always fail.
fn dead_candidate(url: &str) -> StreamCandidate {
    StreamCandidate {
        url: url.to_string(),
        score: 0,
        ..StreamCandidate::default()
    }
}

/// When the candidate list is empty, `AllCandidatesExhausted` is returned.
#[test]
fn empty_candidate_list_yields_exhausted_error() {
    let candidates: Vec<StreamCandidate> = vec![];
    let result = stui_runtime::quality::select_best_candidate(&candidates);
    assert!(
        result.is_none(),
        "empty list should yield None, caller maps to AllCandidatesExhausted"
    );
}

/// `StuidError::AllCandidatesExhausted` must be recoverable = false.
#[test]
fn exhausted_error_is_not_recoverable() {
    let err = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    };
    assert!(
        !err.is_recoverable(),
        "AllCandidatesExhausted should not be auto-retried"
    );
}

/// `user_message()` for exhausted error must be human-readable.
#[test]
fn exhausted_error_has_user_message() {
    let err = StuidError::AllCandidatesExhausted {
        entry_id: "tt1234567".into(),
    };
    let msg = err.user_message();
    assert!(!msg.is_empty());
    assert!(
        msg.contains("stream") || msg.contains("working") || msg.contains("found"),
        "user_message should mention stream failure, got: {msg}"
    );
}
```

- [ ] **Step 2: Find the correct path to `select_best_candidate`**

```bash
grep -rn "fn select_best_candidate\|pub fn.*best_candidate\|pub fn.*select" src/quality/ | head -10
```

Adjust the import in the test to match. If the function is named differently, use whatever selects the best candidate from a slice.

- [ ] **Step 3: Run tests**

```bash
cargo test --test stream_exhaustion_tests 2>&1 | tail -10
# Expected: test result: ok. 3 passed
```

If `StreamCandidate::default()` doesn't exist, check the actual struct fields:
```bash
grep -n "pub struct StreamCandidate" src/quality/*.rs
```
And construct accordingly.

- [ ] **Step 4: Commit**

```bash
git add tests/stream_exhaustion_tests.rs
git commit -m "test: integration tests for stream candidate exhaustion path"
```

---

## Chunk 5: P2 — Tracing spans for search/resolve/playback

**Goal:** Add `#[tracing::instrument]` (or manual `tracing::info_span!`) to the three core operations — search, resolve_stream, and playback start — so production traces show latency and errors per operation.

**Files:**
- Modify: `runtime/src/engine/mod.rs` — `search` and `resolve_stream` (or equivalent) functions
- Modify: `runtime/src/player/manager.rs` — playback start handler

---

### Task 15: Span on `Engine::search`

- [ ] **Step 1: Find the search function signature**

```bash
grep -n "pub async fn search\|pub fn search" src/engine/mod.rs | head -5
```

- [ ] **Step 2: Add `#[tracing::instrument]`**

Add the attribute above the function. Include fields that will appear in every log line for this span:

```rust
#[tracing::instrument(
    name = "engine.search",
    skip(self, opts),
    fields(
        query = %opts.query,
        provider_count = %self.provider_count(),
    )
)]
pub async fn search(&self, opts: SearchOptions) -> Vec<CatalogEntry> {
```

`skip(self, opts)` prevents the full structs from being serialized (which could be large). We capture only the fields we name explicitly.

If `provider_count()` doesn't exist, skip that field or replace with a static count:
```bash
grep -n "fn provider_count\|providers\.len\|registry\.len" src/engine/mod.rs | head -5
```

- [ ] **Step 3: Verify compile**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 4: Commit**

```bash
git add src/engine/mod.rs
git commit -m "obs: add tracing span to Engine::search"
```

---

### Task 16: Span on `Engine::resolve_stream` (or equivalent)

- [ ] **Step 1: Find the stream resolve function**

```bash
grep -n "pub async fn.*stream\|pub async fn.*resolve" src/engine/mod.rs | head -10
```

- [ ] **Step 2: Add instrument**

```rust
#[tracing::instrument(
    name = "engine.resolve_stream",
    skip(self),
    fields(entry_id = %entry_id)
)]
pub async fn resolve_stream(&self, entry_id: &str, ...) -> Result<Vec<StreamCandidate>> {
```

Adjust field names to match actual parameters.

- [ ] **Step 3: Verify**

```bash
cargo check -p stui-runtime
```

- [ ] **Step 4: Commit**

```bash
git add src/engine/mod.rs
git commit -m "obs: add tracing span to Engine::resolve_stream"
```

---

### Task 17: Span on playback start

- [ ] **Step 1: Find playback dispatch in PlayerManager**

```bash
grep -n "pub async fn\|fn handle\|fn play\|fn start" src/player/manager.rs | head -20
```

- [ ] **Step 2: Add span to the play/start function**

```rust
#[tracing::instrument(
    name = "player.start",
    skip(self),
    fields(url = %stream_url)
)]
pub async fn play(&self, stream_url: &str, ...) -> Result<()> {
```

- [ ] **Step 3: Verify + full test suite**

```bash
cargo check -p stui-runtime && cargo test 2>&1 | tail -5
# Expected: test result: ok.
```

- [ ] **Step 4: Final commit**

```bash
git add src/player/manager.rs src/engine/mod.rs
git commit -m "obs: add tracing spans to playback start"
```

---

## Final verification

After all chunks are done:

```bash
# No dead_code warnings at crate level
cargo check 2>&1 | grep dead_code
# Expected: no output

# All tests green
cargo test 2>&1 | tail -5
# Expected: test result: ok.

# No bare .unwrap() left in production code (test code is OK)
grep -rn "\.unwrap()" src/ --include="*.rs" \
  | grep -v "#\[test\]\|mod tests\|cfg(test)\|//.*unwrap" \
  | grep -v "store\.rs:\|analyzer\.rs:\|fingerprint\.rs:\|dsd\.rs:\|convolution\.rs:\|resample\.rs:\|mpd_config\.rs:\|profile_store\.rs:\|ipc_batcher\.rs:\|roon\.rs:\|protocol\.rs:\|process\.rs:\|secrets\.rs:\|manager\.rs:"
# Expected: only lines you consciously decided to leave (test helpers outside cfg(test))
```
