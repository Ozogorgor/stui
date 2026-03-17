# Stream Ranking Migration — Design Spec

**Date:** 2026-03-17
**Status:** Approved

---

## Overview

Move stream quality ranking and user stream preferences from the Go TUI into the Rust runtime. The TUI currently contains a ~200-line scoring engine (`scoreStream`, `rankStreams`, `StreamPolicy`) that duplicates ranking logic already present in `runtime/src/quality/`. After this change the runtime owns all ranking; the TUI receives pre-ranked results and renders them.

**Boundary principle:** The TUI is a renderer. All business logic — including stream selection policy — belongs in the runtime.

---

## Data Model

### New: `StreamPreferences` in `runtime/src/config/types.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPreferences {
    pub preferred_protocol: Option<String>, // "http" | "torrent" | None (no preference)
    pub max_resolution:     Option<String>, // "4k" | "1080p" | "720p" | "sd" | None
    pub max_size_mb:        Option<u64>,    // None = no limit
    pub min_seeders:        u32,            // default 0
    pub avoid_labels:       Vec<String>,    // e.g. ["CAM", "HDTV"]
    pub prefer_hdr:         bool,           // default false
    pub preferred_codecs:   Vec<String>,    // e.g. ["HEVC", "AV1"]
    pub seeder_weight:      f64,            // default 1.0 (config-file only; not in Settings UI)
    pub exclude_cam:        bool,           // default true
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

`RuntimeConfig` gains:

```rust
pub stream: StreamPreferences,
```

The `stui.toml` gains a `[stream]` section (all fields optional; defaults used when absent).

### Conversion: `From<&StreamPreferences> for RankingPolicy`

Added to `runtime/src/quality/policy.rs`. The quality module imports `StreamPreferences` from `config::types` (quality → config dependency, not the reverse):

```rust
use crate::config::types::StreamPreferences;

impl From<&StreamPreferences> for RankingPolicy {
    fn from(prefs: &StreamPreferences) -> Self {
        let resolution_weights = match prefs.max_resolution.as_deref() {
            Some("sd")    => [100,   0,   0,   0],
            Some("720p")  => [100, 200,   0,   0],
            Some("1080p") => [100, 200, 300,   0],
            _             => [100, 200, 300, 400], // None or "4k" = no cap
        };
        RankingPolicy {
            resolution_weights,
            prefer_lower_resolution: false,
            seeder_weight: prefs.seeder_weight,
            exclude_cam:   prefs.exclude_cam,
            min_seeders:   prefs.min_seeders,
        }
    }
}
```

Protocol preference, avoid_labels, prefer_hdr, preferred_codecs, and max_size_mb are applied as a **post-rank adjustment** in the engine (see below) after `quality::rank()` returns, since `RankingPolicy` does not currently model these dimensions.

---

## IPC Protocol Changes

### New request variant: `GetConfig`

Added to the `Request` enum in `runtime/src/ipc/v1/mod.rs` (follows existing `#[serde(tag = "type", rename_all = "snake_case")]` — serialises as `"type": "get_config"`):

```rust
GetConfig {
    keys: Vec<String>, // dot-notation keys; empty = return all
},
```

### New response variant: `ConfigValues`

```rust
ConfigValues {
    values: std::collections::HashMap<String, serde_json::Value>,
},
```

Serialises as `"type": "config_values"`.

### `SetConfig` key namespace: `stream.*`

| Key | Type | Default |
|-----|------|---------|
| `stream.preferred_protocol` | `string \| null` | `null` |
| `stream.max_resolution` | `string \| null` | `null` |
| `stream.max_size_mb` | `number \| null` | `null` |
| `stream.min_seeders` | `number` | `0` |
| `stream.avoid_labels` | JSON `string[]` | `[]` |
| `stream.prefer_hdr` | `bool` | `false` |
| `stream.preferred_codecs` | JSON `string[]` | `[]` |
| `stream.seeder_weight` | `number` | `1.0` |
| `stream.exclude_cam` | `bool` | `true` |

No wire format changes to `StreamInfoWire` or `StreamsResult`. Streams are returned in ranked order; the `score` field continues to carry the composite quality score.

---

## GetStreams Handler

**Files:** `runtime/src/ipc/v1/mod.rs` (handler), `runtime/src/engine/mod.rs` (ranking call site and `post_rank_adjust`)

New flow:

```
GetStreams
  → config_manager.snapshot().stream          // reads StreamPreferences from current config
  → RankingPolicy::from(&prefs)               // convert to internal policy
  → engine.resolve(entry_id)                  // fetch raw candidates
  → quality::rank(candidates, policy)         // rank by quality + policy weights
  → post_rank_adjust(&mut candidates, &prefs) // protocol, size, labels, codecs, HDR
  → candidate.badge() called per candidate    // badges regenerated after score adjustments
  → StreamsResult (pre-ranked Vec<StreamInfoWire>)
```

`post_rank_adjust` is a private function in `runtime/src/engine/mod.rs`:

```rust
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
        // HDR preference bonus (hdr field is HdrFormat enum, not bool)
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

**Badge regeneration note:** `StreamCandidate::badge()` embeds `score.total()`. Because `post_rank_adjust` mutates scores, badges must be generated *after* this function runs — not before. The `StreamInfoWire` construction loop in the handler calls `candidate.badge()` last.

---

## ConfigManager Changes

**File:** `runtime/src/config/manager.rs`

- `SetConfig` handler gains cases for all `stream.*` keys, parsing each into the appropriate field on `RuntimeConfig.stream`. Uses `config_manager.snapshot()` to read then `config_manager.update(...)` to write (following the existing pattern).
- `GetConfig` handler calls `config_manager.snapshot()`, serialises each requested key to `serde_json::Value` via `serde_json::to_value`, and returns a `ConfigValues` response. Empty `keys` vec returns all keys.
- Array fields (`avoid_labels`, `preferred_codecs`) accept a JSON-encoded string array on the wire: `["CAM","HDTV"]`.

---

## TUI Changes

### `tui/internal/ui/screens/stream_picker.go`

**Remove:**
- `qualityScore()` function
- `scoreStream()` function (~75 lines)
- `rankStreams()` function
- `BestStreamForTier()` function
- `StreamPolicy` struct
- `loadStreamPolicy()` and `SaveStreamPolicy()` functions
- `scoredStream` type — replaced by `StreamInfo` directly (score field already present)
- `autoRanked` field and smart-mode toggle logic on `StreamPickerScreen`

**Keep (column sorts):** The tab-key column sort cycle remains. Each sort is a simple field comparison on `StreamInfo` (no score computation):
- Sort by quality: keep `qualityRank` map **for sort-order lookup only** (not for scoring)
- Sort by seeders, size, provider: direct field comparisons
- Sort by score: compare `stream.Score` (computed by runtime)

**Net removal:** ~200 lines of scoring/policy logic.

### `tui/internal/ui/screens/settings.go`

The new preference items are added to the **existing "Streaming" category** (not a new category — a "Stream" category would conflict with the existing "Streaming" name and duplicate the `⚡` icon). Items are appended after the existing "Streaming" items:

```go
// Appended to the existing "Streaming" category items:
{label: "Preferred protocol", key: "stream.preferred_protocol",
    kind: settingChoice, choiceVals: []string{"", "http", "torrent"},
    description: "Prefer HTTP direct or torrent streams (blank = no preference)"},
{label: "Max resolution",     key: "stream.max_resolution",
    kind: settingChoice, choiceVals: []string{"", "sd", "720p", "1080p", "4k"},
    description: "Cap stream resolution (blank = no cap)"},
{label: "Min seeders",        key: "stream.min_seeders",
    kind: settingInt, intVal: 0,
    description: "Skip torrents with fewer seeders than this"},
{label: "Max size (MB)",      key: "stream.max_size_mb",
    kind: settingInt, intVal: 0,
    description: "Deprioritise streams larger than this (0 = no limit)"},
{label: "Prefer HDR",         key: "stream.prefer_hdr",
    kind: settingBool, boolVal: false,
    description: "Boost HDR streams in ranking"},
{label: "Exclude CAM",        key: "stream.exclude_cam",
    kind: settingBool, boolVal: true,
    description: "Filter out CAM-quality releases"},
{label: "Avoid labels",       key: "stream.avoid_labels",
    kind: settingPath, strVal: "",
    description: "Comma-separated labels to deprioritise, e.g. CAM,HDTV"},
{label: "Preferred codecs",   key: "stream.preferred_codecs",
    kind: settingPath, strVal: "",
    description: "Comma-separated codec preferences, e.g. HEVC,AV1"},
```

`avoid_labels` and `preferred_codecs` use `settingPath` for its inline text editor. The `displayValue()` path-relative logic (`filepath.Rel`) will produce an error for non-path strings and fall through to the raw value — the display is correct. The semantic mismatch is acceptable for this use case; no new kind is introduced (YAGNI).

`seeder_weight` is **not** in the Settings UI — it is a config-file-only advanced option. Advanced users set it via `stui.toml`.

### `tui/internal/ipc/ipc.go`

New method, following the existing fire-and-forget async pattern (no return value; dispatches result via `c.program.Send()`):

```go
func (c *Client) GetConfig(keys []string) {
    // sends GetConfig request; result arrives as ConfigValuesMsg via program.Send()
}
```

A new `ConfigValuesMsg` type is added to `internal/msg/messages.go`:

```go
type ConfigValuesMsg struct {
    Values map[string]any
}
```

### `tui/internal/ui/ui.go`

- `Init()` calls `client.GetConfig([]string{"stream.preferred_protocol", "stream.max_resolution", "stream.max_size_mb", "stream.min_seeders", "stream.avoid_labels", "stream.prefer_hdr", "stream.preferred_codecs", "stream.exclude_cam"})`.
- A new `ConfigValuesMsg` handler populates the stream preference fields in `SettingsModel` from the response, overwriting `DefaultSettings()` values.
- `SettingsChangedMsg` handler gains cases for all `stream.*` keys (same pattern as existing settings). `avoid_labels` and `preferred_codecs` arrive as comma-separated strings from the TUI and are serialised to JSON arrays before `SetConfig` is called.

### `tui/internal/state/app_state.go`

Stream preference fields added to `Settings` for local mirror (display state):

```go
// Stream preferences
PreferredProtocol string
MaxResolution     string
MaxSizeMB         int
MinSeeders        int
AvoidLabels       []string
PreferHDR         bool
PreferredCodecs   []string
ExcludeCam        bool
// SeederWeight is config-file only; not mirrored in TUI state
```

`DefaultSettings()` initialises these to the same defaults as `StreamPreferences::default()` in Rust. On startup, the `ConfigValuesMsg` handler overwrites them with runtime truth.

---

## Files Changed

| File | Change |
|------|--------|
| `runtime/src/config/types.rs` | Add `StreamPreferences` struct; add `stream` field to `RuntimeConfig` |
| `runtime/src/config/manager.rs` | Handle `stream.*` keys in `SetConfig`; implement `GetConfig` handler using `snapshot()` |
| `runtime/src/ipc/v1/mod.rs` | Add `GetConfig` request variant; add `ConfigValues` response variant |
| `runtime/src/quality/policy.rs` | Add `impl From<&StreamPreferences> for RankingPolicy`; import `StreamPreferences` from `config::types` |
| `runtime/src/engine/mod.rs` | Add `post_rank_adjust()` private function; wire into `GetStreams` handler flow |
| `tui/internal/msg/messages.go` | Add `ConfigValuesMsg` type |
| `tui/internal/ui/screens/stream_picker.go` | Remove ranking engine (~200 lines); remove `StreamPolicy`; simplify column sorts |
| `tui/internal/ui/screens/settings.go` | Append 8 stream preference items to existing "Streaming" category |
| `tui/internal/ui/ui.go` | Add `stream.*` cases to `SettingsChangedMsg` handler; call `GetConfig` on init; handle `ConfigValuesMsg` |
| `tui/internal/ipc/ipc.go` | Add `GetConfig` async method |
| `tui/internal/state/app_state.go` | Add stream preference fields to `Settings` |

## Files Unchanged

| File | Reason |
|------|--------|
| `runtime/src/quality/score.rs` | Scoring algorithm untouched |
| `runtime/src/quality/candidate.rs` | `badge()` generation untouched; called after `post_rank_adjust` by handler |
| `runtime/src/quality/mod.rs` | `rank()` / `rank_with_health()` signatures untouched |
| `tui/pkg/streambench/bench.go` | Out of scope (stream benchmarking is a separate migration item) |

---

## Migration Notes

- `~/.config/stui/stream_policy.json` — silently ignored after upgrade (file remains on disk but is no longer read). No migration needed; defaults are equivalent.
- The `autoRanked` smart-mode toggle in the stream picker is removed without replacement — ranking is always policy-aware after this change.
- `seeder_weight` is not surfaced in the Settings UI — users who previously relied on this field must set it in `stui.toml` under `[stream]`.

---

## Testing

**Runtime:**
- `StreamPreferences::default()` converts to a `RankingPolicy` equivalent to `RankingPolicy::default()`
- `From<&StreamPreferences>` with `max_resolution = "1080p"` sets 4K weight to 0
- `post_rank_adjust()` correctly deprioritises streams matching `avoid_labels`
- `post_rank_adjust()` correctly penalises non-matching protocol
- `post_rank_adjust()` does not apply HDR bonus when `c.stream.hdr == HdrFormat::None`
- `GetConfig` returns current serialised values for requested `stream.*` keys
- `SetConfig("stream.min_seeders", 5)` updates `RuntimeConfig.stream.min_seeders`

**TUI:**
- Settings "Streaming" category contains all 8 new items appended after existing items
- Changing a `settingBool` / `settingChoice` / `settingInt` item fires `SettingsChangedMsg` with correct `stream.*` key
- `ConfigValuesMsg` on startup populates stream preference fields (not hardcoded defaults)
- Existing `settings_test.go` continues to pass unchanged

**Build:**
- `cargo build` clean after runtime changes
- `go build ./...` clean after TUI changes
- `go test ./...` passes
