# Subtitle pipeline — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the SDK validator gap for subtitle-only manifests, migrate the three remaining subtitle plugins (`kitsunekko`, `subscene`, `yify-subs`) to the current SDK, and wire subtitle-capability plugins into the play path so external subtitles auto-download and land in mpv.

**Architecture:** One-line SDK default flip (Enabled(false)) unblocks validation. Three plugin migrations follow the opensubtitles Approach B template. Runtime gains a new `engine::subtitles` module; `PlayerBridge::play` grows a subtitle-fetch prelude that writes into the existing sidecar layout so `find_subtitle` picks up the result.

**Tech Stack:** Rust (runtime + plugins + SDK), Go (TUI toast), `tokio::time::timeout` for the 5s fetch cap, existing aria2 bridge for downloads.

**Design spec:** [2026-04-23-subtitle-pipeline-design.md](../specs/2026-04-23-subtitle-pipeline-design.md)

---

## Commit policy

Per the prior session's precedent: no commits until all chunks land. **Two commits in Chunk 6 — one per repo.** `stui` monorepo commit in Task 6.2; `stui_plugins` commit + `v0.3.0` tag in Task 6.3.

---

## File structure

### Rust changes

| File | Responsibility | Chunk |
|---|---|---|
| `stui/sdk/src/manifest.rs` | Flip `CatalogCapability::default()` to `Enabled(false)` | 1 |
| `stui_plugins/sdk/src/manifest.rs` | Same flip (duplicated SDK) | 1 |
| `stui_plugins/opensubtitles-provider/tests/manifest_parses.rs` | Enable `validate` assertion (gap is now closed) | 1 |
| `stui_plugins/kitsunekko/plugin.toml` | Full canonical rewrite (like opensubtitles) | 2 |
| `stui_plugins/kitsunekko/src/lib.rs` | Approach B migration | 2 |
| `stui_plugins/kitsunekko/tests/manifest_parses.rs` | New — parse + validate | 2 |
| `stui_plugins/subscene/plugin.toml` | Same | 3 |
| `stui_plugins/subscene/src/lib.rs` | Same | 3 |
| `stui_plugins/subscene/tests/manifest_parses.rs` | New | 3 |
| `stui_plugins/yify-subs/plugin.toml` | Same | 4 |
| `stui_plugins/yify-subs/src/lib.rs` | Same | 4 |
| `stui_plugins/yify-subs/tests/manifest_parses.rs` | New | 4 |
| `stui_plugins/.github/workflows/release.yml` | Matrix adds three plugins | 4 |
| `stui/runtime/src/engine/subtitles.rs` | New module: `SubtitleCandidate` + `fetch_subtitles` | 5 |
| `stui/runtime/src/engine/mod.rs` | `mod subtitles;` + re-export | 5 |
| `stui/runtime/src/ipc/v1/mod.rs` | New `SubtitleFetchedEvent` + `SubtitleSearchFailedEvent` | 5 |
| `stui/runtime/src/ipc/mod.rs` | Re-exports for new events | 5 |
| `stui/runtime/src/player/bridge.rs` | Subtitle-fetch prelude before `find_subtitle` | 5 |

### TUI changes

| File | Responsibility | Chunk |
|---|---|---|
| `stui/tui/internal/ipc/messages.go` | `SubtitleFetchedMsg`, `SubtitleSearchFailedMsg` types | 5 |
| `stui/tui/internal/ipc/internal.go` | Route incoming events into the toast pipeline | 5 |

---

## Chunk 1: SDK default flip + opensubtitles test unblock

### Task 1.1: Flip the SDK default in both SDK copies

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/sdk/src/manifest.rs` (around line 63-75, search for `impl Default for CatalogCapability`)
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/sdk/src/manifest.rs` (same function)

- [ ] **Step 1: Read the current `impl Default for CatalogCapability` in both files**

```bash
grep -n -A10 "impl Default for CatalogCapability" /home/ozogorgor/Projects/Stui_Project/stui/sdk/src/manifest.rs
grep -n -A10 "impl Default for CatalogCapability" /home/ozogorgor/Projects/Stui_Project/stui_plugins/sdk/src/manifest.rs
```

Expected: both files show the current `Typed { kinds: Vec::new(), search: None, ... }` default.

- [ ] **Step 2: Replace with `Enabled(false)` in both files**

Exact replacement (apply verbatim in both SDKs):

```rust
impl Default for CatalogCapability {
    fn default() -> Self {
        // Subtitle-only and stream-only plugins declare no
        // [capabilities.catalog] block, so serde hits this default. Returning
        // `Enabled(false)` is an explicit "no catalog capability" and skips
        // the validator's typed-catalog branch (which requires
        // `search: Some(true)` for the Typed variant). Plugins that DO declare
        // a typed catalog block override this at deserialize time, so this
        // change does not affect existing metadata plugins.
        Self::Enabled(false)
    }
}
```

- [ ] **Step 3: Verify monorepo plugins still validate**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/plugins && \
    cargo build --workspace --target wasm32-wasip1 --release 2>&1 | tail -5
```

Expected: all 7 bundled metadata plugins (tmdb/omdb/anilist/kitsu/discogs/lastfm/musicbrainz) build cleanly. They declare explicit `[capabilities.catalog]` so the default change doesn't affect them.

- [ ] **Step 4: Verify opensubtitles now validates against the patched SDK**

Write the test first (in Task 1.2 below), but the verification command is just `cargo test`:

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo test -p opensubtitles-provider --test manifest_parses 2>&1 | tail -10
```

At this point the existing opensubtitles test passes (SDK default change doesn't break it; test doesn't yet call `validate`). Task 1.2 adds the validate call.

### Task 1.2: Add validate_manifest assertion to the opensubtitles test

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/opensubtitles-provider/tests/manifest_parses.rs`

- [ ] **Step 1: Read the current test**

```bash
cat /home/ozogorgor/Projects/Stui_Project/stui_plugins/opensubtitles-provider/tests/manifest_parses.rs
```

Note the TODO-style comment block explaining why `validate_manifest` was deliberately absent (the SDK gap Chunk 1 just closed).

- [ ] **Step 2: Rewrite the test to use validate_manifest**

Replace the file content with:

```rust
use stui_plugin_sdk::parse_manifest;

#[test]
fn plugin_toml_parses() {
    let m = parse_manifest(include_str!("../plugin.toml"))
        .expect("plugin.toml parses");
    assert_eq!(m.plugin.name, "opensubtitles-provider");
    assert!(!m.capabilities.streams);
    // `.kinds()` accessor method, NOT `.kinds` field — CatalogCapability
    // is an untagged enum. With the SDK default now Enabled(false),
    // `.kinds()` is still `&[]` for subtitle-only plugins.
    assert!(m.capabilities.catalog.kinds().is_empty());
    assert!(
        m.capabilities._extra.contains_key("subtitles"),
        "subtitles capability not parsed",
    );
    stui_plugin_sdk::validate_manifest(&m)
        .expect("manifest validates under the current SDK schema");
}
```

- [ ] **Step 3: Run the test**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo test -p opensubtitles-provider --test manifest_parses 2>&1 | tail -10
```

Expected: 1 passed. If validate fails with `MissingRequiredVerb("search")` — Task 1.1 was not applied correctly.

---

## Chunk 2: Migrate kitsunekko

### Task 2.1: Read kitsunekko and note its plugin-specific shape

**Files:**
- Read only: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/src/lib.rs`
- Read only: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/plugin.toml`

- [ ] **Step 1: Read the source to understand its scraping pattern**

```bash
cat /home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/src/lib.rs
```

Note:
- The URL shape it scrapes (likely `https://kitsunekko.net/dirlist.php?dir=...`).
- How it parses the HTML response (regex, `scraper` crate, or string search).
- What `req.tab` branches exist (kitsunekko is anime-only — the branches may map to different category directories).
- How entry IDs are packed in `PluginEntry.id` for resolve to use.
- Any plugin-specific helpers (auth headers, URL encoding quirks).

### Task 2.2: Replace kitsunekko/plugin.toml

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/plugin.toml`

- [ ] **Step 1: Overwrite the manifest**

Replace with:

```toml
# kitsunekko — plugin manifest
# Anime subtitle scraper for kitsunekko.net

[plugin]
name        = "kitsunekko"
version     = "0.1.0"
abi_version = 1
description = "Anime subtitle scraper for kitsunekko.net"
tags        = ["subtitles", "anime"]

[capabilities.subtitles]
# Opaque capability — _extra HashMap catch-all. Runtime dispatch path:
# engine::subtitles::fetch_subtitles → stui_search → stui_resolve.

[permissions]
network = ["kitsunekko.net", "www.kitsunekko.net"]

[rate_limit]
requests_per_second = 1
burst               = 3

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

If the existing manifest has custom `[env]`, `[[config]]`, or additional permissions, preserve them between `[capabilities.subtitles]` and `[meta]`.

### Task 2.3: Migrate kitsunekko/src/lib.rs — Approach B

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/src/lib.rs`

Reference template: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/opensubtitles-provider/src/lib.rs`.

- [ ] **Step 1: Add explicit SDK imports below the existing `use stui_plugin_sdk::prelude::*;`**

```rust
use stui_plugin_sdk::{
    parse_manifest, PluginManifest,
    Plugin, CatalogPlugin,
    EntryKind, SearchScope,
};
```

- [ ] **Step 2: Replace the struct + Default**

```rust
pub struct Kitsunekko {
    manifest: PluginManifest,
}

impl Default for Kitsunekko {
    fn default() -> Self {
        Self {
            manifest: parse_manifest(include_str!("../plugin.toml"))
                .expect("plugin.toml failed to parse at compile time"),
        }
    }
}
```

(If the existing struct has a different name, e.g. `KitsunekkoProvider`, preserve the original name throughout.)

- [ ] **Step 3: Add `Plugin` impl**

```rust
impl Plugin for Kitsunekko {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
}
```

- [ ] **Step 4: Replace the existing `impl StuiPlugin` with `impl CatalogPlugin` (real work) + stubbed `impl StuiPlugin`**

```rust
impl CatalogPlugin for Kitsunekko {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // Kitsunekko is anime-only. Movie/Series/Episode all fetch anime
        // subtitles; Track/Artist/Album are nonsensical here.
        let kind = match req.scope {
            SearchScope::Series | SearchScope::Episode => EntryKind::Series,
            SearchScope::Movie => EntryKind::Movie,
            _ => {
                return PluginResult::err(
                    "UNSUPPORTED_SCOPE",
                    "kitsunekko only supports movie and series/episode scopes",
                );
            }
        };

        // ... port the existing scraping body verbatim. Replace any
        //     `req.tab.as_str()` branching with `req.scope` mapping.
        //     Map each scraped candidate into `PluginEntry { id, kind, title,
        //     description: Some(...), ..Default::default() }`.
    }
    // lookup / enrich / get_artwork / get_credits / related use trait defaults.
}

// `StuiPlugin` is deprecated in favor of `Plugin + CatalogPlugin`, but
// `stui_export_plugin!` still requires it for the `stui_resolve` ABI
// export. This block goes away when the subtitle/stream ABIs land and
// the macro drops its `$plugin_ty: StuiPlugin` bound.
#[allow(deprecated)]
impl StuiPlugin for Kitsunekko {
    fn name(&self) -> &str { &self.manifest.plugin.name }
    fn version(&self) -> &str { &self.manifest.plugin.version }
    fn plugin_type(&self) -> PluginType { PluginType::Subtitle }

    fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
        PluginResult::err("LEGACY_UNUSED", "search dispatches via CatalogPlugin")
    }

    fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse> {
        // ... keep verbatim. Returns ResolveResponse { stream_url: <srt_url>,
        //     quality: None, subtitles: vec![] }.
    }
}
```

- [ ] **Step 5: Ensure `stui_export_plugin!(Kitsunekko);` remains at the bottom**

No change.

- [ ] **Step 6: Grep for `req.tab` — expect empty**

```bash
grep -n "req\.tab" /home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/src/lib.rs
```

### Task 2.4: Add manifest parse test

**Files:**
- Create: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/kitsunekko/tests/manifest_parses.rs`

- [ ] **Step 1: Create the test file**

```rust
use stui_plugin_sdk::parse_manifest;

#[test]
fn plugin_toml_parses() {
    let m = parse_manifest(include_str!("../plugin.toml"))
        .expect("plugin.toml parses");
    assert_eq!(m.plugin.name, "kitsunekko");
    assert!(!m.capabilities.streams);
    assert!(m.capabilities.catalog.kinds().is_empty());
    assert!(
        m.capabilities._extra.contains_key("subtitles"),
        "subtitles capability not parsed",
    );
    stui_plugin_sdk::validate_manifest(&m)
        .expect("manifest validates under the current SDK schema");
}
```

### Task 2.5: Compile + test

- [ ] **Step 1: Build kitsunekko for wasm**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo build -p kitsunekko --target wasm32-wasip1 --release 2>&1 | tail -10
```

- [ ] **Step 2: Run test**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo test -p kitsunekko --test manifest_parses 2>&1 | tail -5
```

---

## Chunk 3: Migrate subscene

**Same structure as Chunk 2.** Substitute `kitsunekko` → `subscene`, `Kitsunekko` → `Subscene` (or the existing struct name).

### Task 3.1: Read subscene source to note plugin-specific shape

```bash
cat /home/ozogorgor/Projects/Stui_Project/stui_plugins/subscene/src/lib.rs
```

Note subscene-specific scraping URL shape, permissions, config.

### Task 3.2: Replace subscene/plugin.toml

Canonical shape like kitsunekko's but with `description = "Subtitle scraper for subscene.com"` and `network = ["subscene.com", "www.subscene.com"]`. Preserve any existing `[env]` / `[[config]]` between `[capabilities.subtitles]` and `[meta]`.

### Task 3.3: Migrate subscene/src/lib.rs — Approach B

Apply Task 2.3 Steps 1-6 verbatim, substituting names. Scope mapping for subscene is the same as kitsunekko (movie/series only).

### Task 3.4: Add manifest parse test

Create `stui_plugins/subscene/tests/manifest_parses.rs` asserting `m.plugin.name == "subscene"`.

### Task 3.5: Compile + test

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo build -p subscene --target wasm32-wasip1 --release 2>&1 | tail -10
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo test -p subscene --test manifest_parses 2>&1 | tail -5
```

---

## Chunk 4: Migrate yify-subs + extend CI matrix

### Task 4.1: Read yify-subs source

```bash
cat /home/ozogorgor/Projects/Stui_Project/stui_plugins/yify-subs/src/lib.rs
```

Note yify-subs-specific URL shape. YIFY is a movie-release-focused site — scope mapping may prefer returning `UNSUPPORTED_SCOPE` for Series too if the source doesn't index series subs.

### Task 4.2: Replace yify-subs/plugin.toml

Canonical shape with `description = "Subtitle scraper for YIFY releases"`, appropriate `network` hosts.

### Task 4.3: Migrate yify-subs/src/lib.rs — Approach B

Apply Task 2.3 Steps 1-6 verbatim, substituting names. If yify-subs is movie-only, the scope mapping should only accept `SearchScope::Movie` and error on Series/Episode too.

### Task 4.4: Add manifest parse test

Create `stui_plugins/yify-subs/tests/manifest_parses.rs`. Note: the crate name has a hyphen that becomes underscore in Rust (`yify_subs`), so the test path is relative to the plugin dir.

### Task 4.5: Compile + test

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo build -p yify-subs --target wasm32-wasip1 --release 2>&1 | tail -10
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    cargo test -p yify-subs --test manifest_parses 2>&1 | tail -5
```

### Task 4.6: Extend CI matrix

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui_plugins/.github/workflows/release.yml`

- [ ] **Step 1: Add kitsunekko, subscene, yify-subs to the matrix after opensubtitles-provider**

The matrix should end up like:

```yaml
plugin:
  - example-provider
  - example-resolver
  - jackett-provider
  - prowlarr-provider
  - opensubtitles-provider
  - kitsunekko
  - subscene
  - yify-subs
  # Streaming services stay commented — separate session.
  # - spotify
  # ...
```

- [ ] **Step 2: Verify YAML parses**

```bash
python3 -c "import yaml; yaml.safe_load(open('/home/ozogorgor/Projects/Stui_Project/stui_plugins/.github/workflows/release.yml'))"
```

---

## Chunk 5: Runtime dispatch + player integration + TUI toast

Verified runtime facts (do not re-derive):
- `Engine` at `runtime/src/engine/mod.rs:402` derives `Clone` — `engine.clone()` in JoinSet tasks is cheap (internal state is `Arc`-wrapped).
- Runtime uses `crate::abi::SearchRequest` / `abi::ResolveRequest` / `abi::PluginEntry` (defined in `runtime/src/abi/types.rs`) for host-side plugin dispatch — NOT `stui_plugin_sdk::SearchRequest`. The two are structurally identical but are distinct Rust types at the host boundary. Follow the existing pattern at `Engine::supervisor_search` (`runtime/src/engine/mod.rs:522`).
- `PlayerBridge` (at `runtime/src/player/bridge.rs:78`) fields are `mpv, aria2, mpd, engine, storage, watch_history, ipc_tx: mpsc::Sender<String>, data_dir: String, playback_cfg, dsp, mpd_active`. **No `config` field, no `event_tx`.** Outbound events are serialized to JSON strings and pushed through `ipc_tx`, modeled on the `download_started` pattern at `runtime/src/player/bridge.rs:186-192`.
- `PlayerBridge::play` signature at `runtime/src/player/bridge.rs:144`:
  `pub async fn play(&self, entry_id: &str, provider: &str, imdb_id: &str, tab: Option<MediaTab>, media_type: Option<MediaType>, year: Option<u32>)`.
  There is no structured `entry` arg. Title is derived inline as `entry_id.split('|').next().unwrap_or(entry_id)`.
- `find_subtitle(data_dir, imdb_id)` is called at `runtime/src/player/bridge.rs:167` and the result is passed to `start_stream` on line 168. The subtitle-fetch prelude inserts on lines 166-167 (between the audio-early-return block and the `find_subtitle` call).
- `Event` enum at `runtime/src/ipc/v1/stream.rs:33` currently has only `Event::ScopeResults(ScopeResultsMsg)`. Out-of-band events that aren't `ScopeResults` (like `PluginToastEvent`, `download_started`) flow as ad-hoc JSON through `ipc_tx`, not through the `Event` enum. The subtitle events follow the ad-hoc-JSON pattern.

### Task 5.0: Plumb `ConfigManager` into `PlayerBridge`

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/runtime/src/player/bridge.rs`
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/runtime/src/main.rs` (the `PlayerBridge::new(...)` call at line 401)

The subtitle prelude in Task 5.3 reads `cfg.subtitles.auto_download` and `cfg.subtitles.preferred_language`. `PlayerBridge` currently has no config handle, so this task plumbs one in.

- [ ] **Step 1: Add `config: Arc<ConfigManager>` to `PlayerBridge` struct**

In `runtime/src/player/bridge.rs:78`, add a field immediately after `engine: Arc<Engine>`:

```rust
config:       Arc<crate::config::ConfigManager>,
```

Add the corresponding parameter to `PlayerBridge::new` (line 96) after the `engine` param and threadthrough to the struct literal at line 116.

- [ ] **Step 2: Pass `config` at the call site**

In `runtime/src/main.rs`, the `PlayerBridge::new(...)` call starting at line 401 currently passes:

```rust
player::PlayerBridge::new(
    Arc::clone(&engine),
    aria2.clone(),
    ...
)
```

Add `Arc::clone(&config)` as the second argument (immediately after `engine`). The `config` symbol already exists in scope — search for `let config = Arc::new(ConfigManager::...)` earlier in `main.rs` to confirm the variable name; it's typically `config` or `config_manager`.

- [ ] **Step 3: Build to verify no unrelated breakage**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui && \
    cargo build --release -p stui-runtime 2>&1 | tail -5
```

Expected: `Finished`. If the variable name at the main.rs call site is different, compile error will name it.

### Task 5.1: Create `engine/subtitles.rs`

**Files:**
- Create: `/home/ozogorgor/Projects/Stui_Project/stui/runtime/src/engine/subtitles.rs`
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/runtime/src/engine/mod.rs` (add `pub mod subtitles;`)

- [ ] **Step 1: Write the new module**

Use host-side `abi::*` types throughout. Model on `Engine::supervisor_search` (`runtime/src/engine/mod.rs:522`) — it already shows the registry lookup + supervisor dispatch + error mapping idiom.

```rust
//! Subtitle fan-out pipeline.
//!
//! Called from `PlayerBridge::play` when `config.subtitles.auto_download`
//! is on. Fans `stui_search` across every enabled Subtitles-capability
//! plugin, filters to `preferred_language`, returns the top 5 candidates.
//! The caller is responsible for calling `stui_resolve` on the chosen
//! candidate and downloading the subtitle file.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tracing::warn;

use stui_plugin_sdk::{EntryKind, SearchScope};

use crate::abi::{SearchRequest as AbiSearchRequest, types::PluginEntry as AbiPluginEntry};
use crate::engine::Engine;
use crate::plugin::PluginCapability;

/// One subtitle candidate from a single plugin.
#[derive(Debug, Clone)]
pub struct SubtitleCandidate {
    pub plugin_id:   String,
    pub plugin_name: String,
    pub entry:       AbiPluginEntry,
    /// Best-effort language extraction, BCP-47 or ISO 639-1/2/3.
    /// None if the plugin didn't surface language info in the entry.
    pub language:    Option<String>,
}

/// Fan subtitle search across every enabled Subtitles-capability plugin.
///
/// Per-plugin timeout: 10s. Language match: case-insensitive, normalized
/// 2-char / 3-char / full-name forms.
///
/// Returns top 5 candidates sorted by (language exact match, imdb_id
/// match, description richness). Empty vec on "no matches" — not an error.
pub async fn fetch_subtitles(
    engine: &Engine,
    title: &str,
    imdb_id: Option<&str>,
    kind: EntryKind,
    language: &str,
) -> Vec<SubtitleCandidate> {
    // Collect enabled Subtitles-capability plugins under a short read-lock,
    // then drop it so per-plugin calls don't hold the registry.
    let providers: Vec<(String, String)> = {
        let reg = engine.registry_read().await;
        reg.find_by_capability(PluginCapability::Subtitles)
            .into_iter()
            .map(|p| (p.id.clone(), p.manifest.plugin.name.clone()))
            .collect()
    };

    if providers.is_empty() {
        return vec![];
    }

    let scope = kind_to_scope(kind);
    let req = AbiSearchRequest {
        query:           title.to_string(),
        scope,
        page:            0,
        limit:           20,
        per_scope_limit: Some(20),
        locale:          Some(language.to_string()),
    };

    // Fan out. Each task owns an Engine clone (cheap — Arc internal state).
    let mut set: JoinSet<(String, String, Option<Vec<AbiPluginEntry>>)> = JoinSet::new();
    for (plugin_id, plugin_name) in providers {
        let req_c = req.clone();
        let engine_c = engine.clone();
        set.spawn(async move {
            let entries = match tokio::time::timeout(
                Duration::from_secs(10),
                call_plugin_search(&engine_c, &plugin_id, req_c),
            ).await {
                Ok(Ok(items)) => Some(items),
                Ok(Err(e)) => {
                    warn!(plugin = %plugin_id, err = %e,
                          "subtitle plugin search failed");
                    None
                }
                Err(_) => {
                    warn!(plugin = %plugin_id,
                          "subtitle plugin search timed out (10s)");
                    None
                }
            };
            (plugin_id, plugin_name, entries)
        });
    }

    let normalized_lang = normalize_language(language);
    let target_imdb = imdb_id.map(str::to_string);

    let mut candidates: Vec<SubtitleCandidate> = Vec::new();
    while let Some(Ok((plugin_id, plugin_name, maybe_entries))) = set.join_next().await {
        let Some(entries) = maybe_entries else { continue };
        for entry in entries {
            let lang = extract_language(&entry);
            candidates.push(SubtitleCandidate {
                plugin_id:   plugin_id.clone(),
                plugin_name: plugin_name.clone(),
                entry,
                language:    lang,
            });
        }
    }

    // Filter: keep only candidates whose language matches. If nothing
    // matches, keep everything (unknown-language candidates are better than
    // no candidates — caller decides whether to use them).
    let (matching, unknown): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|c| c.language
            .as_ref()
            .map(|l| normalize_language(l) == normalized_lang)
            .unwrap_or(false));
    let mut candidates = if !matching.is_empty() { matching } else { unknown };

    // Sort: imdb_id match first, then description richness.
    candidates.sort_by(|a, b| {
        let a_imdb = target_imdb.as_deref() == a.entry.imdb_id.as_deref();
        let b_imdb = target_imdb.as_deref() == b.entry.imdb_id.as_deref();
        b_imdb.cmp(&a_imdb).then_with(|| {
            let a_desc = a.entry.description.as_deref().map(str::len).unwrap_or(0);
            let b_desc = b.entry.description.as_deref().map(str::len).unwrap_or(0);
            b_desc.cmp(&a_desc)
        })
    });

    candidates.truncate(5);
    candidates
}

/// Look up a plugin's WASM supervisor and call `stui_search`, returning
/// the raw `abi::PluginEntry` list. Error mapping is string-flattened
/// here — callers don't need the structured error taxonomy.
async fn call_plugin_search(
    engine: &Engine,
    plugin_id: &str,
    req: AbiSearchRequest,
) -> Result<Vec<AbiPluginEntry>, String> {
    let sup = {
        let reg = engine.registry_read().await;
        match reg.resolve_id(plugin_id) {
            Some(canonical) => reg.wasm_supervisor_for(canonical),
            None => return Err(format!("plugin '{plugin_id}' not found")),
        }
    };
    let sup = sup.ok_or_else(|| format!("plugin '{plugin_id}' has no WASM supervisor"))?;
    let resp = sup.search(&req).await.map_err(|e| e.to_string())?;
    Ok(resp.items)
}

/// Same shape as `call_plugin_search` but for `stui_resolve`. Public so
/// `PlayerBridge::download_subtitle` can use it.
pub async fn call_plugin_resolve(
    engine: &Engine,
    plugin_id: &str,
    entry_id: &str,
) -> Result<crate::abi::types::ResolveResponse, String> {
    let sup = {
        let reg = engine.registry_read().await;
        match reg.resolve_id(plugin_id) {
            Some(canonical) => reg.wasm_supervisor_for(canonical),
            None => return Err(format!("plugin '{plugin_id}' not found")),
        }
    };
    let sup = sup.ok_or_else(|| format!("plugin '{plugin_id}' has no WASM supervisor"))?;
    let req = crate::abi::types::ResolveRequest { entry_id: entry_id.to_string() };
    sup.resolve(&req).await.map_err(|e| e.to_string())
}

fn kind_to_scope(kind: EntryKind) -> SearchScope {
    match kind {
        EntryKind::Movie                        => SearchScope::Movie,
        EntryKind::Series | EntryKind::Episode  => SearchScope::Series,
        // Music kinds are nonsensical for subtitles — send Movie scope
        // (plugins will UNSUPPORTED_SCOPE and return empty). Callers
        // typically won't hit this branch because kind is derived from
        // the play request's media tab.
        _                                       => SearchScope::Movie,
    }
}

/// Lowercase, trim, normalize ISO 639-2/3 and full-name forms to 2-char.
fn normalize_language(lang: &str) -> String {
    let l = lang.trim().to_lowercase();
    match l.as_str() {
        "eng" | "english"            => "en".into(),
        "spa" | "es" | "spanish"     => "es".into(),
        "fre" | "fra" | "french"     => "fr".into(),
        "ger" | "deu" | "german"     => "de".into(),
        "ita" | "italian"            => "it".into(),
        "por" | "portuguese"         => "pt".into(),
        "rus" | "russian"            => "ru".into(),
        "jpn" | "japanese"           => "ja".into(),
        "kor" | "korean"             => "ko".into(),
        "chi" | "zho" | "chinese"    => "zh".into(),
        "ara" | "arabic"             => "ar".into(),
        _ if l.len() > 3             => l,   // unknown full name, keep
        _                            => l,   // already short
    }
}

fn extract_language(entry: &AbiPluginEntry) -> Option<String> {
    if let Some(lang) = &entry.original_language {
        if !lang.is_empty() {
            return Some(lang.clone());
        }
    }
    let haystack = format!(
        "{} {}",
        entry.title.to_lowercase(),
        entry.description.as_deref().unwrap_or("").to_lowercase(),
    );
    for token in ["english", "spanish", "french", "german", "italian",
                  "portuguese", "russian", "japanese", "korean", "chinese",
                  "arabic"] {
        if haystack.contains(token) {
            return Some(normalize_language(token));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::types::PluginEntry;

    #[test]
    fn language_normalization() {
        assert_eq!(normalize_language("en"), "en");
        assert_eq!(normalize_language("eng"), "en");
        assert_eq!(normalize_language("English"), "en");
        assert_eq!(normalize_language("ES"), "es");
    }

    #[test]
    fn extract_from_original_language() {
        let entry = PluginEntry {
            id: "x".into(),
            title: "whatever".into(),
            original_language: Some("fr".into()),
            ..Default::default()
        };
        assert_eq!(extract_language(&entry), Some("fr".into()));
    }

    #[test]
    fn extract_from_description_keyword() {
        let entry = PluginEntry {
            id: "x".into(),
            title: "The Movie".into(),
            description: Some("English SDH".into()),
            ..Default::default()
        };
        assert_eq!(extract_language(&entry), Some("en".into()));
    }
}
```

Suppress the unused-import warning on `std::sync::Arc` if clippy complains — trim it if not actually used.

- [ ] **Step 2: Register the module**

Edit `runtime/src/engine/mod.rs` near the top (after the existing `mod pipeline;` / `mod search_scoped;` / similar declarations):

```rust
pub mod subtitles;
```

No further engine-level pub wrapping needed — callers reach the functions as `crate::engine::subtitles::fetch_subtitles(...)` and `crate::engine::subtitles::call_plugin_resolve(...)`.

- [ ] **Step 3: Build runtime**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui && \
    cargo build --release -p stui-runtime 2>&1 | tail -10
```

Expected: `Finished`. First rebuild after Task 5.0 + 5.1 is likely a full LTO (~13 min).

- [ ] **Step 4: Run the subtitle unit tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui && \
    cargo test --release -p stui-runtime engine::subtitles 2>&1 | tail -10
```

Expected: 3 tests pass.

### Task 5.2: (Deleted — see rationale)

The original plan inserted dedicated `Event` enum variants for subtitle events. Investigation showed the runtime's `Event` enum carries only `ScopeResults`; all other out-of-band events (`PluginToastEvent`, `download_started`, `download_progress`, player lifecycle events) flow as ad-hoc JSON strings through `ipc_tx`. Subtitle events match that pattern — no enum change needed. Event emission is inlined in Task 5.3 Step 3 below.

### Task 5.3: Player-bridge subtitle prelude

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/runtime/src/player/bridge.rs`

- [ ] **Step 1: Add imports at the top of the file (if not already present)**

```rust
use crate::config::ConfigManager;
// (above imports may already include tokio, tracing::warn, etc.)
```

- [ ] **Step 2: Insert the subtitle prelude in `PlayerBridge::play` between lines 166 and 167**

Current code at `runtime/src/player/bridge.rs:166-168`:

```rust
let sub_path = find_subtitle(&self.data_dir, imdb_id);
self.start_stream(entry_id, &stream_url, title, sub_path.as_deref(), media_type, year).await;
```

Replace lines 166-168 with:

```rust
// Subtitle auto-download prelude. Best-effort: any failure falls
// through to the existing sidecar helper. 5s total cap on fetch so
// mpv warmup isn't blocked.
let cfg_snap = self.config.snapshot().await;
if cfg_snap.subtitles.auto_download && !imdb_id.is_empty() {
    let kind = match tab {
        Some(crate::ipc::MediaTab::Series) | Some(crate::ipc::MediaTab::Anime) =>
            stui_plugin_sdk::EntryKind::Series,
        _ => stui_plugin_sdk::EntryKind::Movie,
    };
    let lang = cfg_snap.subtitles.preferred_language.clone();
    let engine = self.engine.clone();
    let title_owned = title.to_string();
    let imdb_owned = imdb_id.to_string();
    let data_dir = self.data_dir.clone();
    let tx = self.ipc_tx.clone();

    // Spawn in a task so the 5s timeout doesn't block play when the
    // fetch is fast; we explicitly await this task since play needs
    // the subtitle file on disk before `find_subtitle` runs next.
    let fetched = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::engine::subtitles::fetch_subtitles(
            &engine, &title_owned, Some(&imdb_owned), kind, &lang,
        ),
    ).await.unwrap_or_default();

    if let Some(candidate) = fetched.into_iter().next() {
        match Self::download_subtitle_candidate(&engine, &candidate, &imdb_owned, &data_dir).await {
            Ok(file_name) => {
                let msg = serde_json::to_string(&serde_json::json!({
                    "type":      "subtitle_fetched",
                    "language":  candidate.language.unwrap_or_else(|| "unknown".into()),
                    "provider":  candidate.plugin_name,
                    "file_name": file_name,
                })).unwrap_or_default();
                let _ = tx.send(msg).await;
            }
            Err(e) => {
                warn!(error = %e, "subtitle download failed");
                let msg = serde_json::to_string(&serde_json::json!({
                    "type":   "subtitle_search_failed",
                    "reason": e.to_string(),
                })).unwrap_or_default();
                let _ = tx.send(msg).await;
            }
        }
    }
}
drop(cfg_snap);

let sub_path = find_subtitle(&self.data_dir, imdb_id);
self.start_stream(entry_id, &stream_url, title, sub_path.as_deref(), media_type, year).await;
```

Note: `imdb_id` is `&str` not `Option<&str>` in the existing signature; we gate on `!imdb_id.is_empty()`. The existing `find_subtitle` call uses the raw param directly; its internal handling of empty strings is unchanged.

The `Anime` arm on `tab` maps to `EntryKind::Series` — most anime is series-shaped, and subtitle plugins that specialize in anime (e.g. kitsunekko) expect Series-style queries.

- [ ] **Step 3: Add the `download_subtitle_candidate` associated function**

Add as an `impl PlayerBridge` associated function (static, takes engine by ref — NOT a method) near the bottom of the impl block, before `find_subtitle`:

```rust
/// Resolve the candidate's entry_id to a subtitle URL via the plugin,
/// HTTP-GET the file to the canonical sidecar path. Returns the basename
/// of the written file.
///
/// Layout: `{data_dir}/subtitles/{imdb_id}/{lang}.srt` matches the
/// layout that `find_subtitle` already scans, so the file picks up
/// automatically on the next `find_subtitle` call in the play path.
async fn download_subtitle_candidate(
    engine: &Engine,
    candidate: &crate::engine::subtitles::SubtitleCandidate,
    imdb_id: &str,
    data_dir: &str,
) -> anyhow::Result<String> {
    // 1. Resolve — 10s cap.
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::engine::subtitles::call_plugin_resolve(
            engine, &candidate.plugin_id, &candidate.entry.id,
        ),
    ).await
        .map_err(|_| anyhow::anyhow!("subtitle resolve timeout (10s)"))?
        .map_err(|e| anyhow::anyhow!("subtitle resolve: {e}"))?;

    let url = resp.stream_url;
    if url.is_empty() {
        anyhow::bail!("subtitle resolve returned empty stream_url");
    }

    // 2. Compose sidecar path.
    let lang = candidate.language.as_deref().unwrap_or("unknown");
    let sub_dir = format!("{data_dir}/subtitles/{imdb_id}");
    tokio::fs::create_dir_all(&sub_dir).await
        .map_err(|e| anyhow::anyhow!("mkdir {sub_dir}: {e}"))?;
    let file_name = format!("{lang}.srt");
    let file_path = format!("{sub_dir}/{file_name}");

    // 3. Plain HTTP GET — 15s cap. Subtitle files are <100KB; aria2
    // is overkill and not needed for a one-shot fetch.
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        async {
            reqwest::get(&url).await
                .map_err(|e| anyhow::anyhow!("GET {url}: {e}"))?
                .error_for_status()
                .map_err(|e| anyhow::anyhow!("GET {url}: HTTP {e}"))?
                .bytes().await
                .map_err(|e| anyhow::anyhow!("read {url}: {e}"))
        },
    ).await
        .map_err(|_| anyhow::anyhow!("subtitle download timeout (15s)"))??;

    tokio::fs::write(&file_path, &bytes).await
        .map_err(|e| anyhow::anyhow!("write {file_path}: {e}"))?;

    tracing::info!(plugin = %candidate.plugin_name, path = %file_path,
                   "subtitle downloaded");
    Ok(file_name)
}
```

- [ ] **Step 4: Build runtime**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui && \
    cargo build --release -p stui-runtime 2>&1 | tail -10
```

Expected: incremental, ~2-5 min.

### Task 5.4: TUI toast wiring

**Files:**
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/tui/internal/ipc/messages.go`
- Modify: `/home/ozogorgor/Projects/Stui_Project/stui/tui/internal/ipc/internal.go`

- [ ] **Step 1: Add message types**

In `messages.go`:

```go
// SubtitleFetchedMsg is pushed when auto-download succeeds for a played stream.
type SubtitleFetchedMsg struct {
    Language string `json:"language"`
    Provider string `json:"provider"`
    FileName string `json:"file_name"`
}

// SubtitleSearchFailedMsg is pushed when subtitle search/download fails.
type SubtitleSearchFailedMsg struct {
    Reason string `json:"reason"`
}
```

- [ ] **Step 2: Route events from readLoop**

In `internal.go` where other events like `PluginToastMsg` are dispatched, add handlers:

```go
case "subtitle_fetched":
    var msg SubtitleFetchedMsg
    if err := json.Unmarshal(raw.Raw, &msg); err != nil {
        c.logger.Warn("failed to parse subtitle_fetched", "error", err)
    } else {
        c.send(msg)
    }
case "subtitle_search_failed":
    var msg SubtitleSearchFailedMsg
    if err := json.Unmarshal(raw.Raw, &msg); err != nil {
        c.logger.Warn("failed to parse subtitle_search_failed", "error", err)
    } else {
        c.send(msg)
    }
```

- [ ] **Step 3: Handle in Model.Update**

Find the existing `PluginToastMsg` handler in `tui/internal/ui/ui.go`. Add adjacent handlers:

```go
case ipc.SubtitleFetchedMsg:
    t, cmd := components.ShowToast(
        fmt.Sprintf("Subtitle: %s · %s", msg.Language, msg.Provider),
        false,
    )
    m.toast = t
    return m, cmd

case ipc.SubtitleSearchFailedMsg:
    t, cmd := components.ShowToast(
        fmt.Sprintf("Subtitle search failed: %s", msg.Reason),
        true,
    )
    m.toast = t
    return m, cmd
```

- [ ] **Step 4: Build TUI**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && \
    go build -ldflags="-s -w" -o /home/ozogorgor/Projects/Stui_Project/stui/dist/stui ./cmd/stui 2>&1 | tail -5
```

---

## Chunk 6: Deploy, tag, smoke

### Task 6.1: Deploy runtime + TUI

- [ ] **Step 1: Kill stale runtime**

```bash
pkill -x stui-runtime 2>/dev/null || true
```

- [ ] **Step 2: Deploy both**

```bash
cp /home/ozogorgor/.cargo/target/release/stui-runtime \
   /home/ozogorgor/Projects/Stui_Project/stui/dist/stui-runtime
cp /home/ozogorgor/Projects/Stui_Project/stui/dist/stui-runtime \
   /home/ozogorgor/.local/bin/stui-runtime
cp /home/ozogorgor/Projects/Stui_Project/stui/dist/stui \
   /home/ozogorgor/.local/bin/stui
ls -la /home/ozogorgor/.local/bin/stui /home/ozogorgor/.local/bin/stui-runtime
```

### Task 6.2: Commit stui monorepo

- [ ] **Step 1: Stage + commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui && \
    git status --short
```

Expected files:
- `sdk/src/manifest.rs` (default flip)
- `runtime/src/engine/subtitles.rs` (new)
- `runtime/src/engine/mod.rs` (mod subtitles;)
- `runtime/src/ipc/v1/mod.rs` (events)
- `runtime/src/ipc/mod.rs` (re-exports)
- `runtime/src/player/bridge.rs` (pipeline prelude + download helper)
- `tui/internal/ipc/messages.go` (types)
- `tui/internal/ipc/internal.go` (routing)
- `tui/internal/ui/ui.go` (toast handlers)
- `docs/superpowers/specs/2026-04-23-subtitle-pipeline-design.md` (new)
- `docs/superpowers/plans/2026-04-23-subtitle-pipeline.md` (new)

```bash
git add -A && git commit -m "$(cat <<'EOF'
feat(subtitles): runtime dispatch pipeline + SDK default fix

SDK: CatalogCapability::default() flips from `Typed { kinds: [],
search: None }` to `Enabled(false)`. Unblocks validation for
subtitle-only and stream-only plugins. Typed-catalog consumers
(all bundled metadata plugins) declare their catalog block
explicitly, so this default change is invisible to them.

Runtime:
- New engine::subtitles module: fetch_subtitles fan-outs stui_search
  across Subtitles-capability plugins with a 10s per-plugin cap,
  filters by preferred_language with best-effort BCP-47/ISO-639
  normalization, sorts imdb-match > description-richness, returns
  top 5.
- PlayerBridge::play now auto-fetches subtitles when
  config.subtitles.auto_download is true. Writes to the existing
  sidecar layout {data_dir}/subtitles/{imdb_id}/{lang}.srt so the
  existing find_subtitle(...) helper picks up the new file
  naturally — no bespoke mpv arg-mangling.
- 5s cap on the fetch; 10s cap per plugin; 10s resolve; 15s
  download. All best-effort — any failure logs and falls through
  to play-without-subs.
- New IPC events SubtitleFetchedEvent / SubtitleSearchFailedEvent.

TUI: toast on success / failure. No picker UI (deferred).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)" && git push origin main
```

### Task 6.3: Commit stui_plugins + tag v0.3.0

- [ ] **Step 1: Commit the three subtitle plugin migrations + SDK default flip + matrix extension**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui_plugins && \
    git status --short
```

Expected files:
- `sdk/src/manifest.rs` (default flip)
- `kitsunekko/plugin.toml` (rewrite)
- `kitsunekko/src/lib.rs` (migration)
- `kitsunekko/tests/manifest_parses.rs` (new)
- `subscene/plugin.toml`, `src/lib.rs`, `tests/manifest_parses.rs`
- `yify-subs/plugin.toml`, `src/lib.rs`, `tests/manifest_parses.rs`
- `opensubtitles-provider/tests/manifest_parses.rs` (now validates)
- `.github/workflows/release.yml` (matrix)

```bash
git add -A && git commit -m "$(cat <<'EOF'
feat(plugins): migrate kitsunekko/subscene/yify-subs + SDK fix

Three subtitle plugins migrated to Plugin + CatalogPlugin using the
opensubtitles template (Approach B). SDK CatalogCapability default
flipped to Enabled(false) so subtitle-only manifests validate.
Opensubtitles test now exercises validate_manifest as well, since
the SDK gap it was working around is closed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)" && git push origin main && \
git tag -a v0.3.0 -m "v0.3.0 — subtitle plugin fleet + SDK gap fix" && \
git push origin v0.3.0
```

- [ ] **Step 2: Wait for CI**

```bash
gh run list --repo Ozogorgor/stui_plugins --limit 1
```

Expected: v0.3.0 CI run in progress, then success within ~3 min.

- [ ] **Step 3: Verify release + plugins.json**

```bash
gh release view v0.3.0 --repo Ozogorgor/stui_plugins 2>&1 | grep "^asset:"
curl -sL https://ozogorgor.github.io/stui_plugins/plugins.json | \
    python3 -c "import json,sys; d=json.load(sys.stdin); print(f'{len(d)} entries:'); [print(' ', e['name'], e['version']) for e in d]"
```

Expected assets: `kitsunekko-*.tar.gz`, `subscene-*.tar.gz`, `yify-subs-*.tar.gz` alongside existing. plugins.json shows 9 entries.

### Task 6.4: Manual smoke

- [ ] **Step 1: Relaunch stui, install the three new subtitle plugins from Available**

- [ ] **Step 2: Configure `subtitles.auto_download = true` and `preferred_language = en` in stui**

- [ ] **Step 3: Play any movie with an imdb_id via the stui grid**

Expected: within ~5 seconds of play-start, a toast shows "Subtitle: en · <provider>". Check `~/.stui/data/subtitles/<imdb_id>/en.srt` exists.

- [ ] **Step 4: Negative test**

Disable all subtitle plugins via the Plugin Manager. Play the same movie. Expected: no toast, no subtitle download. Play still works.

---

## Out-of-band risks

1. **Engine clone semantics.** `engine::subtitles::fetch_subtitles` receives `&Engine`, but the in-thread JoinSet tasks need owned `Engine`. If `Engine` is not `Clone` (i.e. wraps `Arc<_>` internally without a `Clone` impl on the outer type), expose a `.arc()` accessor or have the JoinSet task take `Arc<Engine>` directly. Adjust the signature if needed.

2. **Scraper plugin fingerprinting.** kitsunekko / subscene / yify-subs are scrapers. If they 403 during smoke test, the plugin returns an error, `fetch_subtitles` falls through to the next, the final empty list is silently accepted. Not a correctness bug.

3. **Sidecar name collision.** Writing `en.srt` to `{data_dir}/subtitles/{imdb_id}/` may clobber a user-dropped subtitle with the same basename. For v1 this is acceptable — users who curate their own subs can turn `auto_download` off. Not a regression: the existing `find_subtitle` already grabs the first `.srt` it finds, so user files with different names coexist fine.

4. **Language detection is crude.** Scrapers often don't populate `original_language`; the regex-on-description fallback may miss or false-positive. Flag candidates in logs so misses are diagnosable.
