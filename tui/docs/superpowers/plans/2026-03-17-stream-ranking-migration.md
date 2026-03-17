# Stream Ranking Migration Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move stream quality ranking and user stream preferences from the Go TUI into the Rust runtime, so the runtime returns pre-ranked streams and the TUI is a pure renderer.

**Architecture:** A new `StreamPreferences` struct is added to the Rust config system. The existing `quality::rank()` pipeline is extended with a `post_rank_adjust()` step that applies user preferences (protocol, size, labels, codecs, HDR). A new `GetConfig` IPC command lets the TUI initialise its Settings display from the runtime. The Go ranking engine (~200 lines) is deleted; the Settings screen gains 8 new items in the existing "Streaming" category.

**Tech Stack:** Rust (tokio, serde, anyhow), Go 1.22 (Bubble Tea), NDJSON IPC over Unix socket

---

## Chunk 1: Rust Backend

### Task 1: StreamPreferences struct + RuntimeConfig field

**Files:**
- Modify: `runtime/src/config/types.rs`
- Test: `runtime/tests/config_manager_tests.rs`

**Context:** `RuntimeConfig` is defined in `runtime/src/config/types.rs`. It already has sub-structs like `StreamingConfig`, `PlaybackConfig`, etc. All sub-structs derive `Debug, Clone, Serialize, Deserialize` and implement `Default`. Add `StreamPreferences` following the same pattern.

- [ ] **Step 1: Write the failing test**

Add to `runtime/tests/config_manager_tests.rs`:

```rust
#[test]
fn test_stream_preferences_default_values() {
    let prefs = stui_runtime::config::types::StreamPreferences::default();
    assert_eq!(prefs.preferred_protocol, None);
    assert_eq!(prefs.max_resolution, None);
    assert_eq!(prefs.max_size_mb, None);
    assert_eq!(prefs.min_seeders, 0);
    assert!(prefs.avoid_labels.is_empty());
    assert!(!prefs.prefer_hdr);
    assert!(prefs.preferred_codecs.is_empty());
    assert_eq!(prefs.seeder_weight, 1.0);
    assert!(prefs.exclude_cam);
}

#[test]
fn test_runtime_config_has_stream_field() {
    let cfg = stui_runtime::config::types::RuntimeConfig::default();
    // stream field exists and has correct defaults
    assert_eq!(cfg.stream.min_seeders, 0);
    assert!(cfg.stream.exclude_cam);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_stream_preferences 2>&1 | tail -20
```

Expected: compile error — `StreamPreferences` does not exist yet.

- [ ] **Step 3: Add `StreamPreferences` to `runtime/src/config/types.rs`**

After the last existing sub-struct (before `RuntimeConfig`), add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPreferences {
    pub preferred_protocol: Option<String>,
    pub max_resolution:     Option<String>,
    pub max_size_mb:        Option<u64>,
    pub min_seeders:        u32,
    pub avoid_labels:       Vec<String>,
    pub prefer_hdr:         bool,
    pub preferred_codecs:   Vec<String>,
    pub seeder_weight:      f64,
    pub exclude_cam:        bool,
}

impl Default for StreamPreferences {
    fn default() -> Self {
        Self {
            preferred_protocol: None,
            max_resolution:     None,
            max_size_mb:        None,
            min_seeders:        0,
            avoid_labels:       vec![],
            prefer_hdr:         false,
            preferred_codecs:   vec![],
            seeder_weight:      1.0,
            exclude_cam:        true,
        }
    }
}
```

Then add the field to `RuntimeConfig` struct (after the existing `plugin_repos` field):

```rust
#[serde(default)]
pub stream: StreamPreferences,
```

The `#[serde(default)]` attribute ensures existing `stui.toml` files without a `[stream]` section still deserialise correctly.

**Important:** `RuntimeConfig` uses an explicit `impl Default` (not `#[derive(Default)]`). You must add the new field to its body manually. Find the `impl Default for RuntimeConfig` block and add:

```rust
stream: StreamPreferences::default(),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_stream_preferences 2>&1 | tail -10
cargo test -p stui-runtime test_runtime_config_has_stream 2>&1 | tail -10
```

Expected: both PASS.

- [ ] **Step 5: Verify full test suite still passes**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime 2>&1 | tail -20
```

Expected: all existing tests pass.

- [ ] **Step 6: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add runtime/src/config/types.rs runtime/tests/config_manager_tests.rs
git commit -m "feat(runtime): add StreamPreferences struct and RuntimeConfig.stream field"
```

---

### Task 2: `From<&StreamPreferences> for RankingPolicy`

**Files:**
- Modify: `runtime/src/quality/policy.rs`
- Test: `runtime/tests/ranking_tests.rs`

**Context:** `RankingPolicy` is in `runtime/src/quality/policy.rs`. Its fields are: `resolution_weights: [u32; 4]`, `prefer_lower_resolution: bool`, `seeder_weight: f64`, `exclude_cam: bool`, `min_seeders: u32`. The conversion maps `StreamPreferences` user-facing fields to these internal weights.

- [ ] **Step 1: Write the failing test**

Add to `runtime/tests/ranking_tests.rs`:

```rust
use stui_runtime::config::types::StreamPreferences;

#[test]
fn test_stream_prefs_default_equals_ranking_policy_default() {
    let prefs = StreamPreferences::default();
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    let default_policy = stui_runtime::quality::RankingPolicy::default();
    assert_eq!(policy.resolution_weights, default_policy.resolution_weights);
    assert_eq!(policy.seeder_weight, default_policy.seeder_weight);
    assert_eq!(policy.exclude_cam, default_policy.exclude_cam);
    assert_eq!(policy.min_seeders, default_policy.min_seeders);
}

#[test]
fn test_max_resolution_1080p_zeroes_4k_weight() {
    let prefs = StreamPreferences {
        max_resolution: Some("1080p".to_string()),
        ..Default::default()
    };
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    assert_eq!(policy.resolution_weights[3], 0, "4K weight should be 0 when max is 1080p");
    assert_eq!(policy.resolution_weights[2], 300, "1080p weight should be 300");
}

#[test]
fn test_max_resolution_720p_zeroes_1080p_and_4k() {
    let prefs = StreamPreferences {
        max_resolution: Some("720p".to_string()),
        ..Default::default()
    };
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    assert_eq!(policy.resolution_weights[3], 0);
    assert_eq!(policy.resolution_weights[2], 0);
    assert_eq!(policy.resolution_weights[1], 200);
}

#[test]
fn test_seeder_weight_and_min_seeders_forwarded() {
    let prefs = StreamPreferences {
        seeder_weight: 2.5,
        min_seeders: 10,
        ..Default::default()
    };
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    assert_eq!(policy.seeder_weight, 2.5);
    assert_eq!(policy.min_seeders, 10);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_stream_prefs_default 2>&1 | tail -10
```

Expected: compile error — `From<&StreamPreferences>` not implemented.

- [ ] **Step 3: Add the `From` impl to `runtime/src/quality/policy.rs`**

At the top of the file, add:

```rust
use crate::config::types::StreamPreferences;
```

After the existing `RankingPolicy` impls, add:

```rust
impl From<&StreamPreferences> for RankingPolicy {
    fn from(prefs: &StreamPreferences) -> Self {
        let resolution_weights = match prefs.max_resolution.as_deref() {
            Some("sd")    => [100,   0,   0,   0],
            Some("720p")  => [100, 200,   0,   0],
            Some("1080p") => [100, 200, 300,   0],
            _             => [100, 200, 300, 400],
        };
        RankingPolicy {
            resolution_weights,
            prefer_lower_resolution: false,
            seeder_weight:           prefs.seeder_weight,
            exclude_cam:             prefs.exclude_cam,
            min_seeders:             prefs.min_seeders,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_stream_prefs 2>&1 | tail -10
cargo test -p stui-runtime test_max_resolution 2>&1 | tail -10
cargo test -p stui-runtime test_seeder_weight 2>&1 | tail -10
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add runtime/src/quality/policy.rs runtime/tests/ranking_tests.rs
git commit -m "feat(runtime): add From<&StreamPreferences> for RankingPolicy"
```

---

### Task 3: `post_rank_adjust` + GetStreams wiring

**Files:**
- Modify: `runtime/src/engine/mod.rs`
- Modify: `runtime/src/pipeline/resolve.rs`
- Test: `runtime/tests/ranking_tests.rs`

**Context:** The `GetStreams` IPC request is handled in `runtime/src/pipeline/resolve.rs` (`run_get_streams` function), which calls `engine.ranked_streams()` with a hardcoded `RankingPolicy::default()`. `engine/mod.rs` contains `ranked_streams()` (around line 512) which calls `crate::quality::rank(streams, policy)`. The `StreamCandidate` struct has `stream: Stream` and `score: QualityScore`. `QualityScore` fields: `resolution`, `codec`, `seeders`, `bitrate`, `source`, `hdr_bonus` — all `u32`. `Stream.hdr` is `HdrFormat` (enum) from `crate::providers`. `HdrFormat::None` means no HDR. `Stream.size_bytes: Option<u64>`. `Stream.url: String`, `Stream.name: String`.

Read both `engine/mod.rs` and `pipeline/resolve.rs` to find exactly where `quality::rank` is called and where `RankingPolicy::default()` is constructed before implementing.

- [ ] **Step 1: Write the failing test**

Add to `runtime/tests/ranking_tests.rs`:

```rust
use stui_runtime::config::types::StreamPreferences;
use stui_runtime::providers::HdrFormat;

fn stream_with_size(name: &str, quality: StreamQuality, size_mb: u64) -> Stream {
    Stream {
        size_bytes: Some(size_mb * 1_048_576),
        ..stream(name, quality)
    }
}

#[test]
fn test_post_rank_adjust_deprioritises_oversized_streams() {
    // A stream over the size limit should rank below a smaller stream
    // even if it has higher raw quality
    let prefs = StreamPreferences {
        max_size_mb: Some(500),
        ..Default::default()
    };
    let streams = vec![
        stream_with_size("4K BluRay huge", StreamQuality::Uhd4k, 20_000),
        stream_with_size("1080p WEB-DL", StreamQuality::Hd1080, 300),
    ];
    let ranked = stui_runtime::engine::rank_with_prefs(streams, &prefs);
    assert!(
        ranked[0].stream.name.contains("1080p"),
        "1080p under size limit should beat oversized 4K"
    );
}

#[test]
fn test_post_rank_adjust_penalises_avoided_label() {
    let prefs = StreamPreferences {
        avoid_labels: vec!["CAM".to_string()],
        ..Default::default()
    };
    let streams = vec![
        stream("Movie CAM 1080p", StreamQuality::Hd1080),
        stream("Movie WEB-DL 720p", StreamQuality::Hd720),
    ];
    let ranked = stui_runtime::engine::rank_with_prefs(streams, &prefs);
    assert!(
        ranked[0].stream.name.contains("WEB-DL"),
        "WEB-DL 720p should beat CAM 1080p when CAM is avoided"
    );
}

#[test]
fn test_post_rank_adjust_penalises_wrong_protocol() {
    let prefs = StreamPreferences {
        preferred_protocol: Some("http".to_string()),
        ..Default::default()
    };
    let streams = vec![
        stream("720p torrent", StreamQuality::Hd720),
        stream("720p http", StreamQuality::Hd720),
    ];
    // Set the torrent stream URL to a magnet link
    let mut torrent = stream("720p torrent", StreamQuality::Hd720);
    torrent.url = "magnet:?xt=urn:btih:abc".to_string();
    let mut http_stream = stream("720p http", StreamQuality::Hd720);
    http_stream.url = "https://example.com/video.mp4".to_string();
    let ranked = stui_runtime::engine::rank_with_prefs(vec![torrent, http_stream], &prefs);
    assert!(
        ranked[0].stream.url.starts_with("https"),
        "HTTP stream should beat magnet when preferred_protocol is http"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_post_rank_adjust 2>&1 | tail -10
```

Expected: compile error — `engine::rank_with_prefs` does not exist.

- [ ] **Step 3: Add `post_rank_adjust` and `rank_with_prefs` to `runtime/src/engine/mod.rs`**

First, read `engine/mod.rs` lines 480-560 to find the exact call site of `quality::rank`. Then add the following at the bottom of the file (or near the `ranked_streams` method):

```rust
use crate::config::types::StreamPreferences;
use crate::providers::HdrFormat;
use crate::quality::{rank, StreamCandidate};

/// Public entry point for ranking with user preferences applied.
/// Used by the IPC GetStreams handler and exposed for tests.
pub fn rank_with_prefs(
    streams: Vec<crate::providers::Stream>,
    prefs: &StreamPreferences,
) -> Vec<StreamCandidate> {
    let policy = crate::quality::RankingPolicy::from(prefs);
    let mut candidates = rank(streams, &policy);
    post_rank_adjust(&mut candidates, prefs);
    candidates
}

fn post_rank_adjust(candidates: &mut Vec<StreamCandidate>, prefs: &StreamPreferences) {
    for c in candidates.iter_mut() {
        // Max size — deprioritise if over limit
        if let Some(max_mb) = prefs.max_size_mb {
            if c.stream.size_bytes.map(|b| b / 1_048_576).unwrap_or(0) > max_mb {
                c.score.source = 0;
            }
        }
        // Avoid labels — heavy penalty per match
        for label in &prefs.avoid_labels {
            if c.stream.name.to_lowercase().contains(&label.to_lowercase()) {
                c.score.source = c.score.source.saturating_sub(200);
            }
        }
        // Preferred codecs — bonus
        for codec in &prefs.preferred_codecs {
            if c.stream.name.to_lowercase().contains(&codec.to_lowercase()) {
                c.score.codec = (c.score.codec + 40).min(150);
            }
        }
        // HDR preference bonus
        if prefs.prefer_hdr && c.stream.hdr != HdrFormat::None {
            c.score.hdr_bonus = (c.score.hdr_bonus + 25).min(50);
        }
        // Protocol preference — penalise non-matching
        if let Some(ref proto) = prefs.preferred_protocol {
            let is_torrent = c.stream.url.starts_with("magnet:")
                || c.stream.url.ends_with(".torrent");
            let matches = match proto.as_str() {
                "torrent" => is_torrent,
                "http"    => !is_torrent,
                _         => true,
            };
            if !matches {
                c.score.source = c.score.source.saturating_sub(100);
            }
        }
    }
    // Re-sort after adjustments
    candidates.sort_by(|a, b| b.score.total().cmp(&a.score.total()));
}
```

Next, wire `rank_with_prefs` into the actual `GetStreams` handler. The call chain is:

`IPC GetStreams request` → `pipeline/resolve.rs: run_get_streams()` → `engine.ranked_streams(policy)` → `quality::rank()`

Open `runtime/src/pipeline/resolve.rs` and find where `RankingPolicy::default()` is constructed (this is passed to `engine.ranked_streams`). Replace that construction with:

```rust
// Read user preferences and convert to policy
let stream_prefs = config_manager.snapshot().await.stream;
let policy = crate::quality::RankingPolicy::from(&stream_prefs);
```

Then, after `engine.ranked_streams()` returns its candidates, add a call to `post_rank_adjust` (via the public `rank_with_prefs` or by calling `engine::post_rank_adjust` if made pub). The cleanest approach: in `run_get_streams`, replace the `engine.ranked_streams(policy)` call with `engine::rank_with_prefs(raw_streams, &stream_prefs)` if the raw streams are available at that point, or apply `post_rank_adjust` as a post-step after `engine.ranked_streams()` returns.

Read `pipeline/resolve.rs` carefully to determine which approach fits the existing flow before implementing.

Note: other callers of `engine.ranked_streams()` are unaffected — only `run_get_streams` is changed.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_post_rank_adjust 2>&1 | tail -10
```

Expected: all PASS.

- [ ] **Step 5: Verify full test suite still passes**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add runtime/src/engine/mod.rs runtime/src/pipeline/resolve.rs runtime/tests/ranking_tests.rs
git commit -m "feat(runtime): add post_rank_adjust and rank_with_prefs, wire into GetStreams"
```

---

### Task 4: `SetConfig stream.*` keys + `GetConfig` IPC

**Files:**
- Modify: `runtime/src/config/manager.rs`
- Modify: `runtime/src/ipc/v1/mod.rs`
- Test: `runtime/tests/config_manager_tests.rs`

**Context:** `apply_key()` in `manager.rs` is an exhaustive match on the config key string. Helper functions `as_bool(key, value)?`, `as_f64(key, value)?` etc. are private functions in the same file. Look for `as_u32`, `as_u64`, `as_opt_string`, `as_string_vec` — add them if missing.

The `Request` enum in `ipc/v1/mod.rs` uses `#[serde(tag = "type", rename_all = "snake_case")]`. Adding a variant `GetConfig { keys: Vec<String> }` will serialise as `"type": "get_config"`. The `Response` enum uses the same attributes; `ConfigValues { values: HashMap<String, serde_json::Value> }` will serialise as `"type": "config_values"`.

- [ ] **Step 1: Write failing tests**

Add to `runtime/tests/config_manager_tests.rs`:

```rust
use std::sync::Arc;
use stui_runtime::config::manager::ConfigManager;
use stui_runtime::config::types::RuntimeConfig;
use stui_runtime::events::EventBus;

// ConfigManager::new requires an EventBus (same pattern as make_manager() in existing tests)
fn test_manager() -> ConfigManager {
    let bus = Arc::new(EventBus::new());
    ConfigManager::new(RuntimeConfig::default(), bus)
}

#[tokio::test]
async fn test_set_config_stream_min_seeders() {
    let mgr = test_manager();
    mgr.set("stream.min_seeders", serde_json::json!(5)).await.unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.stream.min_seeders, 5);
}

#[tokio::test]
async fn test_set_config_stream_avoid_labels() {
    let mgr = test_manager();
    mgr.set("stream.avoid_labels", serde_json::json!(["CAM", "HDTV"])).await.unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.stream.avoid_labels, vec!["CAM", "HDTV"]);
}

#[tokio::test]
async fn test_set_config_stream_preferred_protocol() {
    let mgr = test_manager();
    mgr.set("stream.preferred_protocol", serde_json::json!("http")).await.unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.stream.preferred_protocol, Some("http".to_string()));
}

#[tokio::test]
async fn test_set_config_stream_exclude_cam() {
    let mgr = test_manager();
    mgr.set("stream.exclude_cam", serde_json::json!(false)).await.unwrap();
    let snap = mgr.snapshot().await;
    assert!(!snap.stream.exclude_cam);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_set_config_stream 2>&1 | tail -10
```

Expected: tests fail with "unknown config key: stream.min_seeders".

- [ ] **Step 3: Add `stream.*` match arms to `apply_key()` in `manager.rs`**

Read `manager.rs` to find the `apply_key` function and the existing helper functions. Add the following arms to the match (before the `other =>` catch-all):

```rust
"stream.preferred_protocol" => {
    cfg.stream.preferred_protocol = as_opt_string(value);
}
"stream.max_resolution" => {
    cfg.stream.max_resolution = as_opt_string(value);
}
"stream.max_size_mb" => {
    cfg.stream.max_size_mb = as_opt_u64(key, value)?;
}
"stream.min_seeders" => {
    cfg.stream.min_seeders = as_u32(key, value)?;
}
"stream.avoid_labels" => {
    cfg.stream.avoid_labels = as_string_vec(key, value)?;
}
"stream.prefer_hdr" => {
    cfg.stream.prefer_hdr = as_bool(key, value)?;
}
"stream.preferred_codecs" => {
    cfg.stream.preferred_codecs = as_string_vec(key, value)?;
}
"stream.seeder_weight" => {
    cfg.stream.seeder_weight = as_f64(key, value)?;
}
"stream.exclude_cam" => {
    cfg.stream.exclude_cam = as_bool(key, value)?;
}
```

All existing helpers in `manager.rs` take `v: &Value` (a borrowed reference) — `apply_key` passes `&value` to them. The new helpers must follow the same convention. `as_u32` already exists; do not redefine it. Add only the missing helpers (`as_opt_u64`, `as_opt_string`, `as_string_vec`) alongside the existing ones:

```rust
fn as_opt_u64(_key: &str, v: &Value) -> Result<Option<u64>, StuidError> {
    if v.is_null() { return Ok(None); }
    Ok(v.as_u64())
}

fn as_opt_string(v: &Value) -> Option<String> {
    if v.is_null() { return None; }
    v.as_str().map(str::to_owned)
}

fn as_string_vec(key: &str, v: &Value) -> Result<Vec<String>, StuidError> {
    v.as_array()
     .ok_or_else(|| StuidError::config(format!("{key}: expected array")))?
     .iter()
     .map(|x| x.as_str().map(str::to_owned)
          .ok_or_else(|| StuidError::config(format!("{key}: expected string elements"))))
     .collect()
}
```

Note: `Value` is `serde_json::Value`. Check whether it is already imported at the top of `manager.rs` (likely as `use serde_json::Value;`) or add the import if not present.

- [ ] **Step 4: Add `GetConfig` request + `ConfigValues` response to `ipc/v1/mod.rs`**

In the `Request` enum (after the existing `InstallPlugin` variant):

```rust
GetConfig {
    #[serde(default)]
    keys: Vec<String>,
},
```

In the `Response` enum (after the existing `PluginInstalled` variant):

```rust
ConfigValues {
    values: std::collections::HashMap<String, serde_json::Value>,
},
```

In the IPC dispatch function (find where `Request::SetConfig` is handled and add alongside it):

```rust
Request::GetConfig { keys } => {
    let snap = config_manager.snapshot().await;
    let all = serde_json::to_value(&snap).unwrap_or_default();
    let obj = all.as_object().cloned().unwrap_or_default();
    let values: std::collections::HashMap<String, serde_json::Value> = if keys.is_empty() {
        // Flatten one level: return stream.* keys
        flat_config(&obj)
    } else {
        keys.iter()
            .filter_map(|k| lookup_flat(&obj, k).map(|v| (k.clone(), v)))
            .collect()
    };
    Response::ConfigValues { values }
}
```

Add the two private helpers at the bottom of the file:

```rust
/// Flatten a serde_json object one level deep using dot notation.
/// E.g. {"stream": {"min_seeders": 5}} -> {"stream.min_seeders": 5}
fn flat_config(obj: &serde_json::Map<String, serde_json::Value>)
    -> std::collections::HashMap<String, serde_json::Value>
{
    let mut out = std::collections::HashMap::new();
    for (section, val) in obj {
        if let Some(inner) = val.as_object() {
            for (field, fval) in inner {
                out.insert(format!("{section}.{field}"), fval.clone());
            }
        } else {
            out.insert(section.clone(), val.clone());
        }
    }
    out
}

/// Look up a dot-notation key in a flattened config object.
fn lookup_flat(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<serde_json::Value> {
    flat_config(obj).get(key).cloned()
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime test_set_config_stream 2>&1 | tail -10
```

Expected: all PASS.

- [ ] **Step 6: Verify full test suite still passes**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime 2>&1 | tail -20
```

Expected: no regressions.

- [ ] **Step 7: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add runtime/src/config/manager.rs runtime/src/ipc/v1/mod.rs \
        runtime/tests/config_manager_tests.rs
git commit -m "feat(runtime): add stream.* SetConfig keys and GetConfig IPC command"
```

---

## Chunk 2: Go TUI

### Task 5: `ConfigValuesMsg` + `GetConfig` IPC client method

**Files:**
- Modify: `tui/internal/ipc/ipc.go`
- Modify: `tui/internal/msg/messages.go`

**Context:** All IPC client methods in `ipc.go` are void — they send a request JSON and return nothing. They wrap the `sendRaw` call in `go func() { ... }()` so they don't block the caller. Responses arrive via the `dispatchUnsolicited()` function (called from `readLoop()`) which switches on `raw.Type` and calls `c.program.Send(SomeMsgType{...})`. `messages.go` contains type aliases: `type FooMsg = ipc.FooMsg`.

Note: `GetProviderSettings` uses a different pattern (request-response with correlation ID via `sendWithID`). For `GetConfig`, use the simpler fire-and-forget pattern: `go func() { sendRaw(...) }()` and handle the response in `dispatchUnsolicited`.

- [ ] **Step 1: Add `ConfigValuesMsg` type to `ipc.go`**

In `tui/internal/ipc/ipc.go`, alongside the other message types (search for `ProviderSettingsMsg` or similar), add:

```go
type ConfigValuesMsg struct {
    Values map[string]any
}
```

- [ ] **Step 2: Add `GetConfig` method to the IPC client**

All fire-and-forget IPC methods wrap the send in a goroutine so they don't block the Bubble Tea update loop. Follow this exact pattern:

```go
func (c *Client) GetConfig(keys []string) {
    go func() {
        _ = c.sendRaw(map[string]any{
            "type": "get_config",
            "keys": keys,
        })
    }()
}
```

- [ ] **Step 3: Handle `config_values` response in `dispatchUnsolicited`**

Find the `dispatchUnsolicited` function in `ipc.go` (called from `readLoop`). It has a `switch raw.Type` block. Insert the new case **inside the switch, before the closing `}` of the switch** (i.e., before any `default:` or `case "error":` if present). Do not add it outside the switch:

```go
case "config_values":
    var payload struct {
        Values map[string]any `json:"values"`
    }
    if err := json.Unmarshal(raw.Raw, &payload); err == nil {
        c.program.Send(ConfigValuesMsg{Values: payload.Values})
    }
```

- [ ] **Step 4: Add type alias to `messages.go`**

In `tui/internal/msg/messages.go`, add:

```go
type ConfigValuesMsg = ipc.ConfigValuesMsg
```

- [ ] **Step 5: Build to verify no compile errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./... 2>&1
```

Expected: clean build.

- [ ] **Step 6: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add tui/internal/ipc/ipc.go tui/internal/msg/messages.go
git commit -m "feat(tui): add GetConfig IPC method and ConfigValuesMsg"
```

---

### Task 6: Remove Go ranking engine from `stream_picker.go`

**Files:**
- Modify: `tui/internal/ui/screens/stream_picker.go`
- Test: `tui/internal/ui/screens/stream_picker_test.go`

**Context:** The code to remove spans approximately lines 70–289 of `stream_picker.go`. Read the file first to confirm exact line ranges. The `qualityRank` map (lines ~76-85) must be **kept** — it is still used for column sort-by-quality ordering. Everything else in that block is removed.

Items to remove/rename:
- `qualityScore()` function — **do NOT delete; rename to `qualityRankOf()` and keep the loop body unchanged.** The function uses `strings.HasPrefix` semantics: quality strings like `"1080p HDR"` must match the `"1080p"` key. A direct map lookup (`qualityRank["1080p hdr"]`) returns 0 (miss). The existing loop is the correct implementation — keep it, just rename the function and remove all callers except `streamLess`.
- `qualityKeys` map — **used by the 1-4 tier-jump quick-key feature; removing it intentionally removes that feature (ranking is now always policy-aware from the runtime)**
- `BestStreamForTier()` function — **also used by the 1-4 tier-jump quick-key feature; removing it intentionally removes that feature**
- The `'1'`–`'4'` key handlers that call `BestStreamForTier()` — remove along with `BestStreamForTier`
- `StreamPolicy` struct
- `defaultStreamPolicy()` function
- Any `loadStreamPolicy()` / `SaveStreamPolicy()` functions
- `scoreStream()` function
- `rankStreams()` function
- `scoredStream` type
- `viewAutoMode()` function (if present — renders the auto-rank status line)
- `policyHints()` function (if present — renders policy hint text in the footer)
- The `'A'` key handler and any `autoRanked`/smart-mode toggle logic on `StreamPickerScreen`

Items to keep:
- `qualityRank` map (for column sort-by-quality lookup only)
- All display/rendering code

After removal, the stream list field on `StreamPickerScreen` should be `[]StreamInfo` (not `[]scoredStream`). Find all references to `scoredStream`, `m.autoRanked`, `m.autoMode`, and `m.policy` and remove them. The streams are already pre-sorted by the runtime.

**Import cleanup:** After removing the above, check whether these imports are now unused and remove them if so: `"encoding/json"`, `"math"`, `"os"`, `"path/filepath"`.

- [ ] **Step 1: Read `stream_picker.go` lines 70-350 to understand the full scope**

Before deleting, read the file to map exactly what references `scoredStream`, `autoRanked`, and `StreamPolicy` throughout the rest of the file (not just lines 70-289). These must all be cleaned up.

- [ ] **Step 2: Run existing tests to establish baseline**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/ -run TestStreamPicker -v 2>&1 | tail -20
```

Note which tests pass currently. There are approximately 6 tests in `stream_picker_test.go` that call `BestStreamForTier()` directly — these will be deleted in Step 3b.

- [ ] **Step 3a: Remove the ranking engine**

In `stream_picker.go`:
- **Rename** `qualityScore()` to `qualityRankOf()` — keep the body unchanged (it does the prefix scan). Remove all callers except `streamLess`.
- **Delete** the `qualityKeys` map (the secondary map for 1-4 tier display strings)
- `BestStreamForTier()` function
- The `'1'`–`'4'` key handlers that call `BestStreamForTier()`
- `StreamPolicy` struct and its methods
- `defaultStreamPolicy()` function
- `loadStreamPolicy()` function
- `SaveStreamPolicy()` function
- `scoreStream()` function
- `rankStreams()` function
- `scoredStream` type
- `viewAutoMode()` function (auto-rank status line renderer)
- `policyHints()` function (policy hint text renderer — calls `streamPolicyPath()`, so remove this first)
- `streamPolicyPath()` function (returns path to `stream_policy.json` — used only by `viewAutoMode`/`policyHints`; keeping it unused leaves `"os"` and `"path/filepath"` imports live)
- The `'A'` key handler and any `autoRanked`/smart-mode toggle logic

Replace `[]scoredStream` field with `[]StreamInfo` on `StreamPickerScreen`. Replace `autoRanked []scoredStream` with nothing (remove). Remove any `autoMode`, `policy`, or `autoRanked` fields from the struct. Update any code that previously called `rankStreams()` or `scoreStream()` — the streams are now used as-is from the IPC response (already ranked by runtime).

Update `streamLess()` (the sort comparator function, around line 946): it currently calls `qualityScore()`. After the rename, update the call to use `qualityRankOf()`:

```go
// Before:
// case sortByQuality:
//     sa, sb := qualityScore(a.Quality), qualityScore(b.Quality)
//     if sa != sb {
//         return sa < sb
//     }
//     return a.Score < b.Score

// After (qualityScore renamed to qualityRankOf, logic unchanged):
case sortByQuality:
    sa, sb := qualityRankOf(a.Quality), qualityRankOf(b.Quality)
    if sa != sb {
        return sa < sb
    }
    return a.Score < b.Score
```

Also update `View()`: it currently guards with `if s.autoRanked != nil { return s.viewAutoMode() }`. After removing `autoRanked` and `viewAutoMode`, simplify `View()` to:

```go
func (s StreamPickerScreen) View() string {
    return s.viewManualMode()
}
```

Also remove the `policy: loadStreamPolicy()` line from `NewStreamPickerScreen()` — after removing the `policy` field from the struct, this line will not compile.

Also update `viewManualMode()`: remove the hint strings `"A auto-pick"` and `"1-4 quality"` from the hint bar it renders. These advertise features that no longer exist. The rest of the hint bar (navigate, play, sort, esc) should be kept.

Remove any imports that become unused: `"encoding/json"`, `"math"`, `"os"`, `"path/filepath"`.

- [ ] **Step 3b: Delete tests for removed functions in `stream_picker_test.go`**

Open `tui/internal/ui/screens/stream_picker_test.go`. Find and delete all test functions that call `BestStreamForTier()`, `scoreStream()`, `rankStreams()`, or reference `StreamPolicy` / `scoredStream`. These tests test code that no longer exists and will cause compile errors. Keeping test coverage for column sorts (if any such tests exist) is fine — only remove tests for the deleted functions.

- [ ] **Step 4: Build to verify no compile errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./... 2>&1
```

Fix any compile errors before proceeding.

- [ ] **Step 5: Run tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/ -v 2>&1 | tail -30
```

Expected: all existing tests pass.

- [ ] **Step 6: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add tui/internal/ui/screens/stream_picker.go \
        tui/internal/ui/screens/stream_picker_test.go
git commit -m "feat(tui): remove client-side stream ranking engine (~200 lines)"
```

---

### Task 7: Add stream preference items to Settings screen

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`
- Modify: `tui/internal/state/app_state.go`
- Test: `tui/internal/ui/screens/settings_test.go`

**Context:** The "Streaming" category is defined in `defaultCategories()` in `settings.go` (around line 611). It has 6 items (`prefer_http`, `auto_fallback`, `max_candidates`, `benchmark_streams`, `auto_delete_video`, `auto_delete_audio`). The 8 new items are appended after these. Use exactly these kind constants: `settingBool`, `settingInt`, `settingChoice` (with `choiceVals`), `settingPath` (for comma-separated text).

`Settings` struct in `app_state.go` needs new fields to mirror the stream preference state.

- [ ] **Step 1: Write failing test**

Add to `tui/internal/ui/screens/settings_test.go`:

```go
func TestStreamingCategoryHasPreferenceItems(t *testing.T) {
    cats := defaultCategories()
    var streaming *settingCategory
    for i := range cats {
        if cats[i].name == "Streaming" {
            streaming = &cats[i]
            break
        }
    }
    if streaming == nil {
        t.Fatal("Streaming category not found")
    }

    wantKeys := []string{
        "stream.preferred_protocol",
        "stream.max_resolution",
        "stream.min_seeders",
        "stream.max_size_mb",
        "stream.prefer_hdr",
        "stream.exclude_cam",
        "stream.avoid_labels",
        "stream.preferred_codecs",
    }
    gotKeys := map[string]bool{}
    for _, item := range streaming.items {
        gotKeys[item.key] = true
    }
    for _, k := range wantKeys {
        if !gotKeys[k] {
            t.Errorf("missing setting item with key %q in Streaming category", k)
        }
    }
}

func TestStreamPreferenceKinds(t *testing.T) {
    cats := defaultCategories()
    var streaming *settingCategory
    for i := range cats {
        if cats[i].name == "Streaming" {
            streaming = &cats[i]
            break
        }
    }

    kindFor := map[string]settingKind{}
    for _, item := range streaming.items {
        kindFor[item.key] = item.kind
    }

    cases := []struct {
        key  string
        kind settingKind
    }{
        {"stream.prefer_hdr", settingBool},
        {"stream.exclude_cam", settingBool},
        {"stream.min_seeders", settingInt},
        {"stream.max_size_mb", settingInt},
        {"stream.preferred_protocol", settingChoice},
        {"stream.max_resolution", settingChoice},
        {"stream.avoid_labels", settingPath},
        {"stream.preferred_codecs", settingPath},
    }
    for _, tc := range cases {
        if kindFor[tc.key] != tc.kind {
            t.Errorf("key %q: want kind %d, got %d", tc.key, tc.kind, kindFor[tc.key])
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/ -run TestStreamingCategory -v 2>&1 | tail -20
go test ./internal/ui/screens/ -run TestStreamPreferenceKinds -v 2>&1 | tail -20
```

Expected: FAIL — items not present yet.

- [ ] **Step 3: Add 8 new items to the "Streaming" category in `defaultCategories()`**

Find the "Streaming" category items slice in `settings.go` and append:

```go
{
    label:       "Preferred protocol",
    key:         "stream.preferred_protocol",
    kind:        settingChoice,
    choiceVals:  []string{"", "http", "torrent"},
    description: "Prefer HTTP direct or torrent streams (blank = no preference)",
},
{
    label:       "Max resolution",
    key:         "stream.max_resolution",
    kind:        settingChoice,
    choiceVals:  []string{"", "sd", "720p", "1080p", "4k"},
    description: "Cap stream resolution (blank = no cap)",
},
{
    label:       "Min seeders",
    key:         "stream.min_seeders",
    kind:        settingInt,
    intVal:      0,
    description: "Skip torrents with fewer seeders than this",
},
{
    label:       "Max size (MB)",
    key:         "stream.max_size_mb",
    kind:        settingInt,
    intVal:      0,
    description: "Deprioritise streams larger than this (0 = no limit)",
},
{
    label:       "Prefer HDR",
    key:         "stream.prefer_hdr",
    kind:        settingBool,
    boolVal:     false,
    description: "Boost HDR streams in ranking",
},
{
    label:       "Exclude CAM",
    key:         "stream.exclude_cam",
    kind:        settingBool,
    boolVal:     true,
    description: "Filter out CAM-quality releases",
},
{
    label:       "Avoid labels",
    key:         "stream.avoid_labels",
    kind:        settingPath,
    strVal:      "",
    description: "Comma-separated labels to deprioritise, e.g. CAM,HDTV",
},
{
    label:       "Preferred codecs",
    key:         "stream.preferred_codecs",
    kind:        settingPath,
    strVal:      "",
    description: "Comma-separated codec preferences, e.g. HEVC,AV1",
},
```

- [ ] **Step 4: Add stream preference fields to `Settings` in `app_state.go`**

In the `Settings` struct, add after the existing fields:

```go
// Stream preferences (mirrors runtime stream.* config)
PreferredProtocol string
MaxResolution     string
MaxSizeMB         int
MinSeeders        int
AvoidLabels       []string
PreferHDR         bool
PreferredCodecs   []string
ExcludeCam        bool
```

In `DefaultSettings()`, add:

```go
ExcludeCam: true,
```

(All other stream preference fields default to zero values, which match `StreamPreferences::default()`.)

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/ -run TestStreamingCategory -v 2>&1 | tail -10
go test ./internal/ui/screens/ -run TestStreamPreferenceKinds -v 2>&1 | tail -10
```

Expected: both PASS.

- [ ] **Step 6: Verify full test suite**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./... 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add tui/internal/ui/screens/settings.go tui/internal/ui/screens/settings_test.go \
        tui/internal/state/app_state.go
git commit -m "feat(tui): add stream preference settings to Streaming category"
```

---

### Task 8: Wire `GetConfig` on startup + `SettingsChangedMsg` cases

**Files:**
- Modify: `tui/internal/ui/ui.go`

**Context:** `ui.go` manages the root Bubble Tea model. `Init()` returns a command that starts the runtime. The `runtimeStartedMsg` handler fires after the runtime is connected — that is the right place to call `GetConfig`. The `SettingsChangedMsg` handler has a switch on `msg.Key` — add `stream.*` cases here. For `avoid_labels` and `preferred_codecs`, the TUI sends a comma-separated string via `settingPath`; they need to be converted to `[]string` before storing in state.

Look at an existing `SettingsChangedMsg` case (e.g. `"downloads.video_dir"`) to see the exact handler pattern.

- [ ] **Step 1: Add `GetConfig` call in `runtimeStartedMsg` handler**

Find the `runtimeStartedMsg` case in `ui.go`'s `Update()` function. It ends with a `return` statement (e.g. `return m, musicInitCmd`). Add the `GetConfig` call **immediately before that return** — code placed after a `return` is unreachable:

```go
// ... existing setup code ...
m.client.GetConfig([]string{
    "stream.preferred_protocol",
    "stream.max_resolution",
    "stream.max_size_mb",
    "stream.min_seeders",
    "stream.avoid_labels",
    "stream.prefer_hdr",
    "stream.preferred_codecs",
    "stream.exclude_cam",
})
return m, musicInitCmd  // (or whatever the existing return is — do not change it)
```

- [ ] **Step 2: Handle `ConfigValuesMsg` to populate settings**

Add a new case to `Update()`'s message switch:

```go
case ipc.ConfigValuesMsg:
    if v, ok := msg.Values["stream.preferred_protocol"].(string); ok {
        m.state.Settings.PreferredProtocol = v
    }
    if v, ok := msg.Values["stream.max_resolution"].(string); ok {
        m.state.Settings.MaxResolution = v
    }
    if v, ok := msg.Values["stream.max_size_mb"].(float64); ok {
        m.state.Settings.MaxSizeMB = int(v)
    }
    if v, ok := msg.Values["stream.min_seeders"].(float64); ok {
        m.state.Settings.MinSeeders = int(v)
    }
    if v, ok := msg.Values["stream.prefer_hdr"].(bool); ok {
        m.state.Settings.PreferHDR = v
    }
    if v, ok := msg.Values["stream.exclude_cam"].(bool); ok {
        m.state.Settings.ExcludeCam = v
    }
    if v, ok := msg.Values["stream.avoid_labels"].([]any); ok {
        strs := make([]string, 0, len(v))
        for _, x := range v {
            if s, ok := x.(string); ok {
                strs = append(strs, s)
            }
        }
        m.state.Settings.AvoidLabels = strs
    }
    if v, ok := msg.Values["stream.preferred_codecs"].([]any); ok {
        strs := make([]string, 0, len(v))
        for _, x := range v {
            if s, ok := x.(string); ok {
                strs = append(strs, s)
            }
        }
        m.state.Settings.PreferredCodecs = strs
    }
    return m, nil
```

Note: JSON numbers unmarshal as `float64` in `map[string]any`.

- [ ] **Step 3: Add `stream.*` cases to `SettingsChangedMsg` handler**

In the existing `SettingsChangedMsg` switch, add (after the `downloads.*` cases):

```go
case "stream.preferred_protocol":
    if v, ok := msg.Value.(string); ok {
        m.state.Settings.PreferredProtocol = v
    }
case "stream.max_resolution":
    if v, ok := msg.Value.(string); ok {
        m.state.Settings.MaxResolution = v
    }
case "stream.max_size_mb":
    if v, ok := msg.Value.(int); ok {
        m.state.Settings.MaxSizeMB = v
    }
case "stream.min_seeders":
    if v, ok := msg.Value.(int); ok {
        m.state.Settings.MinSeeders = v
    }
case "stream.prefer_hdr":
    if v, ok := msg.Value.(bool); ok {
        m.state.Settings.PreferHDR = v
    }
case "stream.exclude_cam":
    if v, ok := msg.Value.(bool); ok {
        m.state.Settings.ExcludeCam = v
    }
**Note on `avoid_labels` and `preferred_codecs`:** Do NOT add `case "stream.avoid_labels":` or `case "stream.preferred_codecs":` to the mirror-state switch — they are intercepted before the switch (see below) and the `return m, nil` in the intercept means the switch is never reached for these two keys.

Note: `settingPath` delivers `string` (the raw text input value). `settingBool` delivers `bool`. `settingInt` delivers `int`. `settingChoice` delivers `string` (the chosen value).

**Pre-existing limitation:** The `SettingsModel` (the active settings screen widget) has its own local display state. On startup it initialises from `DefaultSettings()` defaults before the `ConfigValuesMsg` response arrives. This means the settings screen may briefly show default values until the `ConfigValuesMsg` handler fires. This is a pre-existing limitation of the settings architecture, not introduced by this change — do not attempt to fix it here.

Also note: the existing `m.client.SetConfig(msg.Key, msg.Value)` call (which runs for all non-visualizer keys) automatically forwards these to the runtime — no extra IPC work needed.

For `avoid_labels` and `preferred_codecs`, the SetConfig call needs to send a JSON array, not the comma-separated string. These two keys must be intercepted **before** the generic `m.client.SetConfig(msg.Key, msg.Value)` call. The intercept must also update local state (since the handler returns early, the mirror-state switch below will not run for these keys):

```go
// Insert BEFORE the generic m.client.SetConfig call.
// These two keys must send a []string to the runtime, not the raw comma-separated string.
// Local state is also updated here because the early return skips the mirror-state switch below.
if msg.Key == "stream.avoid_labels" || msg.Key == "stream.preferred_codecs" {
    if v, ok := msg.Value.(string); ok {
        var arr []string
        if v != "" {
            parts := strings.Split(v, ",")
            for i := range parts {
                parts[i] = strings.TrimSpace(parts[i])
            }
            arr = parts
        }
        m.client.SetConfig(msg.Key, arr) // sends []string to runtime
        if msg.Key == "stream.avoid_labels" {
            m.state.Settings.AvoidLabels = arr
        } else {
            m.state.Settings.PreferredCodecs = arr
        }
        return m, nil // skip generic SetConfig and mirror-state switch below
    }
}
// Generic path for all other keys (bool, int, choice):
m.client.SetConfig(msg.Key, msg.Value)
```

The `SettingsChangedMsg` handler structure is:
1. Visualizer prefix guard (`if strings.HasPrefix(msg.Key, "visualizer.")`) — returns early for visualizer keys
2. Generic `m.client.SetConfig(msg.Key, msg.Value)` call — fires for all non-visualizer keys
3. `switch msg.Key` mirror-state block — updates local state

Insert the intercept **between step 1 and step 2** (i.e., after the visualizer guard's closing `}` and before the `if m.client != nil { m.client.SetConfig(...) }` line). Do NOT insert it before the visualizer block — that would incorrectly intercept visualizer keys. The `return m, nil` is critical — without it the generic call in step 2 sends the comma-separated string as a second SetConfig, which the runtime will reject.

- [ ] **Step 4: Build to verify no compile errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./... 2>&1
```

- [ ] **Step 5: Run full test suite**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./... 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Step 6: Build the Rust side too**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo build -p stui-runtime 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 7: Commit**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
git add tui/internal/ui/ui.go
git commit -m "feat(tui): wire GetConfig on startup and stream.* SettingsChangedMsg cases"
```

---

## Final Verification

- [ ] **Run all Rust tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo test -p stui-runtime 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Run all Go tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./... 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Full build**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui"
cargo build 2>&1 | tail -5
cd tui && go build ./... 2>&1
```

Expected: both clean.

- [ ] **Manual smoke test**

Start the application (or connect to a running runtime). Navigate to Settings → Streaming category. Verify:
1. The 8 new stream preference items appear after the existing Streaming items: "Preferred protocol", "Max resolution", "Min seeders", "Max size (MB)", "Prefer HDR", "Exclude CAM", "Avoid labels", "Preferred codecs".
2. Change "Min seeders" from 0 to 5 and navigate away — the setting should persist (runtime accepts the SetConfig and returns it on next GetConfig).
3. Open the stream picker for any title — streams should be displayed without errors (no crash from removed ranking code).
4. Confirm the sort-by-quality column sort still works via the tab key.
