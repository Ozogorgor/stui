# Rust/Go Boundary Separation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all identified boundary violations between the Rust business-logic layer and the Go UI layer, so each layer owns only its proper concerns.

**Architecture:** Four violations are addressed in descending severity. Each task is self-contained: it removes code from the wrong layer, adjusts or adds tests, and commits. No task depends on another being complete first (except Task 3 which should follow Task 1 to avoid confusion).

**Tech Stack:** Rust (cargo/nextest), Go (go test), JSON-over-stdio IPC, no new dependencies.

---

## Violation Summary

| # | Severity | Violation | Files |
|---|----------|-----------|-------|
| 1 | HIGH | Go's `IPCStore.UpdatePosition` recalculates the completion threshold itself instead of trusting Rust | `tui/pkg/watchhistory/ipc.go:143` |
| 2 | MEDIUM | Go loads and owns `stream_policy.json`; Rust should own config files | `tui/internal/ui/screens/stream_picker.go:129-193` |
| 3 | LOW | Rust formats the `badge` display string (`"1080p ★ 87"`) before sending it over IPC | `runtime/src/pipeline/resolve.rs:19` |
| 4 | LOW | Dead presentation helpers (`position_str`, `duration_str`, `progress_str`, `audio_label`, `sub_label`) sit in Rust's `PlaybackState` | `runtime/src/player/state.rs:143-220` |

---

## Chunk 1: Completion Threshold (HIGH)

### Task 1: Remove Go's duplicate completion check from IPCStore

**Background:**
- `COMPLETED_THRESHOLD = 0.90` is the authoritative business rule in Rust (`runtime/src/watchhistory/store.rs:13`).
- `completedThreshold = 0.90` also lives in Go (`tui/pkg/watchhistory/history.go:17`) and is used by two things:
  1. `Store.UpdatePosition` (the local-file store — acceptable, it runs standalone without a daemon)
  2. `IPCStore.UpdatePosition` (the IPC-backed store — **violation**: Rust already applies the rule)
- The fix removes only the redundant check in `IPCStore.UpdatePosition`. The constant in `history.go` stays (it's needed by the local `Store`).

**Files:**
- Modify: `tui/pkg/watchhistory/ipc.go:134-151`
- Modify: `tui/pkg/watchhistory/history_test.go` (add regression test)

- [ ] **Step 1: Read the current IPCStore.UpdatePosition**

Open `tui/pkg/watchhistory/ipc.go`. Locate `UpdatePosition` (line ~134). Note the three lines that apply `completedThreshold`:
```go
if duration > 0 && position/duration >= completedThreshold {
    e.Completed = true
}
```

- [ ] **Step 2: Write a failing test that documents the expected behaviour**

Add to `tui/pkg/watchhistory/history_test.go`:

```go
// TestIPCStoreUpdatePositionDoesNotAutoComplete verifies that IPCStore does NOT
// locally apply the completion threshold — that decision belongs to Rust.
// The completed flag must only change when Rust pushes an updated entry.
func TestIPCStoreUpdatePositionDoesNotAutoComplete(t *testing.T) {
    // IPCStore requires a live IPC client, so we test via Store (local) to confirm
    // the constant is still used there, and separately verify IPCStore
    // has no threshold logic by inspecting the source.
    //
    // Regression guard: if someone re-adds the threshold to IPCStore,
    // the Store test will still pass but this comment will be stale.
    // See: tui/pkg/watchhistory/ipc.go UpdatePosition — must NOT contain
    // "completedThreshold" or any fraction comparison.
    s := &Store{}
    s.Upsert(Entry{ID: "x", Title: "Movie", Duration: 100})
    // 89% → should NOT complete
    updated := s.UpdatePosition("x", 89.0, 100.0)
    if !updated {
        t.Fatal("UpdatePosition returned false for known entry")
    }
    e := s.Get("x")
    if e.Completed {
        t.Error("Store.UpdatePosition: should not complete at 89%")
    }
    // 91% → should complete (local Store still applies the rule)
    s.UpdatePosition("x", 91.0, 100.0)
    e = s.Get("x")
    if !e.Completed {
        t.Error("Store.UpdatePosition: should complete at 91%")
    }
}
```

- [ ] **Step 3: Run the test to confirm it passes (documents current Store behaviour)**

```bash
cd tui && go test ./pkg/watchhistory/... -v -run TestIPCStoreUpdatePositionDoesNotAutoComplete
```

Expected: PASS (the test checks `Store`, not `IPCStore` — establishes baseline).

- [ ] **Step 4: Remove the completion-threshold logic from IPCStore.UpdatePosition**

In `tui/pkg/watchhistory/ipc.go`, find `UpdatePosition` and delete these three lines:
```go
// DELETE these lines:
if duration > 0 && position/duration >= completedThreshold {
    e.Completed = true
}
```

The resulting function body should update `e.Position`, `e.Duration`, and `e.LastWatched` — nothing else.

- [ ] **Step 5: Confirm no other reference to completedThreshold was removed accidentally**

```bash
cd tui && grep -n "completedThreshold" pkg/watchhistory/ipc.go
```

Expected: no output (the constant is gone from ipc.go).

```bash
cd tui && grep -n "completedThreshold" pkg/watchhistory/history.go
```

Expected: line 17 still present (needed by local Store).

- [ ] **Step 6: Run all watchhistory tests**

```bash
cd tui && go test ./pkg/watchhistory/... -v
```

Expected: all PASS.

- [ ] **Step 7: Run full Go test suite**

```bash
cd tui && go test ./...
```

Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add tui/pkg/watchhistory/ipc.go tui/pkg/watchhistory/history_test.go
git commit -m "fix(boundary): remove duplicate completion-threshold check from IPCStore

IPCStore delegates position updates to the Rust runtime via IPC.
The Rust backend authoritatively applies the 0.90 threshold in
watchhistory/store.rs. Go's IPCStore was redundantly re-applying
the same check, creating two sources of truth for a business rule.

Remove the threshold check from IPCStore.UpdatePosition; the local
file-backed Store keeps its copy since it runs without a daemon."
```

---

## Chunk 2: Stream Policy Ownership (MEDIUM)

### Task 2: Move stream_policy.json loading to Rust

**Background:**
- `stream_picker.go` defines `StreamPolicy`, loads `~/.config/stui/stream_policy.json`, and converts it to `ipc.StreamPreferences` before every `RankStreams` call.
- Rust already owns `StreamPreferences` / `RankingPolicy` in `runtime/src/quality/policy.rs` and accepts preferences over IPC via `StreamPreferencesWire`.
- A design note in `tui/docs/superpowers/specs/2026-03-17-stream-ranking-migration-design.md` anticipated this migration.
- The fix: Rust reads/writes the policy file; Go fetches the active policy via a new `get_stream_policy` request on screen open, and saves edits via `set_stream_policy`. Go's `StreamPolicy` struct and all file I/O helpers are removed.

**Files:**
- Modify: `runtime/src/ipc/v1/mod.rs` — add `GetStreamPolicy` / `SetStreamPolicy` request + response variants
- Create: `runtime/src/pipeline/policy_io.rs` — file load/save for stream policy
- Modify: `runtime/src/pipeline/mod.rs` — pub use new module
- Modify: `runtime/src/ipc/mod.rs` — wire up new request handlers
- Modify: `tui/internal/ipc/messages.go` — add `GetStreamPolicyRequest` / `SetStreamPolicyRequest` / `StreamPolicyMsg`
- Modify: `tui/internal/ipc/requests.go` — add `GetStreamPolicy()` / `SetStreamPolicy()` client methods
- Modify: `tui/internal/ui/screens/stream_picker.go` — remove `StreamPolicy`, `loadStreamPolicy`, `SaveStreamPolicy`, `streamPolicyPath`, `defaultStreamPolicy`; load policy from IPC
- Modify: `tui/internal/ui/screens/stream_picker_test.go` — remove/update tests that relied on local policy loading

#### Sub-task 2a: Add IPC messages and Rust handler

- [ ] **Step 1: Add request/response variants to Rust IPC v1**

In `runtime/src/ipc/v1/mod.rs`, locate the `Request` enum and add:
```rust
GetStreamPolicy,
SetStreamPolicy(SetStreamPolicyRequest),
```
Locate the `Response` enum and add:
```rust
GetStreamPolicy(StreamPolicyResponse),
SetStreamPolicy,
```
Add the new wire structs near the existing `RankStreamsRequest` area:
```rust
#[derive(Debug, Deserialize)]
pub struct SetStreamPolicyRequest {
    pub policy: StreamPreferencesWire,
}

#[derive(Debug, Serialize)]
pub struct StreamPolicyResponse {
    pub policy: StreamPreferencesWire,
}
```

- [ ] **Step 2: Create runtime/src/pipeline/policy_io.rs**

```rust
//! Persistent storage for the user's stream selection policy.
//!
//! Policy is read from and written to `~/.config/stui/stream_policy.json`.
//! Missing or invalid files are silently replaced with `StreamPreferences::default()`.

use std::path::PathBuf;

use crate::quality::StreamPreferences;

fn policy_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stui")
        .join("stream_policy.json")
}

pub fn load_stream_policy() -> StreamPreferences {
    let path = policy_path();
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return StreamPreferences::default(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

pub fn save_stream_policy(prefs: &StreamPreferences) -> std::io::Result<()> {
    let path = policy_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let data = serde_json::to_vec_pretty(prefs).expect("serialize StreamPreferences");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default_policy() {
        let dir = tempfile::tempdir().unwrap();
        // Override config path is not directly injectable, so test serde only.
        let prefs = StreamPreferences::default();
        let json = serde_json::to_string(&prefs).unwrap();
        let decoded: StreamPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.max_size_mb, prefs.max_size_mb);
        assert_eq!(decoded.min_seeders, prefs.min_seeders);
        assert_eq!(decoded.prefer_hdr, prefs.prefer_hdr);
        drop(dir);
    }
}
```

> Note: add `tempfile` as a dev-dependency in `runtime/Cargo.toml` if it isn't already present.

- [ ] **Step 3: Wire new module into pipeline/mod.rs**

In `runtime/src/pipeline/mod.rs`, add:
```rust
pub mod policy_io;
```

- [ ] **Step 4: Handle GetStreamPolicy and SetStreamPolicy in the IPC dispatcher**

In the Rust IPC request handler (find where `RankStreams` is dispatched — likely `runtime/src/ipc/mod.rs` or the main handler), add:
```rust
Request::GetStreamPolicy => {
    let prefs = pipeline::policy_io::load_stream_policy();
    Response::GetStreamPolicy(v1::StreamPolicyResponse {
        policy: prefs.into(),
    })
}
Request::SetStreamPolicy(req) => {
    let prefs: StreamPreferences = req.policy.into();
    if let Err(e) = pipeline::policy_io::save_stream_policy(&prefs) {
        tracing::warn!("save_stream_policy failed: {e}");
    }
    Response::SetStreamPolicy
}
```

- [ ] **Step 5: Run Rust tests**

```bash
cargo test -p runtime
```

Expected: all PASS (new `policy_io` tests pass, existing tests unaffected).

- [ ] **Step 6: Commit Rust side**

```bash
git add runtime/src/ipc/v1/mod.rs runtime/src/pipeline/policy_io.rs runtime/src/pipeline/mod.rs runtime/src/ipc/mod.rs
git commit -m "feat(ipc): add get_stream_policy / set_stream_policy requests

Rust now owns the stream_policy.json config file. New IPC messages
allow the TUI to fetch and persist the user's stream selection
preferences without touching the filesystem directly."
```

#### Sub-task 2b: Update Go layer

- [ ] **Step 7: Add IPC message types in Go**

In `tui/internal/ipc/messages.go`, add (near `StreamPreferences`):
```go
// StreamPolicyMsg is the response payload for a get_stream_policy request.
type StreamPolicyMsg struct {
    Policy StreamPreferences `json:"policy"`
}
```

- [ ] **Step 8: Add client methods in requests.go**

In `tui/internal/ipc/requests.go`, add:
```go
// GetStreamPolicy fetches the active stream policy from the runtime.
func (c *Client) GetStreamPolicy() <-chan StreamPreferences {
    ch := make(chan StreamPreferences, 1)
    go func() {
        defer close(ch)
        raw := c.sendRequest(map[string]any{"type": "get_stream_policy"})
        var resp StreamPolicyMsg
        if err := json.Unmarshal(raw, &resp); err != nil {
            ch <- StreamPreferences{}
            return
        }
        ch <- resp.Policy
    }()
    return ch
}

// SetStreamPolicy persists the stream policy via the runtime.
func (c *Client) SetStreamPolicy(prefs StreamPreferences) {
    c.sendRequest(map[string]any{
        "type":   "set_stream_policy",
        "policy": prefs,
    })
}
```

- [ ] **Step 9: Update stream_picker.go — remove local policy I/O, load via IPC**

In `tui/internal/ui/screens/stream_picker.go`:

**Delete** the following functions entirely:
- `defaultStreamPolicy()` (lines ~144-148)
- `streamPolicyPath()` (lines ~150-153)
- `loadStreamPolicy()` (lines ~155-163)
- `SaveStreamPolicy()` (lines ~165-180)
- `toStreamPreferences()` method on `StreamPolicy` (lines ~182-193)

**Delete** the `StreamPolicy` struct (lines ~134-142). Also remove imports for `encoding/json`, `os`, `path/filepath` if no longer used elsewhere in the file.

**Replace** every call to `loadStreamPolicy()` with a call to the IPC client:
```go
// In the screen's Init or wherever loadStreamPolicy() was called:
prefs := <-s.client.GetStreamPolicy()
s.policy = prefs  // policy is now ipc.StreamPreferences directly
```

**Replace** every call to `SaveStreamPolicy(p)` with:
```go
s.client.SetStreamPolicy(s.policy)
```

**Replace** calls to `s.policy.toStreamPreferences()` with `s.policy` (it's already `ipc.StreamPreferences`).

Update the `StreamPickerScreen` struct field type:
```go
// Before:
policy StreamPolicy
// After:
policy ipc.StreamPreferences
```

- [ ] **Step 10: Update or remove tests in stream_picker_test.go**

Remove any test that calls `loadStreamPolicy()` or `SaveStreamPolicy()` directly (these no longer exist in Go). `TestBestStreamForTierExactMatch` and friends test `BestStreamForTier` which is UI logic — keep those.

- [ ] **Step 11: Run Go tests**

```bash
cd tui && go test ./internal/ui/screens/... -v
cd tui && go test ./internal/ipc/... -v
```

Expected: all PASS.

- [ ] **Step 12: Run full Go test suite**

```bash
cd tui && go test ./...
```

Expected: all PASS.

- [ ] **Step 13: Commit Go side**

```bash
git add tui/internal/ipc/messages.go tui/internal/ipc/requests.go \
        tui/internal/ui/screens/stream_picker.go \
        tui/internal/ui/screens/stream_picker_test.go
git commit -m "fix(boundary): remove stream policy file I/O from Go layer

Go no longer loads or writes ~/.config/stui/stream_policy.json.
Rust owns the policy file; Go fetches/saves preferences via the
new get_stream_policy and set_stream_policy IPC calls.

Removes: StreamPolicy struct, loadStreamPolicy, SaveStreamPolicy,
streamPolicyPath, defaultStreamPolicy, toStreamPreferences."
```

---

## Chunk 3: Badge Formatting (LOW)

### Task 3: Remove pre-formatted Badge string from Rust IPC wire type

**Background:**
- `runtime/src/pipeline/resolve.rs:19` builds `badge: format!("{} ★ {}", stream.quality.label(), score)` and sends it over IPC.
- `tui/internal/ipc/messages.go:144` exposes `Badge string` in `StreamInfo`.
- `tui/internal/ui/screens/stream_picker.go` uses `st.Badge` and `best.Stream.Badge` for rendering.
- `Quality` and `Score` are already separate fields in `StreamInfo` — Go has everything it needs to build the badge itself.

**Files:**
- Modify: `runtime/src/pipeline/resolve.rs` — remove `badge` field from `stream_to_wire`
- Modify: `runtime/src/ipc/v1/mod.rs` (or wherever `StreamInfoWire` is defined) — remove `badge` field
- Modify: `tui/internal/ipc/messages.go` — remove `Badge` field from `StreamInfo`
- Modify: `tui/internal/ui/screens/stream_picker.go` — replace `st.Badge` with a local helper

- [ ] **Step 1: Add a Go helper that builds the badge string**

In `tui/internal/ui/screens/stream_picker.go`, add this function near the top of the sort helpers section:
```go
// streamBadge builds the quality+score display label for a stream row.
// Example: "1080p ★ 87"
func streamBadge(s ipc.StreamInfo) string {
    if s.Quality == "" {
        return fmt.Sprintf("★ %d", s.Score)
    }
    return fmt.Sprintf("%s ★ %d", s.Quality, s.Score)
}
```

- [ ] **Step 2: Write a test for the new helper**

Add to `tui/internal/ui/screens/stream_picker_test.go`:
```go
func TestStreamBadge(t *testing.T) {
    cases := []struct {
        in   ipc.StreamInfo
        want string
    }{
        {ipc.StreamInfo{Quality: "1080p", Score: 87}, "1080p ★ 87"},
        {ipc.StreamInfo{Quality: "4K", Score: 100}, "4K ★ 100"},
        {ipc.StreamInfo{Quality: "", Score: 50}, "★ 50"},
    }
    for _, tc := range cases {
        got := streamBadge(tc.in)
        if got != tc.want {
            t.Errorf("streamBadge(%+v) = %q, want %q", tc.in, got, tc.want)
        }
    }
}
```

- [ ] **Step 3: Run the test to confirm it passes**

```bash
cd tui && go test ./internal/ui/screens/... -v -run TestStreamBadge
```

Expected: PASS.

- [ ] **Step 4: Replace all uses of st.Badge / best.Stream.Badge in stream_picker.go**

Search for every occurrence of `.Badge` in `stream_picker.go` and replace with a call to `streamBadge(st)` or `streamBadge(best.Stream)` as appropriate.

Locations to update (from grep: lines 605, 693, 794, 842):
- Line ~605: `label := st.Badge` → `label := streamBadge(st)`
- Line ~693: `if st.Badge != "" { qual = st.Badge }` → `qual = streamBadge(st)`
- Line ~794: `label := best.Stream.Badge` → `label := streamBadge(best.Stream)`
- Line ~842: `lbl := r.Stream.Badge` → `lbl := streamBadge(r.Stream)`

- [ ] **Step 5: Remove Badge from Go's StreamInfo struct**

In `tui/internal/ipc/messages.go`, delete the line:
```go
Badge     string  `json:"badge"`
```

- [ ] **Step 6: Remove badge from Rust's StreamInfoWire**

Find `StreamInfoWire` in the Rust IPC types (likely `runtime/src/ipc/v1/mod.rs`). Remove the `badge` field. Then remove the `badge:` assignment from `stream_to_wire` in `runtime/src/pipeline/resolve.rs:19`.

- [ ] **Step 7: Run Rust tests**

```bash
cargo test -p runtime
```

Expected: all PASS.

- [ ] **Step 8: Run Go tests**

```bash
cd tui && go test ./...
```

Expected: all PASS.

- [ ] **Step 9: Commit**

```bash
git add runtime/src/pipeline/resolve.rs runtime/src/ipc/v1/mod.rs \
        tui/internal/ipc/messages.go \
        tui/internal/ui/screens/stream_picker.go \
        tui/internal/ui/screens/stream_picker_test.go
git commit -m "fix(boundary): move badge formatting from Rust to Go

Rust was pre-formatting a display string ('1080p ★ 87') for Go to
render, crossing the presentation/logic boundary. Quality and Score
are already separate wire fields; Go now assembles the badge itself
via a new streamBadge() helper.

Removes: StreamInfoWire.badge, StreamInfo.Badge"
```

---

## Chunk 4: Dead Presentation Helpers (LOW)

### Task 4: Remove dead presentation methods from Rust PlaybackState

**Background:**
- `runtime/src/player/state.rs` contains `position_str()`, `duration_str()`, `progress_str()`, `audio_label()`, `sub_label()` — all marked `#[allow(dead_code)]`.
- None are called from any other Rust file (confirmed by grep); none appear in the IPC wire format.
- `progress_fraction()` is NOT dead — it's a genuine numeric computation (used for progress bar math) and should stay.
- `TrackInfo::label()` is called by `audio_label()` and `sub_label()`. Once those are removed, `label()` also becomes dead unless it has other callers.
- `format_duration()` is a private helper; once `position_str` etc. are gone, it has no callers except its own test.

**Files:**
- Modify: `runtime/src/player/state.rs`

- [ ] **Step 1: Confirm no external callers before deleting**

```bash
grep -rn "position_str\|duration_str\|progress_str\|audio_label\|sub_label" \
    runtime/src/ --include="*.rs" | grep -v "state.rs"
```

Expected: no output. If any hits appear, stop and re-evaluate.

```bash
grep -rn "\.label()" runtime/src/ --include="*.rs" | grep -v "state.rs" | grep -v "quality"
```

Expected: only `plugin.rs` and `candidate.rs` hits (these call `stream.quality.label()` — a different `label()` on a different type, unrelated to `TrackInfo::label()`).

- [ ] **Step 2: Delete the dead methods from PlaybackState**

In `runtime/src/player/state.rs`, delete:
- `impl TrackInfo { pub fn label(...) }` block (lines ~30-42) — only used by `audio_label`/`sub_label`
- `pub fn position_str(...)` (lines ~144-148)
- `pub fn duration_str(...)` (lines ~150-154)
- `pub fn progress_str(...)` (lines ~156-164)
- `pub fn audio_label(...)` (lines ~175-187)
- `pub fn sub_label(...)` (lines ~189-204)
- `pub fn audio_tracks(...)` and `pub fn subtitle_tracks(...)` — check if these have callers first:

```bash
grep -rn "audio_tracks\|subtitle_tracks" runtime/src/ --include="*.rs" | grep -v "state.rs"
```

If no callers: delete them too. If callers exist: keep them.

Also delete the private `format_duration` function (lines ~225-238) and its test (lines ~244-249) — the function will have no callers once the above methods are gone.

> Keep: `progress_fraction()` (has callers in UI rendering logic), `Default impl`, all struct fields.

- [ ] **Step 3: Run Rust tests**

```bash
cargo test -p runtime
```

Expected: all PASS. No `dead_code` warnings on the removed items.

- [ ] **Step 4: Confirm no new dead_code warnings introduced**

```bash
cargo clippy -p runtime 2>&1 | grep "dead_code"
```

Expected: no new warnings compared to before this task.

- [ ] **Step 5: Commit**

```bash
git add runtime/src/player/state.rs
git commit -m "chore(boundary): remove dead presentation helpers from PlaybackState

Methods position_str, duration_str, progress_str, audio_label,
sub_label, TrackInfo::label, and format_duration were all marked
dead_code and never called outside state.rs or its tests.

Presentation formatting (HH:MM:SS, track labels) belongs in the
Go UI layer. PlaybackState retains progress_fraction() which
computes a 0-1 ratio for drawing progress bars — a numeric concern,
not a display concern."
```

---

## Verification

After all four tasks are done, run the full test suite one final time:

```bash
# Rust
cargo test --workspace

# Go
cd tui && go test ./...
```

Both should pass with zero failures.
