# STUI Search Refactor — Design

Date: 2026-04-19
Status: Draft (brainstormed, pending review)

## 1. Summary & Goals

Replace STUI's current global `/` search with focus-scoped search that routes
through each screen's own handler, and extend the plugin ABI + MPD bridge so
music queries actually return typed artist/album/track results.

### Current state (as verified in code)

- `/` triggers an inline top-bar `textinput.Model`. The `SearchScreen` struct
  at `tui/internal/ui/screens/search.go` exists but is disabled (constructor
  commented out in `tui/internal/ui/root.go:22`).
- Movies / Series / Library tabs: inline search → IPC `Request::Search` →
  `pipeline::run_search` → `engine.search()` fan-out across ~14 plugins with
  real API implementations (anilist, discogs, imdb, javdb, kitsu, kitsunekko,
  lastfm, listenbrainz, omdb, r18, subscene, tmdb, torrentio-rpc, yify-subs).
  Functional.
- Music Browse (`tui/internal/ui/screens/music_browse.go:31`): local
  substring `filtered()` on a cached catalog only. Does NOT invoke the engine
  pipeline. Results are static relative to the cache.
- Music Library (`tui/internal/ui/screens/music_library.go`): no search at
  all. MPD bridge exposes only `list*` / `browse` verbs — no `find` / `search`
  passthrough.
- Cross-tab global search toggle (`a` key) fans across Movies+Series+Library
  simultaneously.
- No focus-aware dispatch: `activeTab` is passed to `SearchScreen` but not
  used for routing decisions.

### What changes

1. **Global cross-tab search is removed.** The `a`-toggle and the generic
   fan-out-to-every-tab dispatcher are deleted. `/` always scopes to the
   focused screen.
2. **`Searchable` interface** gates search per-screen. Screens that don't
   implement it hide the search bar and ignore `/`.
3. **Shared `CatalogBrowser` component** is extracted from Music Library's
   mature 3-column browser and reused in Music Browse. Both screens become
   thin adapters around a data source.
4. **`DataSource` abstraction** lets `CatalogBrowser` drive from MPD (Library)
   or plugins (Browse) uniformly.
5. **Plugin manifest declares supported kinds** (`artist`, `album`, `track`,
   `movie`, `series`, `episode`). This is the forward-looking target for the
   pending plugin refactor.
6. **`SearchRequest` gains a required `scope` field**; `PluginEntry` gains an
   `entry_kind` field. Runtime dispatches per-scope only to plugins whose
   declared kinds match.
7. **MPD bridge grows a `search` verb** (thin passthrough to MPD's native
   `search` command) for Music Library.
8. **Streaming:** per-scope batched `ScopeResultsMsg` emitted as each scope's
   plugins settle (or hit a partial deadline). No more single-shot blocking
   result.
9. **Snapshot-and-restore:** clearing the search input or pressing Esc
   restores the pre-search view (cursor, scroll, selection).

### Why this sequencing

By designing search first, we lock in the plugin manifest shape (declared
kinds, scoped search verb) *before* doing the plugin refactor — so we refactor
plugins once against a known target rather than iteratively.

### Explicit non-goals

- Plugin performance rating → future input to adaptive partial-deadline
  timing.
- Plex / Jellyfin / other future providers — plug in later via their own
  bridges or plugin ABI and inherit the same screen contract.
- XDG migration (`~/.stui/` → `~/.config/stui/`) — unchanged. No new paths
  under `~/.config/stui/`.
- Music Library tag/directory mode UI changes.
- Library tab's maturation into a proper local-video library (indexer,
  SQLite store, etc.) — out of scope, but the design is compatible: the
  Library screen adopts `Searchable` with a future `VideoLibraryDataSource`
  when that work lands.
- Global / universal search — deleted. Revisit only if real usage demands it.
- Playlists as a first-class search scope — deferred; Browse redesign
  will give playlists their own surface.
- Genre as a search scope — genre is a filter dimension, not a scope.

## 2. Architecture & Components

Six abstractions, ordered UI → plugin runtime.

### 2.1 `Searchable` interface (Go, TUI)

```go
type Searchable interface {
    SearchScope() []ipc.SearchScope   // Artists, Albums, Tracks — or Movies, etc.
    SearchPlaceholder() string         // "Search library…" etc.
    StartSearch(query string) tea.Cmd  // debounced dispatch
    OnScopeResults(msg ScopeResultsMsg) (tea.Model, tea.Cmd)
    RestoreView() tea.Model            // Esc / clear restores pre-search snapshot
}
```

Main model probes focused screen for this interface. If absent → `/` no-op,
search bar hidden. If present → `/` focuses the top-bar input, placeholder
adapts, streamed results route to `OnScopeResults`.

### 2.2 `DataSource` interface (Go, shared)

```go
type DataSource interface {
    Items(kind EntryKind) []Entry
    Search(query string, kinds []EntryKind) tea.Cmd
    HasMultipleSources() bool          // drives source-column visibility
    Snapshot() DataSourceState
    Restore(DataSourceState)
}
```

Two implementations:

- `MpdDataSource` — dispatches `Request::MpdSearch` over IPC; single-message
  result; `HasMultipleSources == false`.
- `PluginDataSource` — dispatches `Request::Search` with scopes; receives
  streamed `ScopeResultsMsg`s; `HasMultipleSources == true`.

A future `VideoLibraryDataSource` slots in the same way when the Library tab
matures.

### 2.3 `CatalogBrowser` component

Extracted from `music_library.go`. The 3-column Artists | Albums | Tracks
browser becomes a self-contained Bubbletea component taking a `DataSource`
at construction. Adds:

- Optional `Source` / `Sources` column driven by `DataSource.HasMultipleSources()`.
- `SourcePicker` sub-component for music (Y grouping from §2.5).
- Lazy Sources-count resolution for video (Y from §2.5).

Both Music Library and Music Browse render through this. Movies / Series /
Library tabs keep their grid layout but also consume a `DataSource` and
adopt the `Source`/`Sources` column.

### 2.4 SDK types (`sdk/src/lib.rs`)

```rust
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Artist, Album, Track, Movie, Series, Episode,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    Artist, Album, Track, Movie, Series, Episode,
}

pub struct SearchRequest {
    pub query: String,
    pub scope: SearchScope,
    pub page: u32,
    pub limit: u32,
    pub per_scope_limit: Option<u32>,
    pub locale: Option<String>,
}

pub struct PluginEntry {
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    pub source: String,
    // preserved
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub rating: Option<f32>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub imdb_id: Option<String>,
    pub duration: Option<u32>,
    // new optional per-kind fields
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub track_number: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}
```

Two enums (`EntryKind`, `SearchScope`) identical today, kept decoupled because
they occupy different roles: `EntryKind` describes what a returned entry *is*
(wire contract on `PluginEntry`), while `SearchScope` describes what the
caller is asking for (request parameter). Near-term concrete use for the
split: a runtime-only `SearchScope::Any` may be introduced for MPD bridge
internal dispatch (see §5.1) without polluting `EntryKind`. If after
implementation the split proves unused, collapse to one enum in a followup —
cost is low either way.

Flat `PluginEntry` with optional per-kind fields, not a tagged enum — better
FFI ergonomics, cheaper serialization, trivial for plugin authors.

Removed from the plugin ABI: `tab` (implicit in scope), `genre` /
`min_rating` / `year_range` filters (move to runtime-side post-filters).

### 2.5 Plugin manifest (`plugin.toml`)

```toml
[plugin]
name = "spotify-provider"
version = "0.1.0"

[capabilities]
catalog = { kinds = ["artist", "album", "track"] }
streams = true
# subtitles, auth, index unchanged
```

Runtime parses `catalog.kinds` at plugin load. Builds dispatch map
`SearchScope → [plugin_id]`. Plugins without declared kinds receive zero
search calls — strict opt-in, not permissive fallback. Prevents accidental
wrong-scope calls during the refactor.

### 2.6 MPD bridge search (`runtime/src/mpd_bridge/search.rs`)

New file. Translates `MpdSearchRequest { query, scopes, limit }` into
concurrent MPD protocol calls (`search artist <q>`, `search album <q>`,
`search title <q>`). Uses `search` (case-insensitive substring) not `find`
(exact). Returns typed `LibraryEntry` lists keyed by scope.

No caching at this layer. MPD is local and fast enough (single-digit ms on
a moderate library). Library mutation invalidates nothing because there's
nothing to invalidate.

### 2.7 Streaming search engine (`runtime/src/engine/search_scoped.rs`)

Replaces current flat `engine.search()`. Signature:

```rust
pub async fn search_scoped(
    &self,
    query: String,
    scopes: Vec<SearchScope>,
    query_id: String,
) -> impl Stream<Item = ScopeResultsMsg>
```

Per scope: concurrent fan-out only to plugins whose declared kinds include
the scope. Two timers bound the latency model:

- **Partial deadline (default 500ms):** starts on the *first* plugin response
  within a scope. When it expires, emit a `ScopeResultsMsg { partial: true }`
  with results collected so far. Late plugins continue in the background.
- **Hard floor (default 2000ms):** starts at dispatch. If no plugin has
  responded by then, emit an empty `ScopeResultsMsg { partial: true }` so
  the UI shows *something* instead of a blank column. Late stragglers still
  emit a follow-up message when they land.

A finalized `ScopeResultsMsg { partial: false }` is emitted per scope when
every plugin has responded or timed out. Partial deadline and hard floor are
both configurable. See §6.4 for the full edge-case table.

Cache key: `(plugin_id, query_norm, scope, page)`. Cache stores only
finalized per-plugin results; partials are never cached.

The old cross-tab `search()` path and its toggle UI are deleted.

### 2.8 Code being deleted

- `searchAll bool` field and `a`-toggle handler in
  `tui/internal/ui/screens/search.go:42,114` plus its conditional branches
  at lines 181/220/233.
- Disabled `SearchScreen` overlay struct (was never wired in; lives as dead
  code today).
- `music_browse.go::filtered()` — replaced by `PluginDataSource.Search`.
- Legacy catalog fallback in `runtime/src/pipeline/search.rs` once scoped
  plugin engine is authoritative. (Retain only if implementation reveals a
  specific need.)

## 3. Data Flow

### 3.1 Flow A — Music Library search (MPD-backed)

1. Focused screen is `MusicLibraryScreen`, implements `Searchable` with
   `scope = [Artist, Album, Track]`. `/` focuses top-bar input; placeholder
   becomes `Search library…`.
2. Keystroke → 150ms debounce → `StartSearch("radiohead")` → screen calls
   `MpdDataSource.Search(...)` → IPC `Request::MpdSearch { id, query, scopes,
   limit }`.
3. Runtime IPC handler dispatches to `mpd_bridge::search`. Bridge issues
   three concurrent MPD commands (`search artist`, `search album`,
   `search title`).
4. Bridge collects, maps to typed `Entry { kind, source = "Local", … }`,
   returns `MpdSearchResult { artists, albums, tracks }`.
5. Single `ScopeResultsMsg` (MPD is fast; no streaming needed) posted to TUI.
6. `MusicLibraryScreen.OnScopeResults` updates `DataSource` view →
   `CatalogBrowser` re-renders filtered columns in place. Source column
   hidden.
7. Esc / cleared input → `RestoreView()` → `DataSource.Restore(snapshot)` →
   original hierarchical view with prior cursor/scroll.

### 3.2 Flow B — Music Browse search (plugin-backed)

1. Focused screen is `MusicBrowseScreen`, implements `Searchable` with same
   scope as Library.
2. Debounced query → `PluginDataSource.Search("creep", [Artist, Album, Track])`
   → IPC `Request::Search { query, scopes, stream: true, query_id }`.
3. Runtime engine consults dispatch map. Example: Artists scope → Last.fm,
   Discogs, Spotify; Tracks scope → Spotify, SoundCloud, Last.fm.
4. Engine fan-outs per scope × per plugin. Each call goes through
   `WasmSupervisor` (30s timeout). Results cached at
   `(plugin_id, "creep", scope, 0)`.
5. As each scope settles (or hits partial deadline ~500ms after first
   response), emits `ScopeResultsMsg { query_id, scope, entries, partial,
   sources_count_per_entry }`.
6. TUI receives messages as they arrive. `MusicBrowseScreen.OnScopeResults`
   routes each to the right column of `CatalogBrowser`. Column shows a
   loading indicator while `partial==true`. Source column visible; each row
   shows its producing plugin.
7. Enter on a row with `sources_count > 1` → `SourcePicker` modal lists all
   matching entries grouped by title+artist+year heuristic. User picks →
   existing `ResolveRequest` fires against that plugin.
8. Esc / cleared query → `RestoreView()` restores pre-search catalog +
   cursor.

### 3.3 Flow C — Movies / Series / Library (plugin-backed, grid)

Same as Flow B but `scopes = [Movie]` / `[Series]` (typically single scope).
Grid-layout screens adopt the `Sources` column (count, not plugin-name —
rationale in §3.5). `CatalogBrowser` not required for these screens; the
grid renderer consumes the same `DataSource`.

### 3.4 Query-id & cancellation

- Monotonic integer counter on the TUI side, incremented per new query.
  Included in each outbound request and echoed back in each
  `ScopeResultsMsg` / `MpdSearchResult`.
- TUI drops results whose id doesn't match current query. Late messages
  silently ignored; logged at debug.
- Runtime-side: in-flight WASM plugin calls are NOT actively cancelled.
  Supervisor timeout bounds them. Stale results still populate cache; UI-side
  discard via query_id. Net effect: minor wasted work; correctness preserved;
  complexity avoided.

### 3.5 Source / Sources column — music vs video asymmetry

- **Music Browse:** column header = `Source`, cell shows plugin/source name
  (Spotify, SoundCloud, YouTube). Plugin identity ≈ stream source.
- **Movies / Series / Library:** column header = `Sources` (plural), cell
  shows a count of streamable sources (from `Streams`-capable plugins, not
  `Catalog` plugins — they're disjoint). Lazy-resolved on cursor focus
  >300ms (Y option). Column shows `▸` until resolved; count replaces chevron
  when Streams plugins respond.
- **Music Library (MPD):** no column (`HasMultipleSources == false`).

Dedup / grouping differs too:

- Music Browse: dedup across Catalog plugins by title+artist+year heuristic
  → single row per logical track with `SourcePicker` of contributing plugins.
- Video: dedup across Catalog plugins silently (TMDB/OMDb/IMDb for same IMDb
  id collapse to one row). `Sources` count refers to Streams-plugin output,
  not Catalog plugins.

### 3.6 Latency budget (design target)

- MPD search (Flow A): ~10–50ms typical; no streaming needed.
- Plugin-backed first column populated: within partial deadline (~500ms
  after first plugin response).
- Slowest column: bounded by supervisor timeout (30s); in practice capped
  by partial deadline + final-message emission.
- User keystroke → first visible result: debounce (150ms) + first plugin RTT
  (~100–300ms) + partial deadline (~500ms) ≈ 750ms–1s. Feels responsive.

## 4. Plugin ABI & Manifest

Detail for §2.4–§2.5. See those sections for type definitions.

### 4.1 `search` semantics (tightened, signature unchanged)

```rust
pub trait StuiPlugin {
    fn manifest(&self) -> PluginManifest;
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse>;
    fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse>;
}
```

`search` MUST honor `req.scope`:

- Return only entries where `entry.kind == req.scope`.
- If the plugin doesn't support the requested scope, return
  `PluginResult::Err(PluginError::UnsupportedScope)`. Runtime logs at debug
  and drops the scope's contribution from this plugin. Should be rare — the
  dispatch map already filters by declared kinds; this catches stale
  manifest vs runtime divergence.

### 4.2 Dispatch map construction

Built at plugin load from each plugin's `plugin.toml`:

```
SearchScope::Artist → [spotify-provider, lastfm-provider, discogs-provider]
SearchScope::Album  → [spotify-provider, lastfm-provider, discogs-provider]
SearchScope::Track  → [spotify-provider, soundcloud-provider, lastfm-provider]
SearchScope::Movie  → [tmdb-provider, omdb-provider, imdb-provider]
SearchScope::Series → [tmdb-provider, imdb-provider, kitsu-provider, anilist-provider]
```

(Indicative — actual assignment depends on what each plugin declares in
its refactored manifest.)

Plugins without declared kinds are not dispatched to. Strict opt-in.

### 4.3 Migration lint (nice-to-have, not required for this refactor)

`stui check-plugin <path>` can flag plugins missing `capabilities.catalog.kinds`
so the plugin-refactor team can sweep them before search lands live. Not
blocking; tracked separately.

### 4.4 Rejected alternatives (for the record)

- `SearchRequest { scope: Vec<SearchScope> }` (multi-scope per call): awkward
  for per-scope caching and per-scope streaming. Cleaner to call once per
  scope.
- `PluginEntry` as tagged `enum EntryKind::Artist(ArtistEntry) | …`: more
  type-safe but worse FFI ergonomics; forces plugin authors to learn a
  tagged-union serialization dance. Not worth it.
- Typed verbs (`search_artists` / `search_albums` / etc.) on the plugin
  trait: bloats the trait surface and couples it tightly to media taxonomy.
  Rejected in favor of capability-declared scope.

## 5. MPD Bridge Search

Small, contained. See §2.6 for overview.

### 5.1 IPC additions (`runtime/src/ipc/v1/mod.rs`)

```rust
pub enum Request {
    // …existing…
    MpdSearch(MpdSearchRequest),
}

pub struct MpdSearchRequest {
    pub id: String,
    pub query: String,
    pub scopes: Vec<MpdScope>,
    pub limit: u32,
}

pub enum MpdScope { Artist, Album, Track }

pub struct MpdSearchResult {
    pub id: String,
    pub artists: Vec<LibraryEntry>,
    pub albums: Vec<LibraryEntry>,
    pub tracks: Vec<LibraryEntry>,
    pub error: Option<MpdSearchError>,
}
```

`LibraryEntry` is the existing shape used by `list*` responses — no new
types.

### 5.2 Bridge implementation sketch

```rust
impl MpdBridge {
    pub async fn search(&self, req: MpdSearchRequest) -> Result<MpdSearchResult> {
        let conn = self.connection().await?;

        let (artists, albums, tracks) = tokio::try_join!(
            maybe_search(conn, "artist", &req.query, req.limit, &req.scopes, MpdScope::Artist),
            maybe_search(conn, "album",  &req.query, req.limit, &req.scopes, MpdScope::Album),
            maybe_search(conn, "title",  &req.query, req.limit, &req.scopes, MpdScope::Track),
        )?;

        Ok(MpdSearchResult { id: req.id, artists, albums, tracks, error: None })
    }
}
```

### 5.3 MPD protocol notes

- `search` (case-insensitive substring) is the default.
- `find` (exact, case-sensitive) NOT exposed now. Future flag `exact: bool`
  if needed.
- Single-tag form per scope. No multi-tag queries in this pass.
- MPD's natural ordering preserved; no re-ranking in the bridge.

### 5.4 Edge cases

- Empty query: bridge returns empty result; TUI's debounce + snapshot/restore
  handles this on the UI side.
- MPD disconnected: bridge returns `MpdSearchResult { error:
  Some(NotConnected), … }`; TUI renders inline "MPD disconnected" state on
  the `CatalogBrowser` columns.
- Query with special characters: reuses existing MPD quoting helper.

### 5.5 No caching

MPD is local. Cache adds invalidation complexity without measurable win on
current library sizes. Revisit only if profiling shows repeated-identical-
query cost matters.

## 6. Error Handling, Edge Cases, Cache

### 6.1 TUI

- `/` on non-`Searchable` screen → no-op. Search bar hidden; keybind silently
  swallowed.
- Debounce pending when user presses Esc → cancel pending `tea.Cmd`; no
  request fires.
- `ScopeResultsMsg` with stale `query_id` → dropped silently; debug-logged.
- `RestoreView()` with no snapshot → no-op.

### 6.2 Runtime — plugin engine (Flow B)

- `PluginError::UnsupportedScope` from a plugin → log at debug; exclude from
  merged scope result.
- Plugin panic / crash / timeout → supervisor handles (existing auto-reload
  behavior). Engine treats as empty for that (plugin, scope). Other plugins
  still render.
- All plugins in a scope fail → emit `ScopeResultsMsg { scope, entries: [],
  partial: false, error: Some(AllFailed) }`. Column renders "No results —
  scope unavailable."
- No plugins declared for a scope → emit same empty-not-partial message
  immediately. Column renders "No sources configured."

### 6.3 Runtime — MPD bridge (Flow A)

- MPD disconnected → `MpdSearchResult { error: Some(NotConnected) }`; TUI
  renders inline banner.
- MPD command error (malformed / permission / etc.) → logged; returned as
  scope-level error; affected scopes empty, others may still succeed.

### 6.4 Partial deadlines

- Partial deadline starts when the *first* plugin response arrives in a
  scope. Configurable; default 500ms.
- No plugin responds within 2000ms (configurable hard floor) → emit empty
  `ScopeResultsMsg { partial: true }`; keep the door open for stragglers;
  emit a follow-up finalized message when they land or time out.
- Every plugin has responded or timed out → emit `partial: false`; column
  loading indicator clears.
- UI always respects `partial` flag.

### 6.5 Cache

- Plugin-engine cache key: `(plugin_id, query_norm, scope, page)`. TTL 2h
  (existing value).
- Invalidation:
  - Plugin reload / crash / restart → clear that plugin_id's entries.
  - TTL expiry.
  - Manual refresh (no keybind today; reserved for future).
- Partial results NOT cached. Cache stores only finalized per-scope
  per-plugin results. Correctness over micro-latency; mid-partial Esc +
  retype gets a normal round-trip, not a fast stale replay.
- Stale-query results still populate the cache: if the user types "creep",
  then quickly retypes to "creeper" before plugin A finishes, plugin A's
  "creep" response lands at cache key `(A, "creep", scope, 0)` even though
  the TUI discarded it via `query_id`. This is intentional — a future
  retype of "creep" can hit that cache entry. No downside beyond the
  work already done.
- Query normalization: lowercase + trim + collapse whitespace. No Unicode
  diacritic folding (defer unless users complain).

### 6.6 Concurrency

- Existing semaphore in `engine/mod.rs:361` (max 8 concurrent plugin calls
  globally) retained. Works across the entire engine, not per-scope; a
  3-scope × 6-plugin query = 18 queued through 8 slots. Acceptable at
  single-user typing cadence. Config knob if future profiling shows a
  bottleneck.

### 6.7 Tracing

- `search_scoped` wraps each invocation in a trace span:
  `query_id`, `scopes`, `plugin_ids_targeted`. Matches existing tracing
  patterns.
- Cache hit/miss logged at debug per plugin. Useful as input to the future
  plugin-performance-rating work.

## 7. Testing

### 7.1 Rust / runtime

- **Unit — MPD bridge search:** mock connection; verify scope → MPD command
  mapping; concurrent dispatch; query quoting; disconnected-state returns
  structured error (not panic).
- **Unit — dispatch map:** registry of plugins with various declared kinds;
  verify `scope → [plugin_id]` correctness; plugins with empty / missing
  `kinds` receive zero calls.
- **Unit — scope streaming:** first response triggers partial timer; partial
  message emitted at deadline; finalized message emitted when last plugin
  settles; 2000ms hard floor emits empty partial; slow plugin doesn't block
  faster scopes; cache only populated with finalized results.
- **Unit — cache key normalization:** whitespace / case variants collapse;
  different scope → different key; per-plugin isolation.
- **Integration — IPC schema:** `SearchRequest`, `MpdSearchRequest`,
  `ScopeResultsMsg`, `MpdSearchResult` roundtrip through the wire codec.
- **Integration — one real plugin end-to-end:** local test fixture plugin;
  issue `search_scoped(query, scopes)`; assert typed results with correct
  `kind` and `source`.

### 7.2 Go / TUI

- **Unit — `Searchable` contract:** each implementing screen: `SearchScope`
  correct; `StartSearch("")` no-op; `OnScopeResults` with stale id dropped;
  `RestoreView` after `Snapshot` roundtrip is idempotent.
- **Unit — `DataSource` impls:** `MpdDataSource` mocks IPC client, verifies
  search → filter → restore; `PluginDataSource` mocks IPC stream, verifies
  streamed `ScopeResultsMsg` incrementally updates columns, loading
  indicator toggles on `partial`.
- **Component — `CatalogBrowser`:** focus transitions across columns;
  `Source`/`Sources` column toggled by `HasMultipleSources`; `SourcePicker`
  opens on Enter when row has >1 source; lazy "Sources" count for video
  triggers on cursor-focus hover >300ms with chevron fallback.
- **Integration — `/` dispatch routing:** on `Searchable` screen input
  focuses + placeholder matches; on non-`Searchable` screen no-op + bar
  hidden.
- **Snapshot — search-then-escape returns identical view state** for Music
  Library, Music Browse, Movies, Series, Library.

### 7.3 Manual smoke (pre-merge)

- Music Library: type a known artist; all three columns filter correctly;
  Esc restores; cursor preserved.
- Music Browse against real plugins (Discogs, Last.fm): streamed per-column
  population; loading indicators; `SourcePicker` on duplicate entries.
- Movies / TMDB: typed Movie results; lazy `Sources` count after cursor
  focus; source picker on activation (existing flow).
- Disconnected MPD + Music Library search → inline "MPD disconnected" state.
- Forced slow plugin (test fixture) → fast-plugin column populates
  independently.

### 7.4 Out of scope for testing

- The plugin refactor itself — out of scope for this spec. We do test that
  dispatch + declared-kinds contract works with mocked plugins honoring it.
- Performance / load benchmarks — tracked separately with the plugin-
  performance-rating work.
- Cross-tab search — deleted; nothing to test.

### 7.5 Infrastructure

- Existing Rust test harness in `runtime/tests/`. Add fixtures for scoped
  search responses.
- Existing Bubbletea `teatest`-style harness reused for TUI component tests.
- No new testing framework introduced.
