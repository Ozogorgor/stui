# STUI Search Refactor Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace STUI's global `/` search with focus-scoped search. Each
searchable screen dispatches through its own `DataSource` (MPD-backed or
plugin-backed), the plugin ABI gains typed scopes + manifest-declared kinds,
and results stream back per-scope with partial-deadline + hard-floor timing.

**Architecture:** Go-side: a `Searchable` interface gates the top-bar input
per focused screen; a shared `CatalogBrowser` component (extracted from
Music Library) renders typed results via a `DataSource` interface. Rust-side:
a new `Engine::search_scoped` streams per-scope results built from a
dispatch map keyed on `plugin.toml` declared `catalog.kinds`; MPD bridge
gains a thin passthrough to MPD's native `search` command. IPC v1 gets a
first-class streaming-response path (none exists today).

**Tech Stack:** Go / Bubble Tea (TUI), Rust / Tokio (runtime), WASM plugin
host (existing), MPD native protocol, `teatest` (Go component tests),
existing IPC codec.

**Spec:** `docs/superpowers/specs/2026-04-19-search-refactor-design.md`

**Deferred (spec §1):** plugin performance rating, Plex/Jellyfin bridges,
XDG migration, local video indexer, playlist/genre as search scopes,
universal/global search.

---

## Naming Disambiguation

Two identically-named structures exist and are NOT the same. The plan names
them explicitly throughout:

- **`sdk::SearchRequest`** — plugin-facing ABI at `sdk/src/lib.rs:50`.
  Flat `{query, tab, page, limit}` today. Crossed through WASM FFI.
- **`ipc::v1::SearchRequest`** — wire protocol at `runtime/src/ipc/v1/mod.rs:319`.
  Rich filter struct today. Sent between TUI and runtime.

Similarly:

- **Go `MediaEntry`** — `tui/internal/ipc/types.go:63`. Used by search results today.
- **Go `CatalogEntry`** — `tui/internal/ipc/messages.go:76`. Used by the catalog grid cache.

- **Rust MPD wire types** — `MpdArtistWire`, `MpdAlbumWire`, `MpdSongWire`
  (`runtime/src/ipc/v1/mod.rs:701,707,727`). There is **no** `LibraryEntry`
  type today; we reuse the existing `Mpd*Wire` trio in the new `MpdSearchResult`.

- **`PluginError`** is a struct `{code, message}` (`sdk/src/lib.rs:97`),
  NOT an enum. "Unsupported scope" is signaled by
  `PluginResult::err("unsupported_scope", "...")`.

- **`PluginManifest`** lives at `runtime/src/plugin.rs:11`, NOT in the SDK.
  It currently uses `#[serde(flatten)] _extra: HashMap<String, toml::Value>`
  to tolerate unknown capabilities. We replace the catch-all with a typed
  `Capabilities` struct and keep the flatten as a narrower catch-all.

- **Plugin IDs** — the runtime identifies plugins by `manifest.plugin.name`
  (e.g., the `discogs-provider/` directory ships a manifest with
  `name = "discogs"`). The dispatch map keys, cache keys, `PluginEntry.source`,
  and test fixtures all use this `name` value, NOT the directory basename.
  Before touching tests, verify the actual name with
  `grep -h '^name' plugins/*/plugin.toml`.

---

## File Structure

**New Rust modules:**
- `sdk/src/kinds.rs` — `EntryKind` + `SearchScope` enums (keeps `lib.rs` focused)
- `runtime/src/engine/search_scoped.rs` — streaming scoped search, partial-deadline + hard-floor
- `runtime/src/engine/dispatch_map.rs` — `SearchScope → [plugin_id]` lookup, built from manifests at load
- `runtime/src/mpd_bridge/search.rs` — MPD `search` verb passthrough
- `runtime/src/ipc/v1/stream.rs` — streaming-response scaffold for request/event pattern

**Modified Rust files:**
- `sdk/src/lib.rs` — re-export `kinds`; extend `sdk::SearchRequest` + `PluginEntry`; add `error_codes` constant module
- `runtime/src/plugin.rs` — replace `_extra` catch-all with typed `Capabilities { catalog: CatalogCapability, … }`; keep a narrower flatten for forward-compat
- `runtime/src/engine/mod.rs` — wire `search_scoped`; add `dispatch_map()` + `supervisor_search()` helpers; share Engine state through `Arc<EngineInner>`
- `runtime/src/pipeline/search.rs` — rewrite `run_search` to drive `search_scoped` over the new streaming channel
- `runtime/src/cache/search.rs` — extend cache key to `(plugin_id, query_norm, scope, page)`
- `runtime/src/ipc/v1/mod.rs` — add `Request::MpdSearch`; extend/replace `SearchRequest` (scope+query_id); add `ScopeResultsMsg`, `MpdSearchResult`, `MpdScope`, `MpdSearchError`, `ScopeError`; register streaming event types
- `runtime/src/main.rs` — route `Request::MpdSearch`; switch `Request::Search` to streaming handler

**Modified plugin manifests** (one commit per plugin, Chunk 7):
- `plugins/*/plugin.toml` — add `[capabilities.catalog] kinds = [...]` for every Catalog-capable plugin (14 total)
- `plugins/*/src/lib.rs` — honor `req.scope` in `search()`

**New Go files:**
- `tui/internal/ui/screens/catalogbrowser/browser.go` — extracted 3-column browser component
- `tui/internal/ui/screens/catalogbrowser/datasource.go` — `DataSource` interface, `Entry`, `DataSourceState`
- `tui/internal/ui/screens/catalogbrowser/mpd_source.go` — `MpdDataSource`
- `tui/internal/ui/screens/catalogbrowser/plugin_source.go` — `PluginDataSource`
- `tui/internal/ui/screens/catalogbrowser/source_picker.go` — modal sub-component for multi-source rows
- `tui/internal/ui/screens/catalogbrowser/sources_count.go` — lazy cursor-hover sources count resolver (video)
- `tui/internal/ui/screens/searchable.go` — `Searchable` interface definition (package `screens`; consumed by `ui` — no cycle, `ui` already imports `screens`)

**Modified Go files:**
- `tui/internal/ipc/types.go` — add `EntryKind`, `SearchScope` string enums; add `Kind` + `Source` + per-kind fields to `MediaEntry`
- `tui/internal/ipc/messages.go` — add `Kind` + `Source` fields to `CatalogEntry` (mirrors the same plugin wire shape) + `ScopeResultsMsg`
- `tui/internal/ipc/requests.go` — rework `Client.Search` to streaming; add `Client.MpdSearch`; add query_id counter + subscription map
- `tui/internal/ipc/client.go` — route server-initiated streaming events to per-query subscribers
- `tui/internal/ui/ui.go` — route `/` through `Searchable` probe; hide search bar if non-Searchable
- `tui/internal/ui/screens/music_library.go` — swap raw 3-col layout for `CatalogBrowser` + `MpdDataSource`; implement `Searchable`
- `tui/internal/ui/screens/music_browse.go` — drop `filtered()`, adopt `CatalogBrowser` + `PluginDataSource`; implement `Searchable`
- `tui/internal/ui/screens/search.go` — delete `searchAll` field + `a`-toggle + branches
- `tui/internal/ui/root.go` — update the doc comment at line 22 (drop `NewSearchScreen` example) if the constructor is deleted
- (Movies/Series/Library grid model) — adopt `Searchable` + optional `Sources` column

---

## Chunk 1: SDK Types

### Task 1.1: Add `EntryKind` and `SearchScope` enums to SDK

**Files:**
- Create: `sdk/src/kinds.rs`
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Create `sdk/src/kinds.rs`**

```rust
//! Typed kinds for search scoping and entry classification.
//!
//! `EntryKind` describes what a returned entry *is* (wire contract on
//! PluginEntry). `SearchScope` describes what a caller is *asking for*
//! (request parameter). Identical members today; kept separate so future
//! runtime-only scope values don't leak into `EntryKind`.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Artist, Album, Track,
    Movie, Series, Episode,
}

impl Default for EntryKind {
    fn default() -> Self { EntryKind::Track }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    Artist, Album, Track,
    Movie, Series, Episode,
}

impl SearchScope {
    pub fn matches(self, kind: EntryKind) -> bool {
        matches!((self, kind),
            (SearchScope::Artist, EntryKind::Artist) |
            (SearchScope::Album,  EntryKind::Album)  |
            (SearchScope::Track,  EntryKind::Track)  |
            (SearchScope::Movie,  EntryKind::Movie)  |
            (SearchScope::Series, EntryKind::Series) |
            (SearchScope::Episode,EntryKind::Episode)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn entry_kind_snake_case() {
        assert_eq!(serde_json::to_string(&EntryKind::Artist).unwrap(), "\"artist\"");
    }

    #[test] fn search_scope_round_trips() {
        for s in [SearchScope::Artist, SearchScope::Track, SearchScope::Movie] {
            let j = serde_json::to_string(&s).unwrap();
            let back: SearchScope = serde_json::from_str(&j).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test] fn scope_matches_kind() {
        assert!(SearchScope::Track.matches(EntryKind::Track));
        assert!(!SearchScope::Track.matches(EntryKind::Artist));
    }
}
```

- [ ] **Step 2: Re-export from `sdk/src/lib.rs`**

Near existing module declarations:

```rust
pub mod kinds;
pub use kinds::{EntryKind, SearchScope};
```

- [ ] **Step 3: Test**

```
cd sdk && cargo test --lib kinds
```

Expect: three tests pass.

- [ ] **Step 4: Commit**

```
git add sdk/src/kinds.rs sdk/src/lib.rs
git commit -m "feat(sdk): add EntryKind and SearchScope enums"
```

### Task 1.2: Extend `sdk::SearchRequest` with scope, drop `tab`

**Files:**
- Modify: `sdk/src/lib.rs` around line 50

This mutates the **plugin-facing ABI**. Plugin builds will break; Chunk 7
migrates them.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn sdk_search_request_carries_scope() {
    let req = SearchRequest {
        query: "creep".into(),
        scope: SearchScope::Track,
        page: 0,
        limit: 50,
        per_scope_limit: None,
        locale: None,
    };
    let s = serde_json::to_string(&req).unwrap();
    assert!(s.contains("\"scope\":\"track\""));
    assert!(!s.contains("\"tab\""));
}
```

- [ ] **Step 2: Run — expect compile failure**

```
cd sdk && cargo test sdk_search_request_carries_scope
```

- [ ] **Step 3: Update struct**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub scope: SearchScope,
    pub page: u32,
    pub limit: u32,
    #[serde(default)] pub per_scope_limit: Option<u32>,
    #[serde(default)] pub locale: Option<String>,
}
```

Delete the old `tab: String` field. Do NOT add genre / rating / year filters
here — those live on `ipc::v1::SearchRequest` today and stay there (or get
dropped there) per the spec.

- [ ] **Step 4: Run — expect pass; then expect plugin build failure (intended)**

```
cd sdk && cargo test                                      # pass
cd plugins && cargo build --target wasm32-wasip1 --workspace  # fail
```

- [ ] **Step 5: Commit** (plugins intentionally broken until Chunk 7)

```
git add sdk/src/lib.rs
git commit -m "feat(sdk): SearchRequest gains scope, drops tab

Plugins will fail to build until Chunk 7 migrates them to honor scope."
```

### Task 1.3: Extend `PluginEntry` with `kind` and per-kind fields

**Files:**
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Locate existing `PluginEntry`** (search `grep -n "pub struct PluginEntry" sdk/src/lib.rs`)

- [ ] **Step 2: Verify no positional construction exists**

```
grep -rn "PluginEntry {" plugins/ sdk/ runtime/ | grep -v "impl\|pub struct"
```

If any callers use positional `PluginEntry(...)` construction, this refactor
requires touching them. Expect all sites to use named-field literals.

- [ ] **Step 3: Failing test**

```rust
#[test]
fn plugin_entry_has_kind_and_source() {
    let entry = PluginEntry {
        id: "spotify:track:abc".into(),
        kind: EntryKind::Track,
        title: "Creep".into(),
        source: "lastfm-provider".into(),
        year: Some(1993),
        artist_name: Some("Radiohead".into()),
        album_name: Some("Pablo Honey".into()),
        track_number: Some(2),
        ..Default::default()
    };
    let s = serde_json::to_string(&entry).unwrap();
    assert!(s.contains("\"kind\":\"track\""));
    assert!(s.contains("\"source\":\"lastfm-provider\""));
}
```

- [ ] **Step 4: Implement**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginEntry {
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    pub source: String,

    #[serde(default, skip_serializing_if = "Option::is_none")] pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub rating: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub poster_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub duration: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")] pub artist_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub album_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub episode: Option<u32>,
}
```

Preserve existing field order for fields that already exist; new fields go
at the end. Maps cleanly to JSON regardless of order.

- [ ] **Step 5: Run — pass**

- [ ] **Step 6: Commit**

```
git add sdk/src/lib.rs sdk/src/kinds.rs
git commit -m "feat(sdk): PluginEntry gains kind, source, per-kind fields"
```

### Task 1.4: Reserve the `"unsupported_scope"` error code

**Files:**
- Modify: `sdk/src/lib.rs`

`PluginError` is a struct `{code, message}`, not an enum. We reserve the
code string as a public constant; runtime dispatch matches on the string.

- [ ] **Step 1: Add constant module**

```rust
pub mod error_codes {
    pub const UNSUPPORTED_SCOPE: &str = "unsupported_scope";
    pub const INVALID_REQUEST: &str   = "invalid_request";
    // add others as the ABI expands
}
```

- [ ] **Step 2: Test**

```rust
#[test]
fn err_helper_with_unsupported_scope_code() {
    let r: PluginResult<()> = PluginResult::err(
        error_codes::UNSUPPORTED_SCOPE,
        "track scope unsupported by this plugin",
    );
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains("\"code\":\"unsupported_scope\""));
}
```

- [ ] **Step 3: Run — pass**

- [ ] **Step 4: Commit**

```
git add sdk/src/lib.rs
git commit -m "feat(sdk): reserve unsupported_scope error code"
```

---

## Chunk 2: Runtime Infrastructure

### Task 2.1: Add typed `Capabilities` to `runtime::PluginManifest`

**Files:**
- Modify: `runtime/src/plugin.rs`

- [ ] **Step 1: Failing test**

```rust
// runtime/src/plugin.rs tests module
#[test]
fn manifest_parses_catalog_kinds() {
    let toml_text = r#"
[plugin]
name = "discogs-provider"
version = "0.1.0"

[capabilities]
streams = false

[capabilities.catalog]
kinds = ["artist", "album", "track"]
"#;
    let m: PluginManifest = toml::from_str(toml_text).unwrap();
    use stui_plugin_sdk::EntryKind;
    assert_eq!(
        m.capabilities.catalog.kinds,
        vec![EntryKind::Artist, EntryKind::Album, EntryKind::Track]
    );
    assert!(!m.capabilities.streams);
}

#[test]
fn manifest_without_catalog_kinds_opts_out() {
    let toml_text = r#"
[plugin]
name = "subscene"
version = "0.1.0"

[capabilities]
streams = false
"#;
    let m: PluginManifest = toml::from_str(toml_text).unwrap();
    assert!(m.capabilities.catalog.kinds.is_empty());
}
```

- [ ] **Step 2: Run — expect compile failure**

- [ ] **Step 3: Replace `_extra` with a typed `capabilities` field**

```rust
use stui_plugin_sdk::EntryKind;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogCapability {
    #[serde(default)]
    pub kinds: Vec<EntryKind>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub catalog: CatalogCapability,
    #[serde(default)]
    pub streams: bool,
    // catch everything else so unknown capability keys don't break load
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}

pub struct PluginManifest {
    pub plugin: PluginMeta,
    pub permissions: Option<Permissions>,
    pub meta: Option<AuthorMeta>,
    #[serde(default)] pub env: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_config_fields")]
    pub config: Vec<PluginConfigField>,
    #[serde(default)]
    pub capabilities: Capabilities,
    // shrink the top-level flatten: it was a catch-all that included `capabilities`.
    // With `capabilities` now typed, keep a narrower flatten for unknown top-level keys.
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}
```

- [ ] **Step 4: Run — expect pass**

```
cd runtime && cargo test --lib plugin
```

- [ ] **Step 5: Audit every existing `plugin.toml`** to make sure none use
an unexpected `[capabilities.X]` shape that clashes with the new schema.

```
for f in plugins/*/plugin.toml; do echo "=== $f ==="; cat "$f"; done | grep -A3 capabilities
```

- [ ] **Step 6: Commit**

```
git add runtime/src/plugin.rs
git commit -m "feat(runtime): typed Capabilities schema in PluginManifest"
```

### Task 2.2: Add streaming-response scaffold to IPC v1

**Files:**
- Create: `runtime/src/ipc/v1/stream.rs`
- Modify: `runtime/src/ipc/v1/mod.rs`

The existing IPC is request/response. Comment at `runtime/src/main.rs:1258`
confirms no streaming. Before search can stream per-scope results, we add a
first-class streaming-response primitive.

Two options considered:
1. **Server-initiated events** keyed by `query_id` — a new top-level
   `Event::ScopeResults(ScopeResultsMsg)` frame, dispatched by the client to
   per-query subscribers.
2. **Per-request response streams** — extend the request/response frame with
   a `sequence` field so a single request id can receive multiple responses.

Option 1 matches how album-art/tag-write progress events WOULD have looked
if they'd been built (the `// v1: no streaming progress` comment implies
they were planned). We go with option 1 — cleaner semantics, no framer
changes, easier to extend (e.g., future plugin crash notifications).

- [ ] **Step 1: Add a top-level `Event` envelope**

```rust
// runtime/src/ipc/v1/stream.rs
//! Server-initiated events carried over the same IPC connection as
//! request/response. The client routes events to per-query subscribers.

use super::{ScopeResultsMsg};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ScopeResults(ScopeResultsMsg),
    // room for: TagWriteProgress, PluginCrashed, etc.
}
```

- [ ] **Step 2: Extend the IPC envelope** (which is almost certainly a
`Frame` or `Message` enum in `ipc/v1/mod.rs`) with an `Event(Event)` variant.

Find the envelope: `grep -n "pub enum Message\|pub enum Frame\|#\[serde(tag" runtime/src/ipc/v1/mod.rs | head`.

Add variant alongside existing `Request`/`Response`:

```rust
pub enum Frame {
    Request(Request),
    Response(Response),
    Event(stream::Event),
}
```

If the actual envelope shape differs, adapt — the goal is: the server can
emit `Event` frames at any time; the client delivers them to subscribers.

- [ ] **Step 3: Add a helper on the server side** for emitting events:

```rust
// called from inside any handler that needs to stream
pub async fn emit_event(tx: &EventSender, event: Event) {
    let _ = tx.send(Frame::Event(event)).await;
}
```

`EventSender` is whatever type the existing transport uses to write frames
back to the client. The handler receives it alongside the request (or holds
a shared `Arc<EventSender>` on the server state).

- [ ] **Step 4: Test** — integration test that sends a dummy event through
the loop and confirms the client-side decoder routes it as an `Event`.

- [ ] **Step 5: Run — iterate until pass**

- [ ] **Step 6: Commit**

```
git add runtime/src/ipc/v1/stream.rs runtime/src/ipc/v1/mod.rs
git commit -m "feat(ipc): server-initiated Event frame for streaming responses"
```

### Task 2.3: Add `ipc::v1::SearchRequest` scopes + `ScopeResultsMsg`

**Files:**
- Modify: `runtime/src/ipc/v1/mod.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn ipc_search_request_with_scopes_roundtrips() {
    use stui_plugin_sdk::SearchScope;
    let req = SearchRequest {
        id: "q1".into(),
        query: "creep".into(),
        scopes: vec![SearchScope::Artist, SearchScope::Track],
        limit: 50,
        offset: 0,
        query_id: 42,
    };
    let s = serde_json::to_vec(&req).unwrap();
    let back: SearchRequest = serde_json::from_slice(&s).unwrap();
    assert_eq!(back.scopes, vec![SearchScope::Artist, SearchScope::Track]);
    assert_eq!(back.query_id, 42);
}

#[test]
fn scope_results_msg_shape() {
    let msg = ScopeResultsMsg {
        query_id: 42,
        scope: stui_plugin_sdk::SearchScope::Artist,
        entries: vec![],
        partial: true,
        error: None,
    };
    let s = serde_json::to_string(&msg).unwrap();
    assert!(s.contains("\"partial\":true"));
}
```

- [ ] **Step 2: Update `ipc::v1::SearchRequest`** (at line ~319). Replace
the rich filter struct. Previous fields: `id, query, tab, provider, limit,
offset, sort, genre, min_rating, year_from, year_to`.

```rust
use stui_plugin_sdk::SearchScope;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub id: String,
    pub query: String,
    pub scopes: Vec<SearchScope>,
    pub limit: u32,
    pub offset: u32,
    pub query_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeResultsMsg {
    pub query_id: u64,
    pub scope: SearchScope,
    pub entries: Vec<MediaEntry>,
    pub partial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ScopeError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScopeError {
    AllFailed,
    NoPluginsConfigured,
}
```

`MediaEntry` is the existing IPC-side media entry. Confirm whether we
extend it with `kind`/`source` here (same as `PluginEntry`) — yes, because
it's what the TUI renders. Add:

```rust
pub struct MediaEntry {
    // …existing…
    #[serde(default)] pub kind: EntryKind,
    #[serde(default)] pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub artist_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub album_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub episode: Option<u32>,
}
```

- [ ] **Step 3: Remove dropped filter fields** (sort/genre/min_rating/year_*).
Grep the runtime for usages and migrate:

```
grep -rn "\.sort\|\.min_rating\|\.year_from\|\.year_to\|\.genre" runtime/src/
```

Any code referencing them is deleted along with the fields (post-search
filter pass on the runtime side is not being added in this plan).

- [ ] **Step 4: Run — expect pass**

- [ ] **Step 5: Commit**

```
git add runtime/src/ipc/v1/mod.rs
git commit -m "feat(ipc): SearchRequest gets scopes+query_id; add ScopeResultsMsg"
```

### Task 2.4: Add `Request::MpdSearch` + response types

**Files:**
- Modify: `runtime/src/ipc/v1/mod.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn mpd_search_request_roundtrips() {
    let req = MpdSearchRequest {
        id: "q2".into(),
        query: "radiohead".into(),
        scopes: vec![MpdScope::Artist, MpdScope::Album, MpdScope::Track],
        limit: 200,
        query_id: 7,
    };
    let s = serde_json::to_string(&req).unwrap();
    let back: MpdSearchRequest = serde_json::from_str(&s).unwrap();
    assert_eq!(back.scopes.len(), 3);
}
```

- [ ] **Step 2: Add types**

```rust
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MpdScope { Artist, Album, Track }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MpdSearchRequest {
    pub id: String,
    pub query: String,
    pub scopes: Vec<MpdScope>,
    pub limit: u32,
    pub query_id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MpdSearchResult {
    pub id: String,
    pub query_id: u64,
    pub artists: Vec<MpdArtistWire>,
    pub albums:  Vec<MpdAlbumWire>,
    pub tracks:  Vec<MpdSongWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<MpdSearchError>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MpdSearchError {
    NotConnected,
    CommandFailed { message: String },
}

// Add to Request enum:
pub enum Request { /* …existing… */ MpdSearch(MpdSearchRequest) }

// Add to Response enum (for non-streamed reply — MPD is local & fast):
pub enum Response { /* …existing… */ MpdSearch(MpdSearchResult) }
```

Reuse the existing `MpdArtistWire` (line 701), `MpdAlbumWire` (707),
`MpdSongWire` (727). No new `LibraryEntry` type needed.

- [ ] **Step 3: Run — pass**

- [ ] **Step 4: Commit**

```
git add runtime/src/ipc/v1/mod.rs
git commit -m "feat(ipc): add MpdSearch request/response (reuses Mpd*Wire types)"
```

### Task 2.5: Build `DispatchMap` at plugin load

**Files:**
- Create: `runtime/src/engine/dispatch_map.rs`
- Modify: `runtime/src/engine/mod.rs`

- [ ] **Step 1: Failing test in `dispatch_map.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use stui_plugin_sdk::{EntryKind, SearchScope};

    fn fake(id: &str, kinds: &[EntryKind]) -> PluginEntryInfo {
        PluginEntryInfo { id: id.into(), kinds: kinds.to_vec() }
    }

    #[test]
    fn groups_by_scope() {
        let plugins = vec![
            fake("discogs",  &[EntryKind::Artist, EntryKind::Album, EntryKind::Track]),
            fake("tmdb",     &[EntryKind::Movie, EntryKind::Series]),
            fake("lastfm",   &[EntryKind::Artist, EntryKind::Track]),
        ];
        let m = DispatchMap::build(&plugins);
        assert_eq!(m.plugins_for(SearchScope::Artist), vec!["discogs", "lastfm"]);
        assert_eq!(m.plugins_for(SearchScope::Movie),  vec!["tmdb"]);
        assert!(m.plugins_for(SearchScope::Episode).is_empty());
    }

    #[test]
    fn empty_kinds_excluded() {
        let plugins = vec![fake("subscene", &[])];
        let m = DispatchMap::build(&plugins);
        for scope in [SearchScope::Artist, SearchScope::Movie] {
            assert!(m.plugins_for(scope).is_empty());
        }
    }
}
```

- [ ] **Step 2: Implement**

```rust
use std::collections::HashMap;
use stui_plugin_sdk::{EntryKind, SearchScope};

pub struct PluginEntryInfo {
    pub id: String,
    pub kinds: Vec<EntryKind>,
}

#[derive(Default, Debug, Clone)]
pub struct DispatchMap {
    by_scope: HashMap<SearchScope, Vec<String>>,
}

impl DispatchMap {
    pub fn build(plugins: &[PluginEntryInfo]) -> Self {
        let mut by_scope: HashMap<SearchScope, Vec<String>> = HashMap::new();
        for p in plugins {
            for k in &p.kinds {
                by_scope.entry(scope_of(*k)).or_default().push(p.id.clone());
            }
        }
        Self { by_scope }
    }

    pub fn plugins_for(&self, scope: SearchScope) -> Vec<String> {
        self.by_scope.get(&scope).cloned().unwrap_or_default()
    }
}

fn scope_of(k: EntryKind) -> SearchScope {
    match k {
        EntryKind::Artist  => SearchScope::Artist,
        EntryKind::Album   => SearchScope::Album,
        EntryKind::Track   => SearchScope::Track,
        EntryKind::Movie   => SearchScope::Movie,
        EntryKind::Series  => SearchScope::Series,
        EntryKind::Episode => SearchScope::Episode,
    }
}
```

- [ ] **Step 3: Wire into `Engine::mod.rs`**

Find plugin-load flow (`grep -n "pub async fn load_plugins\|fn load_plugins\|fn register_plugin" runtime/src/engine/mod.rs`). After the plugin set is populated:

```rust
let infos: Vec<PluginEntryInfo> = loaded.iter()
    .map(|p| PluginEntryInfo {
        id: p.id().to_string(),
        kinds: p.manifest().capabilities.catalog.kinds.clone(),
    })
    .collect();
self.dispatch_map = DispatchMap::build(&infos);
```

Add `dispatch_map: DispatchMap` to the Engine struct and a getter:
```rust
pub fn dispatch_map(&self) -> &DispatchMap { &self.dispatch_map }
```

- [ ] **Step 4: Run — pass**

- [ ] **Step 5: Commit**

```
git add runtime/src/engine/dispatch_map.rs runtime/src/engine/mod.rs
git commit -m "feat(engine): dispatch map from declared catalog.kinds"
```

### Task 2.6: Add `Engine::supervisor_search` helper and confirm shared state

**Files:**
- Modify: `runtime/src/engine/mod.rs`

Before `search_scoped` can spawn per-plugin tasks, we need (a) an easy way
to call a single plugin's `search()` via the WASM supervisor, and (b) the
Engine shared across tasks cheaply (`Arc<EngineInner>` or similar).

- [ ] **Step 1: Audit current Engine shape**

```
grep -n "pub struct Engine\|impl Engine\b" runtime/src/engine/mod.rs | head -5
```

If Engine is already behind `Arc<EngineInner>`, skip refactor. If not, wrap
the inner state so callers can `engine.clone()` cheaply.

- [ ] **Step 2: Add helper**

```rust
impl Engine {
    /// Call a single plugin's search via the supervisor, producing
    /// ready-to-merge typed entries or an error.
    pub async fn supervisor_search(
        &self,
        plugin_id: &str,
        query: &str,
        scope: SearchScope,
    ) -> Result<Vec<MediaEntry>, PluginCallError> {
        let sup = self.supervisor_for(plugin_id)?;
        let req = sdk::SearchRequest {
            query: query.into(),
            scope,
            page: 0,
            limit: 100,
            per_scope_limit: None,
            locale: None,
        };
        let resp = sup.search(&req).await?;
        Ok(resp.items.into_iter().map(Into::into).collect())
    }
}
```

Where `supervisor_for` looks up the WASM supervisor by plugin id (likely
already exists in a private form — promote it). `PluginCallError` covers
supervisor timeouts, `unsupported_scope` error codes, and other failures.

- [ ] **Step 3: Test** — mocked supervisor, confirm happy path returns
entries and unsupported-scope error is mapped to a dedicated error variant.

- [ ] **Step 4: Commit**

```
git add runtime/src/engine/mod.rs
git commit -m "feat(engine): supervisor_search helper; Arc-shared Engine state"
```

### Task 2.7: Implement `search_scoped` with partial-deadline + hard-floor

**Files:**
- Create: `runtime/src/engine/search_scoped.rs`
- Modify: `runtime/src/engine/mod.rs`

- [ ] **Step 1: Stub module**

In `runtime/src/engine/mod.rs`:
```rust
mod search_scoped;
pub use search_scoped::{search_scoped, ScopedSearchConfig};
```

- [ ] **Step 2: Failing tests**

```rust
// runtime/src/engine/search_scoped.rs tests module
use std::time::Duration;

#[tokio::test]
async fn all_plugins_return_produces_finalized() {
    let engine = test_engine(&[
        ("fast",   Duration::from_millis(10), vec![entry("a", EntryKind::Artist)]),
        ("medium", Duration::from_millis(50), vec![entry("b", EntryKind::Artist)]),
    ]);
    let cfg = ScopedSearchConfig::default();
    let mut stream = search_scoped(engine.clone(), "q".into(),
        vec![SearchScope::Artist], 7, cfg);

    let mut finalized = None;
    while let Some(msg) = stream.next().await {
        if !msg.partial { finalized = Some(msg); break; }
    }
    let f = finalized.unwrap();
    assert_eq!(f.scope, SearchScope::Artist);
    assert_eq!(f.entries.len(), 2);
    assert_eq!(f.query_id, 7);
}

#[tokio::test]
async fn hard_floor_emits_empty_partial_when_nobody_responds() {
    let engine = test_engine(&[
        ("slow", Duration::from_millis(5_000), vec![]),
    ]);
    let cfg = ScopedSearchConfig {
        partial_deadline: Duration::from_millis(500),
        hard_floor:       Duration::from_millis(200),
    };
    let mut stream = search_scoped(engine.clone(), "q".into(),
        vec![SearchScope::Artist], 0, cfg);

    let msg = tokio::time::timeout(Duration::from_millis(500), stream.next())
        .await.expect("message within hard floor").unwrap();
    assert!(msg.partial);
    assert!(msg.entries.is_empty());
}

#[tokio::test]
async fn scope_with_no_plugins_emits_no_plugins_configured() {
    let engine = test_engine(&[]);
    let mut stream = search_scoped(engine.clone(), "q".into(),
        vec![SearchScope::Episode], 0, ScopedSearchConfig::default());

    let msg = stream.next().await.unwrap();
    assert!(!msg.partial);
    assert_eq!(msg.error, Some(ScopeError::NoPluginsConfigured));
}

#[tokio::test]
async fn slow_scope_does_not_block_fast_scope() {
    let engine = test_engine(&[
        ("fast-artist", Duration::from_millis(10),   vec![entry("a", EntryKind::Artist)]),
        ("slow-track",  Duration::from_millis(2_000), vec![entry("b", EntryKind::Track)]),
    ]);
    let mut stream = search_scoped(engine.clone(), "q".into(),
        vec![SearchScope::Artist, SearchScope::Track], 1, ScopedSearchConfig::default());

    // First finalized message should be the Artist scope, long before Track.
    let start = std::time::Instant::now();
    let first = loop {
        let m = stream.next().await.unwrap();
        if !m.partial { break m; }
    };
    let elapsed = start.elapsed();
    assert_eq!(first.scope, SearchScope::Artist);
    assert!(elapsed < Duration::from_millis(500),
        "artist finalized in {:?}, should be well under 500ms", elapsed);
}

#[tokio::test]
async fn finalized_results_cached_partials_not() {
    // Run search twice with same query; second should short-circuit via cache
    // for finalized plugins. Partial deadlines should not leak into cache.
    // Implementation detail — left to authoring.
}
```

`test_engine` is a helper building an engine with mock plugins, each
resolving their supervisor_search call after a fixed delay. `entry` builds
a `MediaEntry` with the given kind.

- [ ] **Step 3: Implement**

Use a `tokio::sync::mpsc::channel` with per-scope spawn. Each scope task:

1. Look up `engine.dispatch_map().plugins_for(scope)`.
2. If empty → send `ScopeResultsMsg { partial: false, error: Some(NoPluginsConfigured) }` and return.
3. Else spawn per-plugin tasks; use `select!` with three arms: new plugin response, partial-deadline timer (started on first response), and hard-floor timer (started at dispatch).
4. On partial-deadline firing: emit `partial: true` snapshot once.
5. On hard-floor firing if nothing received yet: emit empty `partial: true`.
6. When all plugins done: emit finalized `partial: false` with merged entries; if all failed, error = `AllFailed`.

```rust
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{sleep, Instant};
use stui_plugin_sdk::SearchScope;

pub struct ScopedSearchConfig {
    pub partial_deadline: std::time::Duration,
    pub hard_floor:       std::time::Duration,
}

impl Default for ScopedSearchConfig {
    fn default() -> Self {
        Self {
            partial_deadline: std::time::Duration::from_millis(500),
            hard_floor:       std::time::Duration::from_millis(2000),
        }
    }
}

pub fn search_scoped(
    engine: Engine,
    query: String,
    scopes: Vec<SearchScope>,
    query_id: u64,
    cfg: ScopedSearchConfig,
) -> impl futures::Stream<Item = ScopeResultsMsg> {
    let (tx, rx) = mpsc::channel(32);
    for scope in scopes {
        let tx = tx.clone();
        let q = query.clone();
        let e = engine.clone();
        let cfg_ = ScopedSearchConfig { ..cfg };
        tokio::spawn(async move {
            run_one_scope(e, q, scope, query_id, cfg_, tx).await;
        });
    }
    drop(tx);
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

async fn run_one_scope(
    engine: Engine, query: String, scope: SearchScope, query_id: u64,
    cfg: ScopedSearchConfig, out: mpsc::Sender<ScopeResultsMsg>,
) {
    let plugins = engine.dispatch_map().plugins_for(scope);
    if plugins.is_empty() {
        let _ = out.send(ScopeResultsMsg {
            query_id, scope, entries: vec![], partial: false,
            error: Some(ScopeError::NoPluginsConfigured),
        }).await;
        return;
    }

    let (p_tx, mut p_rx) = mpsc::channel(plugins.len().max(1));
    for pid in &plugins {
        let pid = pid.clone(); let q = query.clone();
        let e = engine.clone(); let tx = p_tx.clone();
        tokio::spawn(async move {
            let res = e.supervisor_search(&pid, &q, scope).await;
            let _ = tx.send(res).await;
        });
    }
    drop(p_tx);

    let hard_floor = sleep(cfg.hard_floor);
    tokio::pin!(hard_floor);
    let mut partial_timer: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut collected: Vec<MediaEntry> = Vec::new();
    let mut pending = plugins.len();
    let mut any_error = false;
    let mut emitted_partial = false;

    while pending > 0 || !emitted_partial {
        tokio::select! {
            biased;
            maybe = p_rx.recv() => match maybe {
                Some(Ok(entries)) => { collected.extend(entries); pending -= 1;
                    if partial_timer.is_none() { partial_timer = Some(Box::pin(sleep(cfg.partial_deadline))); }
                }
                Some(Err(_)) => { any_error = true; pending -= 1; }
                None => break,
            },
            _ = async { partial_timer.as_mut().unwrap().as_mut().await },
                if partial_timer.is_some() && !emitted_partial => {
                let _ = out.send(ScopeResultsMsg {
                    query_id, scope, entries: collected.clone(),
                    partial: true, error: None,
                }).await;
                emitted_partial = true;
            }
            _ = hard_floor.as_mut(),
                if !emitted_partial && collected.is_empty() => {
                let _ = out.send(ScopeResultsMsg {
                    query_id, scope, entries: vec![],
                    partial: true, error: None,
                }).await;
                emitted_partial = true;
            }
        }
    }

    let error = if collected.is_empty() && any_error { Some(ScopeError::AllFailed) } else { None };
    let _ = out.send(ScopeResultsMsg {
        query_id, scope, entries: collected, partial: false, error,
    }).await;
}
```

Exact pinning/borrow form may need tweaking during iteration — verify
`Pin` idioms compile; if `tokio::select!` complains about the optional
partial timer form, use an explicit boolean + `tokio::time::Instant` check
instead.

- [ ] **Step 4: Run — iterate until pass**

```
cd runtime && cargo test --lib search_scoped
```

- [ ] **Step 5: Promote the concurrency semaphore to Engine-level** (spec §6.6)

Today `engine/mod.rs:361` creates a `Semaphore::new(8)` *inside* the
legacy `search()` method (and separately at lines 668 and 794 for other
requests) — it is per-call, not process-wide. The spec's "max 8
concurrent plugin calls globally" implies a shared bound.

Add a shared semaphore to Engine state:

```rust
pub struct EngineInner {
    // …existing…
    plugin_semaphore: Arc<tokio::sync::Semaphore>,  // Semaphore::new(8)
}
```

Have `supervisor_search` acquire a permit before calling the WASM
supervisor. Delete the local `Semaphore::new(8)` at line 361 (legacy
`search()` is being removed in Task 2.9 anyway). The sibling semaphores at
668 / 794 are for different request paths — leave them.

Verify with:
```
grep -n "Semaphore" runtime/src/engine/mod.rs
```

- [ ] **Step 6: Verify tracing**

Wrap `search_scoped` top-level in a `tracing::info_span!("search_scoped", query_id, ?scopes)`
and each per-scope task in its own child span. Log cache hits/misses at `debug`.

- [ ] **Step 7: Commit**

```
git add runtime/src/engine/search_scoped.rs runtime/src/engine/mod.rs
git commit -m "feat(engine): streaming scoped search + timers + tracing"
```

### Task 2.8: Migrate search cache key to `(plugin_id, query_norm, scope, page)`

**Files:**
- Modify: `runtime/src/cache/search.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn key_includes_scope_and_plugin() {
    use stui_plugin_sdk::SearchScope;
    let k1 = SearchKey::new("discogs", "creep", SearchScope::Track, 0);
    let k2 = SearchKey::new("discogs", "creep", SearchScope::Artist, 0);
    assert_ne!(k1, k2);
}

#[test]
fn key_normalizes_query() {
    use stui_plugin_sdk::SearchScope;
    let k1 = SearchKey::new("discogs", "  Creep  ", SearchScope::Track, 0);
    let k2 = SearchKey::new("discogs", "creep", SearchScope::Track, 0);
    assert_eq!(k1, k2);
}
```

- [ ] **Step 2: Replace `SearchKey`**

```rust
use stui_plugin_sdk::SearchScope;

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct SearchKey {
    plugin_id: String,
    query_norm: String,
    scope: SearchScope,
    page: u32,
}

impl SearchKey {
    pub fn new(plugin_id: &str, query: &str, scope: SearchScope, page: u32) -> Self {
        let q: String = query.trim().to_lowercase()
            .split_whitespace().collect::<Vec<_>>().join(" ");
        Self { plugin_id: plugin_id.into(), query_norm: q, scope, page }
    }
}
```

Old key was `(tab, query_norm, page)`. Callers: `Engine::search()` uses the
old one. Replace with per-plugin-per-scope keys called from `search_scoped`:
only finalized per-plugin results populate the cache; partials do not.

- [ ] **Step 3: Update callers** — the legacy `Engine::search` path is
going to be rewritten in Task 2.9; for now, either leave the old code
compiling against a deprecated-but-present constructor, or (preferred) bulk
migrate and delete in one commit once `search_scoped` is wired.

- [ ] **Step 4: Run — pass**

- [ ] **Step 5: Commit**

```
git add runtime/src/cache/search.rs runtime/src/engine/search_scoped.rs
git commit -m "refactor(cache): search cache keyed by (plugin, query, scope, page)"
```

### Task 2.9: Rewrite `pipeline::run_search` to drive streaming via `Event::ScopeResults`

**Files:**
- Modify: `runtime/src/pipeline/search.rs`
- Modify: `runtime/src/main.rs`

- [ ] **Step 1: Update `run_search` signature**

`run_search` no longer returns a single `SearchResult`. Instead, it
consumes the streaming engine and emits `Event::ScopeResults` frames via
the shared `EventSender`.

```rust
pub async fn run_search(
    engine: Engine,
    req: SearchRequest,
    event_tx: EventSender,
) -> Result<(), PipelineError> {
    let cfg = ScopedSearchConfig::default();
    let mut stream = search_scoped(engine, req.query, req.scopes, req.query_id, cfg);
    while let Some(msg) = stream.next().await {
        emit_event(&event_tx, Event::ScopeResults(msg)).await;
    }
    Ok(())
}
```

- [ ] **Step 2: Update IPC dispatcher in `main.rs`** (search for `Request::Search(r)` around line 791)

```rust
Request::Search(req) => {
    let engine = engine.clone();
    let event_tx = event_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = pipeline::search::run_search(engine, req, event_tx).await {
            tracing::warn!(error = %e, "search pipeline failed");
        }
    });
    // No synchronous Response::Search — streaming events carry the result.
}
```

- [ ] **Step 3: Delete the old `catalog_search` fallback** if no caller remains after this change.

```
grep -rn "catalog_search\b" runtime/src/
```

If only `pipeline::search::run_search` called it, remove. Otherwise, leave
for Chunk 7 cleanup.

- [ ] **Step 4: Integration test** — fake IPC client sends `Request::Search`, asserts multiple `Event::ScopeResults` events arrive.

- [ ] **Step 5: Run — iterate until pass**

- [ ] **Step 6: Commit**

```
git add runtime/src/pipeline/search.rs runtime/src/main.rs
git commit -m "feat(pipeline): search streams via Event::ScopeResults"
```

---

## Chunk 3: MPD Bridge Search Path

### Task 3.1: Implement `mpd_bridge::search`

**Files:**
- Create: `runtime/src/mpd_bridge/search.rs`
- Modify: `runtime/src/mpd_bridge/mod.rs` (export)

- [ ] **Step 1: Study existing command patterns**

```
grep -n "pub async fn\|fn search\|fn list_" runtime/src/mpd_bridge/*.rs | head -30
```

Identify how `list_artists` / `list_albums` / `list_songs` are implemented.
Match that style. Identify the quoting helper used.

- [ ] **Step 2: Failing tests**

```rust
// runtime/src/mpd_bridge/search.rs tests module
#[tokio::test]
async fn search_fans_out_per_scope() {
    let mock = MockMpdConn::new()
        .expect_cmd("search artist \"radiohead\"", vec![artist_row("Radiohead")])
        .expect_cmd("search album \"radiohead\"",  vec![])
        .expect_cmd("search title \"radiohead\"",  vec![song_row("Creep", "Radiohead", "Pablo Honey")]);
    let bridge = MpdBridge::with_conn(mock);

    let result = bridge.search(MpdSearchRequest {
        id: "q".into(), query: "radiohead".into(),
        scopes: vec![MpdScope::Artist, MpdScope::Album, MpdScope::Track],
        limit: 200, query_id: 3,
    }).await.unwrap();

    assert_eq!(result.artists.len(), 1);
    assert!(result.albums.is_empty());
    assert_eq!(result.tracks.len(), 1);
    assert_eq!(result.query_id, 3);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn search_skips_disabled_scopes() {
    let mock = MockMpdConn::new().expect_cmd("search artist \"x\"", vec![]);
    let bridge = MpdBridge::with_conn(mock);
    let result = bridge.search(MpdSearchRequest {
        id: "q".into(), query: "x".into(),
        scopes: vec![MpdScope::Artist],
        limit: 200, query_id: 1,
    }).await.unwrap();
    assert!(result.albums.is_empty() && result.tracks.is_empty());
    // Mock panics on unexpected commands — absence of album/title calls verified.
}

#[tokio::test]
async fn disconnected_surfaces_error() {
    let mock = MockMpdConn::disconnected();
    let bridge = MpdBridge::with_conn(mock);
    let result = bridge.search(MpdSearchRequest {
        id: "q".into(), query: "x".into(),
        scopes: vec![MpdScope::Artist, MpdScope::Album, MpdScope::Track],
        limit: 200, query_id: 1,
    }).await.unwrap();
    assert_eq!(result.error, Some(MpdSearchError::NotConnected));
}
```

If the MPD tests don't already have a mock, grep for one:
```
grep -rn "MockMpd\|mock_mpd\|mock_connection" runtime/src/mpd_bridge/
```

- [ ] **Step 3: Implement**

```rust
use tokio::try_join;

impl MpdBridge {
    pub async fn search(&self, req: MpdSearchRequest) -> Result<MpdSearchResult> {
        let conn = match self.connection().await {
            Ok(c) => c,
            Err(_) => return Ok(MpdSearchResult {
                id: req.id, query_id: req.query_id,
                artists: vec![], albums: vec![], tracks: vec![],
                error: Some(MpdSearchError::NotConnected),
            }),
        };

        let want = |s: MpdScope| req.scopes.contains(&s);

        let (artists_rows, albums_rows, tracks_rows) = try_join!(
            async { if want(MpdScope::Artist) { conn.search_tag("artist", &req.query).await } else { Ok(vec![]) } },
            async { if want(MpdScope::Album)  { conn.search_tag("album",  &req.query).await } else { Ok(vec![]) } },
            async { if want(MpdScope::Track)  { conn.search_tag("title",  &req.query).await } else { Ok(vec![]) } },
        ).map_err(|e| anyhow::anyhow!(e))?;

        let artists = artists_rows.into_iter().take(req.limit as usize)
                        .map(MpdArtistWire::from_row).collect();
        let albums = albums_rows.into_iter().take(req.limit as usize)
                        .map(MpdAlbumWire::from_row).collect();
        let tracks = tracks_rows.into_iter().take(req.limit as usize)
                        .map(MpdSongWire::from_row).collect();

        Ok(MpdSearchResult {
            id: req.id, query_id: req.query_id,
            artists, albums, tracks, error: None,
        })
    }
}
```

If `conn.search_tag` doesn't exist, add it alongside `list_*` methods,
matching the project's MPD command style (quoting, escaping).

`from_row` helpers may already exist on the wire types; if not, introduce
them in the same file.

- [ ] **Step 4: Run — iterate**

- [ ] **Step 5: Commit**

```
git add runtime/src/mpd_bridge/search.rs runtime/src/mpd_bridge/mod.rs
git commit -m "feat(mpd_bridge): add search verb with per-scope fan-out"
```

### Task 3.2: Wire `Request::MpdSearch` into the IPC dispatcher

**Files:**
- Modify: `runtime/src/main.rs`

- [ ] **Step 1: Add handler branch**

```rust
Request::MpdSearch(req) => {
    let resp = mpd_bridge.search(req.clone()).await.unwrap_or_else(|e| MpdSearchResult {
        id: req.id.clone(), query_id: req.query_id,
        artists: vec![], albums: vec![], tracks: vec![],
        error: Some(MpdSearchError::CommandFailed { message: e.to_string() }),
    });
    send_response(Response::MpdSearch(resp)).await?;
}
```

- [ ] **Step 2: Integration test** — client sends `MpdSearch`, receives a `Response::MpdSearch`.

- [ ] **Step 3: Run — iterate**

- [ ] **Step 4: Commit**

```
git add runtime/src/main.rs
git commit -m "feat(ipc): dispatch Request::MpdSearch to mpd_bridge"
```

---

## Chunk 4: Go IPC Client (Query ID, Streaming, New Requests)

### Task 4.1: Add Go types and extend `MediaEntry` + `CatalogEntry`

**Files:**
- Modify: `tui/internal/ipc/types.go`
- Modify: `tui/internal/ipc/messages.go`

Both `MediaEntry` (types.go:63) and `CatalogEntry` (messages.go:76) exist
and are used by different paths. Search results flow through `MediaEntry`;
the catalog cache uses `CatalogEntry`. We add `Kind` / `Source` fields to
both so renderers can uniformly consume either.

- [ ] **Step 1: Add string enums** in `types.go`

```go
type SearchScope string
const (
    ScopeArtist  SearchScope = "artist"
    ScopeAlbum   SearchScope = "album"
    ScopeTrack   SearchScope = "track"
    ScopeMovie   SearchScope = "movie"
    ScopeSeries  SearchScope = "series"
    ScopeEpisode SearchScope = "episode"
)

type EntryKind string
const (
    KindArtist  EntryKind = "artist"
    KindAlbum   EntryKind = "album"
    KindTrack   EntryKind = "track"
    KindMovie   EntryKind = "movie"
    KindSeries  EntryKind = "series"
    KindEpisode EntryKind = "episode"
)

type MpdScope string
const (
    MpdScopeArtist MpdScope = "artist"
    MpdScopeAlbum  MpdScope = "album"
    MpdScopeTrack  MpdScope = "track"
)
```

- [ ] **Step 2: Extend `MediaEntry`** (search results)

```go
type MediaEntry struct {
    // …existing fields preserved…

    Kind        EntryKind `json:"kind,omitempty"`
    Source      string    `json:"source,omitempty"`
    ArtistName  string    `json:"artist_name,omitempty"`
    AlbumName   string    `json:"album_name,omitempty"`
    TrackNumber uint32    `json:"track_number,omitempty"`
    Season      uint32    `json:"season,omitempty"`
    Episode     uint32    `json:"episode,omitempty"`
}
```

- [ ] **Step 3: Extend `CatalogEntry`** (catalog cache)

Same `Kind` + `Source` additions. This is used by `music_browse.go` —
verify downstream renderers handle empty `Kind` gracefully for entries
loaded before this change.

- [ ] **Step 4: Add new request/result/event types**

```go
// types.go (or requests.go)
type SearchReq struct {
    ID      string        `json:"id"`
    Query   string        `json:"query"`
    Scopes  []SearchScope `json:"scopes"`
    Limit   uint32        `json:"limit"`
    Offset  uint32        `json:"offset"`
    QueryID uint64        `json:"query_id"`
}

type ScopeResultsMsg struct {
    QueryID uint64       `json:"query_id"`
    Scope   SearchScope  `json:"scope"`
    Entries []MediaEntry `json:"entries"`
    Partial bool         `json:"partial"`
    Error   *ScopeError  `json:"error,omitempty"`
}

type ScopeError struct {
    Type string `json:"type"` // "all_failed" | "no_plugins_configured"
}

type MpdSearchReq struct {
    ID      string     `json:"id"`
    Query   string     `json:"query"`
    Scopes  []MpdScope `json:"scopes"`
    Limit   uint32     `json:"limit"`
    QueryID uint64     `json:"query_id"`
}

type MpdSearchResult struct {
    ID       string         `json:"id"`
    QueryID  uint64         `json:"query_id"`
    Artists  []MpdArtistWire `json:"artists"` // reuse existing Go types
    Albums   []MpdAlbumWire  `json:"albums"`
    Tracks   []MpdSongWire   `json:"tracks"`
    Error    *MpdSearchErr   `json:"error,omitempty"`
}

type MpdSearchErr struct {
    Type    string `json:"type"` // "not_connected" | "command_failed"
    Message string `json:"message,omitempty"`
}
```

`MpdArtistWire`, `MpdAlbumWire`, `MpdSongWire` are the Go-side mirrors of
the Rust wire types — grep `tui/internal/ipc/` for existing definitions and
reuse; if they don't exist Go-side yet, add them matching the Rust field
set.

- [ ] **Step 5: Build**

```
cd tui && go build ./...
```

Fix any callers that constructed `MediaEntry` positionally.

- [ ] **Step 6: Commit**

```
git add tui/internal/ipc/types.go tui/internal/ipc/messages.go
git commit -m "feat(ipc): Go scope/kind enums; add Kind+Source to MediaEntry/CatalogEntry"
```

### Task 4.2: Route `Event::ScopeResults` frames to per-query subscribers

**Files:**
- Modify: `tui/internal/ipc/client.go`

The runtime now emits server-initiated `Event::ScopeResults` frames
(Task 2.2). The Go client needs to decode them and dispatch to subscribers
keyed by `query_id`.

- [ ] **Step 1: Extend decoder**

Find where the client decodes incoming frames (`grep -n "switch.*Type\|Decode\|Unmarshal" tui/internal/ipc/client.go`). Add an `event` branch:

```go
func (c *Client) handleFrame(f Frame) {
    switch f.Type {
    case "response": c.handleResponse(f.Response)
    case "event":
        switch f.Event.Type {
        case "scope_results":
            c.dispatchScopeResults(f.Event.ScopeResults)
        }
    }
}
```

- [ ] **Step 2: Add subscription map**

```go
type Client struct {
    // …existing…
    nextQueryID atomic.Uint64
    scopeSubs   sync.Map // key: uint64 query_id; value: *scopeSub
}

type scopeSub struct {
    ch            chan ScopeResultsMsg
    expectedScope map[SearchScope]struct{}
    remaining     int
    mu            sync.Mutex
}

func (c *Client) NextQueryID() uint64 { return c.nextQueryID.Add(1) }

func (c *Client) SubscribeScopeResults(queryID uint64, scopes []SearchScope) <-chan ScopeResultsMsg {
    expected := make(map[SearchScope]struct{}, len(scopes))
    for _, s := range scopes { expected[s] = struct{}{} }
    sub := &scopeSub{
        ch: make(chan ScopeResultsMsg, 8),
        expectedScope: expected, remaining: len(scopes),
    }
    c.scopeSubs.Store(queryID, sub)
    return sub.ch
}

func (c *Client) dispatchScopeResults(msg ScopeResultsMsg) {
    v, ok := c.scopeSubs.Load(msg.QueryID)
    if !ok { return } // stale; nobody's listening
    sub := v.(*scopeSub)
    select { case sub.ch <- msg: default: /* full → drop */ }
    if !msg.Partial {
        sub.mu.Lock()
        delete(sub.expectedScope, msg.Scope)
        sub.remaining--
        lastOne := sub.remaining == 0
        sub.mu.Unlock()
        if lastOne {
            close(sub.ch)
            c.scopeSubs.Delete(msg.QueryID)
        }
    }
}
```

- [ ] **Step 3: Unit tests**

```go
func TestClient_NextQueryID_Monotonic(t *testing.T) {
    c := newClientForTest()
    ids := []uint64{c.NextQueryID(), c.NextQueryID(), c.NextQueryID()}
    for i := 1; i < len(ids); i++ {
        if ids[i] <= ids[i-1] { t.Fatal("not monotonic") }
    }
}

func TestClient_ScopeResultsRouting(t *testing.T) {
    c := newClientForTest()
    qid := c.NextQueryID()
    ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeArtist, ScopeTrack})
    c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: false})
    c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeTrack,  Partial: false})
    got := []SearchScope{(<-ch).Scope, (<-ch).Scope}
    if _, open := <-ch; open { t.Fatal("channel should be closed after both scopes finalize") }
    _ = got
}

func TestClient_StaleQueryIDDropped(t *testing.T) {
    c := newClientForTest()
    // No subscriber for qid 999
    c.dispatchScopeResults(ScopeResultsMsg{QueryID: 999, Scope: ScopeArtist, Partial: false})
    // Expect: no panic, no side effect. Nothing to assert but the absence of a panic.
}
```

- [ ] **Step 4: Run — iterate**

```
cd tui && go test ./internal/ipc/...
```

- [ ] **Step 5: Commit**

```
git add tui/internal/ipc/client.go tui/internal/ipc/client_test.go
git commit -m "feat(ipc): route ScopeResults events to per-query subscribers"
```

### Task 4.3: Add `Search` and `MpdSearch` client methods

**Files:**
- Modify: `tui/internal/ipc/requests.go`

- [ ] **Step 1: Replace old `Search`**

```go
// OLD: Search(query, options) -> single SearchResult — DELETE
// NEW:
func (c *Client) Search(ctx context.Context, query string, scopes []SearchScope) (uint64, <-chan ScopeResultsMsg, error) {
    qid := c.NextQueryID()
    ch := c.SubscribeScopeResults(qid, scopes)
    req := SearchReq{
        ID: uuid.NewString(), Query: query, Scopes: scopes,
        Limit: 50, Offset: 0, QueryID: qid,
    }
    if err := c.sendFireAndForget(ctx, "Search", req); err != nil {
        c.scopeSubs.Delete(qid)
        return 0, nil, err
    }
    return qid, ch, nil
}

func (c *Client) MpdSearch(ctx context.Context, query string, scopes []MpdScope) (*MpdSearchResult, error) {
    qid := c.NextQueryID()
    req := MpdSearchReq{
        ID: uuid.NewString(), Query: query, Scopes: scopes, Limit: 200, QueryID: qid,
    }
    var resp MpdSearchResult
    if err := c.sendRequestResponse(ctx, "MpdSearch", req, &resp); err != nil {
        return nil, err
    }
    return &resp, nil
}
```

- [ ] **Step 2: Tests with streaming mock**

```go
func TestClient_Search_StreamsResults(t *testing.T) {
    c, server := newClientWithMockServer(t)
    qid, ch, err := c.Search(context.Background(), "creep",
        []SearchScope{ScopeArtist, ScopeTrack})
    if err != nil { t.Fatal(err) }

    server.EmitEvent(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: false, Entries: []MediaEntry{{Title: "Radiohead", Kind: KindArtist}}})
    server.EmitEvent(ScopeResultsMsg{QueryID: qid, Scope: ScopeTrack,  Partial: false, Entries: []MediaEntry{{Title: "Creep",      Kind: KindTrack }}})

    first := <-ch; if first.Scope != ScopeArtist { t.Fatalf("got %v", first.Scope) }
    second := <-ch; if second.Scope != ScopeTrack { t.Fatal() }
    if _, open := <-ch; open { t.Fatal("channel should close") }
}

func TestClient_MpdSearch_SingleResponse(t *testing.T) {
    c, server := newClientWithMockServer(t)
    server.RegisterResponse("MpdSearch", &MpdSearchResult{
        QueryID: 1, Artists: []MpdArtistWire{{Name: "Radiohead"}},
    })
    result, err := c.MpdSearch(context.Background(), "radiohead",
        []MpdScope{MpdScopeArtist, MpdScopeAlbum, MpdScopeTrack})
    if err != nil { t.Fatal(err) }
    if len(result.Artists) != 1 { t.Fatal("wrong artists") }
}
```

- [ ] **Step 3: Run — iterate**

- [ ] **Step 4: Commit**

```
git add tui/internal/ipc/requests.go
git commit -m "feat(ipc): new Search (streaming) and MpdSearch client methods"
```

---

## Chunk 5: Shared CatalogBrowser + DataSource

### Task 5.1: `DataSource` interface and state types

**Files:**
- Create: `tui/internal/ui/screens/catalogbrowser/datasource.go`
- Create: `tui/internal/ui/screens/catalogbrowser/datasource_test.go`

- [ ] **Step 1: Define**

```go
package catalogbrowser

import (
    "context"
    tea "github.com/charmbracelet/bubbletea/v2"
    "stui/tui/internal/ipc"
)

type Entry struct {
    ID          string
    Kind        ipc.EntryKind
    Title       string
    Source      string
    ArtistName  string
    AlbumName   string
    TrackNumber uint32
    Year        uint32
    Duration    uint32
}

type Cursor struct { Column, Row, Scroll int }

type DataSourceState struct {
    Items  map[ipc.EntryKind][]Entry
    Cursor Cursor
}

type SearchStatus struct {
    Active  bool
    Partial bool
    Query   string
    QueryID uint64
}

type DataSource interface {
    Items(kind ipc.EntryKind) []Entry
    Search(ctx context.Context, query string, kinds []ipc.EntryKind) tea.Cmd
    HasMultipleSources() bool
    Snapshot() DataSourceState
    Restore(s DataSourceState)
    Status() SearchStatus
}
```

- [ ] **Step 2: Snapshot round-trip smoke test**

```go
func TestDataSourceState_RoundTrip(t *testing.T) {
    s := DataSourceState{
        Items: map[ipc.EntryKind][]Entry{ipc.KindArtist: {{ID: "a1", Title: "Radiohead"}}},
        Cursor: Cursor{Column: 1, Row: 3},
    }
    s2 := s // value type
    if s2.Cursor.Row != 3 || s2.Items[ipc.KindArtist][0].ID != "a1" {
        t.Fatal("round-trip failure")
    }
}
```

- [ ] **Step 3: Commit**

```
git add tui/internal/ui/screens/catalogbrowser/datasource.go tui/internal/ui/screens/catalogbrowser/datasource_test.go
git commit -m "feat(tui): DataSource interface + state types"
```

### Task 5.2: Extract `CatalogBrowser` from `music_library.go`

**Files:**
- Create: `tui/internal/ui/screens/catalogbrowser/browser.go`
- Modify: `tui/internal/ui/screens/music_library.go`

- [ ] **Step 1: Inventory Music Library's 3-column code**

`music_library.go` is 1597 lines. Identify the pure 3-column rendering +
keybind + focus transition logic (sections likely labeled "columns" /
"render" / "handle key"). Leave tag/dir mode selector, menu integration,
pane-width handling in the library screen.

- [ ] **Step 2: Define `CatalogBrowser` model**

```go
// browser.go
package catalogbrowser

import (
    tea "github.com/charmbracelet/bubbletea/v2"
    "stui/tui/internal/ipc"
)

type Model struct {
    src          DataSource
    kinds        []ipc.EntryKind
    cursor       Cursor
    width, height int
    sourcesCount map[string]int
    picker       *SourcePicker // nil unless open
}

func New(src DataSource, kinds []ipc.EntryKind) Model {
    return Model{src: src, kinds: kinds, sourcesCount: map[string]int{}}
}

func (m Model) Source() DataSource { return m.src }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    if m.picker != nil {
        p, cmd := m.picker.Update(msg)
        m.picker = &p
        return m, cmd
    }
    switch msg := msg.(type) {
    case ScopeResultsApplied:
        return m, nil
    case tea.KeyMsg:
        return m.handleKey(msg)
    case tea.WindowSizeMsg:
        m.width, m.height = msg.Width, msg.Height
    }
    return m, nil
}

func (m Model) View() string { /* N-column render; includes Source/Sources column when enabled */ }

// ScopeResultsApplied is a Bubbletea message that carries a DataSource-applied
// update trigger. Emitted by DataSource.Search tea.Cmds.
type ScopeResultsApplied struct {
    QueryID uint64
    Partial bool
    Followup tea.Cmd
}
```

- [ ] **Step 3: Refactor `music_library.go`**

Replace the raw 3-column state with a `catalogbrowser.Model`. Library-
specific concerns (tag/dir mode selector, menu, pane width) stay. Delegate
render/keys/focus to the component.

- [ ] **Step 4: Build + run existing tests**

```
cd tui && go build ./... && go test ./internal/ui/screens/...
```

- [ ] **Step 5: Manual smoke**

Follow the project Makefile's dev target. Open Music Library; confirm
rendering + navigation unchanged.

- [ ] **Step 6: Commit**

```
git add tui/internal/ui/screens/catalogbrowser/browser.go tui/internal/ui/screens/music_library.go
git commit -m "refactor(tui): extract CatalogBrowser from music_library"
```

### Task 5.3: `MpdDataSource`

**Files:**
- Create: `tui/internal/ui/screens/catalogbrowser/mpd_source.go`
- Create: `tui/internal/ui/screens/catalogbrowser/mpd_source_test.go`

- [ ] **Step 1: Failing tests**

```go
func TestMpdDataSource_SearchUpdatesColumns(t *testing.T) {
    client := &mockIPCClient{
        mpdSearchResult: &ipc.MpdSearchResult{
            QueryID: 1,
            Artists: []ipc.MpdArtistWire{{Name: "Radiohead"}},
            Albums:  []ipc.MpdAlbumWire{{Name: "Pablo Honey"}},
            Tracks:  []ipc.MpdSongWire{{Title: "Creep"}},
        },
    }
    src := NewMpdDataSource(client)
    src.SetInitial(mpdInitial())
    cmd := src.Search(context.Background(), "radiohead",
        []ipc.EntryKind{ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack})
    applied := cmd().(ScopeResultsApplied)
    if applied.QueryID != 1 { t.Fatalf("wrong qid") }
    if len(src.Items(ipc.KindTrack)) != 1 { t.Fatalf("track column empty") }
}

func TestMpdDataSource_RestoreAfterSearch(t *testing.T) {
    src := NewMpdDataSource(&mockIPCClient{ /* … */ })
    src.SetInitial(mpdInitial())
    before := src.Snapshot()
    _ = src.Search(context.Background(), "q",
        []ipc.EntryKind{ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack})()
    src.Restore(before)
    if src.Items(ipc.KindArtist)[0].Title != initialArtistTitle() {
        t.Fatal("restore failed")
    }
}
```

- [ ] **Step 2: Implement**

```go
type MpdDataSource struct {
    client   ipc.Client
    items    map[ipc.EntryKind][]Entry
    snapshot *DataSourceState
    status   SearchStatus
}

func NewMpdDataSource(c ipc.Client) *MpdDataSource {
    return &MpdDataSource{client: c, items: map[ipc.EntryKind][]Entry{}}
}

func (s *MpdDataSource) Items(k ipc.EntryKind) []Entry { return s.items[k] }
func (s *MpdDataSource) HasMultipleSources() bool      { return false }
func (s *MpdDataSource) Status() SearchStatus          { return s.status }

func (s *MpdDataSource) Snapshot() DataSourceState {
    cp := make(map[ipc.EntryKind][]Entry, len(s.items))
    for k, v := range s.items { cp[k] = append([]Entry(nil), v...) }
    return DataSourceState{Items: cp}
}

func (s *MpdDataSource) Restore(st DataSourceState) {
    s.items = st.Items
    s.snapshot = nil
    s.status = SearchStatus{}
}

func (s *MpdDataSource) Search(ctx context.Context, q string, _ []ipc.EntryKind) tea.Cmd {
    if s.snapshot == nil {
        snap := s.Snapshot(); s.snapshot = &snap
    }
    return func() tea.Msg {
        result, err := s.client.MpdSearch(ctx, q,
            []ipc.MpdScope{ipc.MpdScopeArtist, ipc.MpdScopeAlbum, ipc.MpdScopeTrack})
        if err != nil || result.Error != nil {
            return MpdSearchFailed{Err: err, RemoteErr: result != nil && result.Error != nil}
        }
        s.items = map[ipc.EntryKind][]Entry{
            ipc.KindArtist: mapMpdArtists(result.Artists),
            ipc.KindAlbum:  mapMpdAlbums(result.Albums),
            ipc.KindTrack:  mapMpdSongs(result.Tracks),
        }
        s.status = SearchStatus{Active: true, Partial: false, Query: q, QueryID: result.QueryID}
        return ScopeResultsApplied{QueryID: result.QueryID, Partial: false}
    }
}
```

- [ ] **Step 3: Run — iterate**

- [ ] **Step 4: Commit**

```
git add tui/internal/ui/screens/catalogbrowser/mpd_source.go tui/internal/ui/screens/catalogbrowser/mpd_source_test.go
git commit -m "feat(tui): MpdDataSource (snapshot+restore, MpdSearch IPC)"
```

### Task 5.4: `PluginDataSource` with streaming + query_id discard

**Files:**
- Create: `tui/internal/ui/screens/catalogbrowser/plugin_source.go`
- Create: `tui/internal/ui/screens/catalogbrowser/plugin_source_test.go`

- [ ] **Step 1: Glue model**

`PluginDataSource.Search` returns a tea.Cmd that reads one message from the
subscription channel, applies it, and returns a `ScopeResultsApplied{…,
Followup: <next read cmd>}`. `browser.go`'s Update sees `Followup != nil`
and dispatches it, repeating until the channel closes.

- [ ] **Step 2: Failing tests**

```go
func TestPluginDataSource_StreamsPerScope(t *testing.T) {
    client := newStreamingMockClient()
    src := NewPluginDataSource(client)

    cmd := src.Search(context.Background(), "creep",
        []ipc.EntryKind{ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack})

    client.Emit(ipc.ScopeResultsMsg{QueryID: 1, Scope: ipc.ScopeArtist, Partial: false,
        Entries: []ipc.MediaEntry{{Title: "Radiohead", Kind: ipc.KindArtist}}})
    client.Emit(ipc.ScopeResultsMsg{QueryID: 1, Scope: ipc.ScopeTrack,  Partial: true,
        Entries: []ipc.MediaEntry{{Title: "Creep", Kind: ipc.KindTrack}}})
    client.Emit(ipc.ScopeResultsMsg{QueryID: 1, Scope: ipc.ScopeTrack,  Partial: false,
        Entries: []ipc.MediaEntry{{Title: "Creep", Kind: ipc.KindTrack}, {Title: "Creep (Acoustic)", Kind: ipc.KindTrack}}})

    // Drive cmd through each emission.
    applied := drainCmd(cmd)
    if len(applied) != 3 { t.Fatalf("expected 3 applied msgs, got %d", len(applied)) }
    if len(src.Items(ipc.KindTrack)) != 2 { t.Fatalf("track col not merged") }
}

func TestPluginDataSource_DiscardsStaleQueryID(t *testing.T) {
    src := NewPluginDataSource(newStreamingMockClient())
    src.setActive(10)
    applied := src.applyMsg(ipc.ScopeResultsMsg{QueryID: 9, Scope: ipc.ScopeArtist, Partial: false})
    if applied { t.Fatal("stale qid applied") }
}
```

- [ ] **Step 3: Implement**

```go
type PluginDataSource struct {
    client   ipc.Client
    items    map[ipc.EntryKind][]Entry
    snapshot *DataSourceState
    status   SearchStatus
    active   uint64
}

func (s *PluginDataSource) HasMultipleSources() bool { return true }

func (s *PluginDataSource) Search(ctx context.Context, q string, kinds []ipc.EntryKind) tea.Cmd {
    if s.snapshot == nil {
        snap := s.Snapshot(); s.snapshot = &snap
    }
    scopes := scopesFromKinds(kinds)
    qid, ch, err := s.client.Search(ctx, q, scopes)
    if err != nil {
        return func() tea.Msg { return SearchDispatchFailed{Err: err} }
    }
    s.active = qid
    s.status = SearchStatus{Active: true, Partial: true, Query: q, QueryID: qid}
    return s.readNextCmd(ch, qid)
}

func (s *PluginDataSource) readNextCmd(ch <-chan ipc.ScopeResultsMsg, qid uint64) tea.Cmd {
    return func() tea.Msg {
        msg, ok := <-ch
        if !ok {
            s.status.Partial = false
            return SearchChannelClosed{QueryID: qid}
        }
        if msg.QueryID != s.active {
            return StaleScopeDropped{Followup: s.readNextCmd(ch, qid)}
        }
        s.applyMsgInternal(msg)
        return ScopeResultsApplied{
            QueryID: msg.QueryID,
            Partial: msg.Partial,
            Followup: s.readNextCmd(ch, qid),
        }
    }
}

func (s *PluginDataSource) applyMsgInternal(msg ipc.ScopeResultsMsg) {
    kind := kindFromScope(msg.Scope)
    s.items[kind] = append([]Entry(nil), mapMediaEntries(msg.Entries)...)
}
```

- [ ] **Step 4: Run — iterate**

- [ ] **Step 5: Commit**

```
git add tui/internal/ui/screens/catalogbrowser/plugin_source.go tui/internal/ui/screens/catalogbrowser/plugin_source_test.go
git commit -m "feat(tui): PluginDataSource streaming + stale-qid discard"
```

### Task 5.5: `SourcePicker` modal

**Files:**
- Create: `tui/internal/ui/screens/catalogbrowser/source_picker.go`
- Create: `tui/internal/ui/screens/catalogbrowser/source_picker_test.go`

- [ ] **Step 1: Failing tests**

```go
func TestSourcePicker_Rendering(t *testing.T) {
    cs := []Entry{
        {ID: "a", Title: "Creep", Source: "discogs-provider"},
        {ID: "b", Title: "Creep", Source: "lastfm-provider"},
    }
    m := NewSourcePicker("Creep — Radiohead", cs)
    out := m.View()
    for _, want := range []string{"discogs-provider", "lastfm-provider"} {
        if !strings.Contains(out, want) { t.Fatalf("missing %q", want) }
    }
}

func TestSourcePicker_Selection(t *testing.T) {
    cs := []Entry{{ID: "a", Source: "x"}, {ID: "b", Source: "y"}}
    m := NewSourcePicker("t", cs)
    m, _ = m.Update(downKey())
    _, cmd := m.Update(enterKey())
    msg := cmd()
    sel, ok := msg.(SourceSelected)
    if !ok || sel.Entry.ID != "b" { t.Fatalf("got %+v", msg) }
}
```

- [ ] **Step 2: Implement** compact model (cursor, Enter → `SourceSelected`, Esc → `PickerCancelled`)

- [ ] **Step 3: Wire into `browser.go`** — on Enter, group current-row entries by `title+artist+year+kind`; if >1, open picker.

- [ ] **Step 4: Commit**

```
git add tui/internal/ui/screens/catalogbrowser/source_picker.go tui/internal/ui/screens/catalogbrowser/source_picker_test.go tui/internal/ui/screens/catalogbrowser/browser.go
git commit -m "feat(tui): SourcePicker modal for multi-source rows"
```

### Task 5.6: Lazy sources-count resolver (video)

**Files:**
- Create: `tui/internal/ui/screens/catalogbrowser/sources_count.go`
- Create: `tui/internal/ui/screens/catalogbrowser/sources_count_test.go`

- [ ] **Step 1: Failing tests**

```go
func TestSourcesCount_TriggersAfterHover(t *testing.T) {
    res := mockStreamsResolver{result: 42}
    r := newSourcesCountResolver(res, 300*time.Millisecond)
    if cmd := r.OnCursor("X", time.Now()); cmd != nil { t.Fatal("should not resolve instantly") }
    cmd := r.OnTick(time.Now().Add(400 * time.Millisecond))
    upd := cmd().(SourcesCountUpdated)
    if upd.EntryID != "X" || upd.Count != 42 { t.Fatalf("unexpected: %+v", upd) }
}

func TestSourcesCount_CancelledOnCursorMove(t *testing.T) {
    res := mockStreamsResolver{result: 7}
    r := newSourcesCountResolver(res, 300*time.Millisecond)
    r.OnCursor("X", time.Now())
    r.OnCursor("Y", time.Now().Add(100 * time.Millisecond))
    cmd := r.OnTick(time.Now().Add(400 * time.Millisecond))
    upd := cmd().(SourcesCountUpdated)
    if upd.EntryID != "Y" { t.Fatalf("wrong entry: %s", upd.EntryID) }
}
```

- [ ] **Step 2: Implement** state-machine resolver tracking current hover + start time; Tick past threshold fires the Streams-plugin resolve and caches the count per entry id.

- [ ] **Step 3: Wire into grid + CatalogBrowser** for video kinds — render `▸` fallback until count arrives.

- [ ] **Step 4: Commit**

```
git add tui/internal/ui/screens/catalogbrowser/sources_count.go tui/internal/ui/screens/catalogbrowser/sources_count_test.go tui/internal/ui/screens/catalogbrowser/browser.go
git commit -m "feat(tui): lazy sources-count resolver on cursor hover"
```

---

## Chunk 6: Per-Screen Adoption + Top-Bar Routing

### Task 6.1: `Searchable` interface + main-model routing

**Files:**
- Create: `tui/internal/ui/screens/searchable.go`
- Modify: `tui/internal/ui/ui.go`

- [ ] **Step 1: Interface**

```go
// tui/internal/ui/screens/searchable.go
package screens

import (
    tea "github.com/charmbracelet/bubbletea/v2"
    "stui/tui/internal/ipc"
)

type Searchable interface {
    SearchScopes() []ipc.SearchScope
    SearchPlaceholder() string
    StartSearch(query string) tea.Cmd
    OnScopeResults(msg ipc.ScopeResultsMsg) (tea.Model, tea.Cmd)
    OnMpdSearchResult(msg ipc.MpdSearchResult) (tea.Model, tea.Cmd)
    RestoreView() tea.Model
}
```

Package `ui` already imports `screens`; consumers call `screens.Searchable`.
No import cycle.

- [ ] **Step 2: Route `/` in `ui.go`**

Find the current `/` handler (around line 2045). Replace with:

```go
case "/":
    if s, ok := m.focusedScreen().(screens.Searchable); ok {
        m.searchBarVisible = true
        m.search.SetPlaceholder(s.SearchPlaceholder())
        m.state.Focus = state.FocusSearch
    }
    return m, nil
```

- [ ] **Step 3: Wire every keystroke while focused on search input** to a 150ms debounce that calls `focused.(screens.Searchable).StartSearch(query)`.

- [ ] **Step 4: Route streaming messages**

```go
case ipc.ScopeResultsMsg:
    if s, ok := m.focusedScreen().(screens.Searchable); ok {
        upd, cmd := s.OnScopeResults(msg)
        m.setFocusedScreen(upd)
        return m, cmd
    }
case ipc.MpdSearchResult:
    if s, ok := m.focusedScreen().(screens.Searchable); ok {
        upd, cmd := s.OnMpdSearchResult(msg)
        m.setFocusedScreen(upd)
        return m, cmd
    }
```

- [ ] **Step 5: Esc / clear query** → call `RestoreView()`, hide bar, reset state.

- [ ] **Step 6: Integration test** — switch to a non-Searchable screen (Settings), press `/`, confirm bar hidden and no-op.

- [ ] **Step 7: Commit**

```
git add tui/internal/ui/screens/searchable.go tui/internal/ui/ui.go
git commit -m "feat(tui): Searchable interface + focus-scoped / routing"
```

### Task 6.2: Music Library implements `Searchable`

**Files:**
- Modify: `tui/internal/ui/screens/music_library.go`

- [ ] **Step 1: Add methods**

```go
func (s *MusicLibraryScreen) SearchScopes() []ipc.SearchScope {
    return []ipc.SearchScope{ipc.ScopeArtist, ipc.ScopeAlbum, ipc.ScopeTrack}
}
func (s *MusicLibraryScreen) SearchPlaceholder() string { return "Search library…" }
func (s *MusicLibraryScreen) StartSearch(q string) tea.Cmd {
    return s.browser.Source().Search(context.Background(), q,
        []ipc.EntryKind{ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack})
}
func (s *MusicLibraryScreen) OnMpdSearchResult(msg ipc.MpdSearchResult) (tea.Model, tea.Cmd) {
    s.browser.Source().(*catalogbrowser.MpdDataSource).Apply(msg)
    return s, nil
}
func (s *MusicLibraryScreen) OnScopeResults(_ ipc.ScopeResultsMsg) (tea.Model, tea.Cmd) {
    return s, nil // not used by MPD data source
}
func (s *MusicLibraryScreen) RestoreView() tea.Model {
    src := s.browser.Source()
    if snap, ok := src.(*catalogbrowser.MpdDataSource).TakeSnapshot(); ok {
        src.Restore(snap)
    }
    return s
}
```

- [ ] **Step 2: teatest integration**

```go
func TestMusicLibrary_SearchFiltersColumns(t *testing.T) {
    tm := teatest.NewTestModel(t, newMusicLibraryForTest(mockMpdBackend()))
    tm.Send(tea.KeyMsg{Type: tea.KeyRune, Runes: []rune("/")})
    tm.Type("radiohead")
    // wait for debounce + IPC round-trip
    view := getFinalView(t, tm)
    assertColumn(t, view, "Artists", []string{"Radiohead"})
    assertColumn(t, view, "Tracks",  []string{"Creep", "Karma Police"})
    tm.Send(tea.KeyMsg{Type: tea.KeyEsc})
    // view restores
}
```

- [ ] **Step 3: Commit**

```
git add tui/internal/ui/screens/music_library.go
git commit -m "feat(tui): Music Library adopts Searchable (MPD-backed)"
```

### Task 6.3: Music Browse switches to `PluginDataSource`

**Files:**
- Modify: `tui/internal/ui/screens/music_browse.go`

- [ ] **Step 1: Delete `filtered()`** (lines 31-43 per spec exploration) and the local-only substring logic.

- [ ] **Step 2: Construct `CatalogBrowser` with `PluginDataSource`** mirroring Music Library but plugin-backed.

- [ ] **Step 3: Implement `Searchable`**

```go
func (s *MusicBrowseScreen) SearchScopes() []ipc.SearchScope {
    return []ipc.SearchScope{ipc.ScopeArtist, ipc.ScopeAlbum, ipc.ScopeTrack}
}
func (s *MusicBrowseScreen) StartSearch(q string) tea.Cmd { /* delegates to source */ }
func (s *MusicBrowseScreen) OnScopeResults(msg ipc.ScopeResultsMsg) (tea.Model, tea.Cmd) {
    // Forward to source — source is already subscribed to its own channel,
    // but if the main model routes events here we call source.apply(msg).
    s.browser.Source().(*catalogbrowser.PluginDataSource).ApplyMsg(msg)
    return s, nil
}
func (s *MusicBrowseScreen) OnMpdSearchResult(_ ipc.MpdSearchResult) (tea.Model, tea.Cmd) { return s, nil }
func (s *MusicBrowseScreen) RestoreView() tea.Model { /* same pattern */ }
```

- [ ] **Step 4: teatest integration** — streaming mock emits per-scope messages, confirm columns populate incrementally; loading indicator toggles with `partial`.

- [ ] **Step 5: Commit**

```
git add tui/internal/ui/screens/music_browse.go
git commit -m "feat(tui): Music Browse uses CatalogBrowser + PluginDataSource"
```

### Task 6.4: Movies / Series / Library grid adoption

**Files:**
- Locate the grid model: `grep -rn "TabMovies\|TabSeries\|TabLibrary" tui/internal/ui/ | head`
- Likely: `tui/internal/ui/ui.go` main `Model` OR a dedicated grid file (confirm via grep)

Three tabs share the grid today. Each needs `Searchable` with its own scope set. Library tab (per spec §3.3) is plugin-backed today with `[Movie, Series]`.

- [ ] **Step 1: Create `GridDataSource`** — single- or multi-kind plugin-backed source analogous to `PluginDataSource` but without the 3-column assumption.

```go
// tui/internal/ui/screens/catalogbrowser/grid_source.go
type GridDataSource struct { /* like PluginDataSource but flat item list */ }
```

- [ ] **Step 2: Movies Searchable**

```go
// on the Movies grid model:
func (s *MoviesScreen) SearchScopes() []ipc.SearchScope { return []ipc.SearchScope{ipc.ScopeMovie} }
func (s *MoviesScreen) SearchPlaceholder() string        { return "Search movies…" }
// …
```

- [ ] **Step 3: Series Searchable** with `[ipc.ScopeSeries]`.

- [ ] **Step 4: Library Searchable** with `[ipc.ScopeMovie, ipc.ScopeSeries]` — confirm the default grid layout can render interleaved Movie + Series results, or keep it simple and treat Library search as flat union.

- [ ] **Step 5: Add `Sources` column** — reuse the lazy resolver from Task 5.6. Column header `Sources`; cell `▸` → count after hover.

- [ ] **Step 6: teatest integration per tab — `replace` AND `restore`**

```go
func TestMovies_SearchReplacesGrid(t *testing.T)   { /* type, assert grid shows matches */ }
func TestMovies_SearchRestoreOnEsc(t *testing.T)   { /* verify restore */ }
func TestSeries_SearchReplacesGrid(t *testing.T)   { /* … */ }
func TestSeries_SearchRestoreOnEsc(t *testing.T)   { /* … */ }
func TestLibrary_SearchSpansMovieAndSeries(t *testing.T) { /* … */ }
func TestLibrary_SearchRestoreOnEsc(t *testing.T)  { /* … */ }
```

Restore tests must exist for every screen adopting `Searchable` (spec §7.2).

- [ ] **Step 7: Commit (one per tab, or batched — author's choice)**

```
git commit -m "feat(tui): Movies/Series/Library grids adopt Searchable + Sources column"
```

### Task 6.5: Remove legacy search toggles and doc-comment update

**Files:**
- Modify: `tui/internal/ui/screens/search.go`
- Modify: `tui/internal/ui/root.go` (doc comment only)

- [ ] **Step 1: Delete `searchAll` field + its handling** (lines 42, 114, 181, 220, 233 per spec exploration)

- [ ] **Step 2: Evaluate `SearchScreen` struct**

Check callers:
```
grep -rn "SearchScreen\|NewSearchScreen" tui/
```

If only the `root.go:22` doc comment references it, and the constructor has
no live usage, delete `SearchScreen` entirely. Update the `root.go:22` doc
comment to use a current transition example instead (e.g., to
`NewSettingsScreen` or another live constructor).

If `SearchScreen` has live usage elsewhere (unlikely per spec), prefer
leaving a thin stub over partial deletion.

- [ ] **Step 3: Build + tests**

```
cd tui && go build ./... && go test ./...
```

- [ ] **Step 4: Commit**

```
git add tui/internal/ui/screens/search.go tui/internal/ui/root.go
git commit -m "refactor(tui): remove searchAll toggle and unused SearchScreen"
```

---

## Chunk 7: Plugin Migration + Final Cleanup

### Task 7.1: Migrate each plugin to declare kinds and honor scope

**Plugins in scope (14):** `anilist-provider`, `discogs-provider`,
`imdb-provider`, `javdb`, `kitsu`, `kitsunekko`, `lastfm-provider`,
`listenbrainz-provider`, `omdb-provider`, `r18`, `subscene`,
`tmdb-provider`, `torrentio-rpc`, `yify-subs`.

For each plugin:

- [ ] **Step 0: Verify the plugin's actual ID**

```
grep -h '^name' plugins/<plugin-dir>/plugin.toml
```

The `name` value is what shows up in the dispatch map, cache keys, and
`PluginEntry.source`. Some directories carry a shorter name than the folder
(e.g., `discogs-provider/` → `name = "discogs"`). Use the `name` value
throughout — not the directory basename.

- [ ] **Step 1: Declare `catalog.kinds`** in `plugin.toml`. Indicative mapping (confirm plugin-by-plugin):

| Plugin                | `catalog.kinds`                     | Notes |
|-----------------------|--------------------------------------|-------|
| `anilist-provider`    | `["movie", "series"]`                | anime |
| `discogs-provider`    | `["artist", "album", "track"]`       | music metadata |
| `imdb-provider`       | `["movie", "series", "episode"]`     | |
| `javdb`               | `["movie"]`                          | adult |
| `kitsu`               | `["movie", "series"]`                | anime |
| `kitsunekko`          | — (subtitles only, no catalog)       | |
| `lastfm-provider`     | `["artist", "album", "track"]`       | |
| `listenbrainz-provider` | `["artist", "album", "track"]`     | |
| `omdb-provider`       | `["movie", "series"]`                | |
| `r18`                 | `["movie"]`                          | adult |
| `subscene`            | — (subtitles only, no catalog)       | |
| `tmdb-provider`       | `["movie", "series", "episode"]`     | |
| `torrentio-rpc`       | — (stream provider, not catalog)     | confirm |
| `yify-subs`           | — (subtitles only)                   | |

- [ ] **Step 2: Update `impl StuiPlugin::search`** in each plugin's `src/lib.rs`:
  - Read `req.scope`.
  - If unsupported, return `PluginResult::err(error_codes::UNSUPPORTED_SCOPE, "…")`.
  - Else call the plugin's typed fetcher, set `kind` + `source` on every returned entry, and (where applicable) populate per-kind fields (`artist_name`, `album_name`, `season`, `episode`).

- [ ] **Step 3: Unit test per plugin** asserting (a) scope-filter behavior and (b) populated `kind`/`source`. Mock the external API via the project's existing test harness (`httpmock` or similar — grep existing plugin tests).

- [ ] **Step 4: Build plugins**

```
cd plugins && cargo build --target wasm32-wasip1 --workspace
```

- [ ] **Step 5: Commit per plugin**

```
git commit -m "feat(plugin/discogs): declare catalog.kinds, honor scope"
```

### Task 7.2: Concurrency + tracing + cache verification

**Files:** (verification only, no new code)

- [ ] **Step 1: Confirm semaphore still bounds concurrency**

Re-run the slow-plugin test from Task 2.7 and verify, with `tracing` at
debug, that concurrent plugin calls never exceed the existing max-8 cap.
If they do, move the permit acquisition into `supervisor_search`.

- [ ] **Step 2: Confirm cache behavior**

Run a search twice with same query/scope. Second run should show cache
hits at debug log level for every plugin's contribution. Partial emissions
should NOT appear as cache entries.

- [ ] **Step 3: Confirm tracing spans**

Spans visible: `search_scoped{query_id, scopes}` with child per-scope
spans and per-plugin child spans.

### Task 7.3: End-to-end smoke

- [ ] **Step 1: Run full matrix**

```
cd sdk && cargo test
cd runtime && cargo test
cd tui && go test ./...
cd plugins && cargo build --target wasm32-wasip1 --workspace
```

- [ ] **Step 2: Manual smoke checklist** (mirror spec §7.3)

- Music Library: type a known artist; all three columns filter; Esc restores; cursor preserved.
- Music Browse: type "creep" against real plugins; streamed per-column population; `SourcePicker` on duplicates.
- Movies (TMDB): type "matrix"; typed Movie results; lazy `Sources` count after cursor focus.
- Series: similar.
- Library: spans Movies + Series.
- Disconnected MPD + Music Library search → inline error banner.
- Injected slow plugin (use a test fixture with artificial delay) → fast-plugin columns populate independently; partial indicator visible.

- [ ] **Step 3: `cargo clippy --all-targets`** and `go vet ./...` — resolve any lints.

- [ ] **Step 4: Final chore commit if stragglers**

```
git commit -m "chore: finalize search refactor cleanup"
```

---

## Execution Notes

- **Chunk order matters.** 1 → 2 (SDK types before manifest + engine) → 3 (MPD, independent) → 4 (Go IPC needs Rust schema frozen) → 5 (components need client methods) → 6 (screens need components) → 7 (plugins + verification).

- **Commit cadence.** Per project convention, the user commits on their own cadence; don't auto-commit. Each `git commit` step is authorial intent, not automatic. Surface to user before committing if unsure.

- **Plugin refactor is in-scope** (Chunk 7). The spec explicitly places this here so plugins don't need two refactor passes.

- **XDG migration not touched.** No new paths under `~/.config/stui/`; everything stays at `~/.stui/`.

- **Library tab caveat.** Today plugin-backed; tomorrow local-indexer-backed. Design is compatible — only the `DataSource` implementation swaps when the indexer arrives.

- **IPC streaming primitive** (Task 2.2) is infrastructure that outlives this feature. Future progress events (tag writes, plugin crashes, downloads) can reuse it.

- **Name conventions.** Throughout the plan, `sdk::SearchRequest` and `ipc::v1::SearchRequest` are distinct; `MediaEntry` and `CatalogEntry` on the Go side are distinct; wire types are `MpdArtistWire`/`MpdAlbumWire`/`MpdSongWire` (no `LibraryEntry`). Re-read the Naming Disambiguation section at the top if confused.
