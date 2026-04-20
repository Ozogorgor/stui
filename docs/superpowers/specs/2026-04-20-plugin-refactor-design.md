# STUI Metadata-Plugin Refactor — Design

Date: 2026-04-20
Status: Draft (brainstormed, pending review)
Predecessor: `docs/superpowers/specs/2026-04-19-search-refactor-design.md`
Backlog context: `docs/superpowers/BACKLOG.md`

## 1. Summary & Goals

Replace the ad-hoc metadata-plugin landscape with one canonical spec: a
typed trait hierarchy, a typed manifest schema, a 4-state plugin status
model, audited-and-migrated keepers for the current metadata plugins
(dropping four, rewriting one from scratch, porting the rest), and a
first-class plugin developer experience via a `stui plugin` CLI.

### Driving motivations

1. **Plugins drifted across stui's development lifecycle** — they follow
   different manifest conventions, some have placeholder / non-functional
   features, and collectively they fail to provide a consistent surface
   the search refactor (merged in `b599334`) can rely on.
2. **The search refactor locked in the manifest-shape direction** — typed
   `[capabilities.catalog]` with `kinds = [...]`, scope-aware dispatch,
   declared-kinds enforcement. This refactor builds on that foundation.
3. **Tag-normalization needs per-entity lookup (MusicBrainz style)** that
   the current ABI cannot express. Memory note
   `project_stui_roadmap.md` (2026-04-15) explicitly queues this.
4. **Plugins-not-recognized symptom** was the "it's broken, can't test
   end-to-end" gate on the search refactor's manual smoke. Fixing the
   metadata-plugin layer unblocks both.

### Design principle

**Vanilla stui ships only plugins that meet a "solid" bar** — stable APIs,
first-party documentation, predictable rate limits, clear ToS.
Scrapers, experimental backends, media-source plugins, and anything
fragile live in the external plugin repo
(`https://github.com/Ozogorgor/stui_plugins`) — user-installable but
not bundled. This principle justifies every drop decision in §5.

### In scope

1. **ABI + trait redesign** — `Plugin` (root: manifest + lifecycle) +
   `CatalogPlugin` (full metadata verb suite).
2. **Manifest schema rewrite** — typed `[capabilities.catalog]` with
   per-verb declaration; closed canonical id-source registry; network
   allowlist + rate-limit declaration; filesystem permissions dropped
   from metadata manifests.
3. **Plugin state model** — `Loaded | NeedsConfig | Failed | Disabled`;
   declarative `required = true` config handled by runtime before init;
   TUI surfaces status with actionable prompts.
4. **Per-entity lookup via `external_ids`** —
   `PluginEntry.external_ids: HashMap<String, String>` for
   cross-namespace id carriage; single-id `lookup(id, kind, id_source)`
   + runtime dispatcher picks id-source-compatible plugins.
5. **Metadata plugin audit + migration** — each of the current metadata
   plugins gets a keep/rewrite/drop decision + migration path. New
   `musicbrainz-provider` plugin written from scratch.
6. **Non-metadata cleanup** — `subscene`, `kitsunekko`, `yify-subs`,
   `torrentio-rpc` moved out of bundled repo (leftovers from a prior
   move-to-own-repo that wasn't completed — they already live in
   `stui_plugins`).
7. **Developer Experience CLI** —
   `stui plugin {init,build,test,lint,install --dev}` subcommand tree;
   canonical template; mocked-host test harness; static lint rules.
8. **Runtime improvements** — token-bucket rate limiting at
   `stui_http_get`; clear actionable error messages on load failure;
   hot-reload dev loop; dispatch routing keyed on declared capability
   + id-source + kind.

### Out of scope (non-goals, tracked in BACKLOG.md)

- Plugin signing, registry, `stui plugin {sign,publish,search}` —
  Tier 3 inspirational.
- Media-source plugin refactor (Stream/Subtitle/Torrent providers) —
  follow-up project, informed by this refactor.
- Cache TTL manifest declaration — deferred to the caching overhaul
  project (next-up on BACKLOG Tier 1 after this refactor).
- Non-WASM plugin SDKs (Python/Go/Deno) — Tier 3.
- ListenBrainz as *metadata* plugin — dropped; future scrobbling plugin
  is a separate capability.
- Theming engine — unrelated.
- Per-host rate limits — global-only; fine-grained deferred.

### What this refactor does NOT change

- WASM host imports (`stui_log`, `stui_http_get`, `stui_cache_*`,
  `stui_auth_*`, `stui_exec`) — stable; may extend, will not break.
- Discovery + hot-reload watcher in `runtime/src/plugin/discovery.rs`.
- ABI-version guard — same pattern; refactor remains on
  `abi_version = 1`.

### End state — 7 bundled metadata plugins

| Directory (on disk) | Manifest `name` (plugin identity) | Status |
|---|---|---|
| `plugins/tmdb-provider/` | `tmdb` | Keep, migrate |
| `plugins/omdb-provider/` | `omdb` | Keep, migrate |
| `plugins/anilist-provider/` | `anilist` | Keep, migrate |
| `plugins/kitsu/` | `kitsu` | Keep, migrate |
| `plugins/discogs-provider/` | `discogs` | Keep, migrate |
| `plugins/lastfm-provider/` | `lastfm` | Keep, migrate |
| `plugins/musicbrainz-provider/` | `musicbrainz` | **New** — write from scratch |

**Naming convention:** directory names carry a `-provider` suffix where
relevant (historical convention); manifest `name` is the short form
(`tmdb`, `lastfm`, etc.). The runtime identifies plugins by manifest
`name` — this is the key used in the state map, dispatch map, cache
keys, and `PluginEntry.source`. Directory basename is not
authoritative. `stui plugin init` generates both consistently.

Dropped from bundled: `imdb-provider`, `javdb`, `r18`,
`listenbrainz-provider`, `subscene`, `kitsunekko`, `yify-subs`,
`torrentio-rpc`.

## 2. Architecture

### Three-layer boundary

```
┌─────────────────────────────────────────────────────────────┐
│  1. SDK (sdk/)                                              │
│     - Plugin trait (root: manifest, lifecycle)              │
│     - CatalogPlugin trait (6 verbs; metadata capability)    │
│     - Request/response types (ids, entries, errors)         │
│     - id_sources module (canonical constants)               │
│     - error_codes module                                    │
│     - stui_export_plugin!() proc macro                      │
│     - Mocked host helpers for test harness                  │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼ compiles to cdylib wasm32-wasip1
┌─────────────────────────────────────────────────────────────┐
│  2. Plugins (plugins/*/)                                    │
│     Each plugin:                                            │
│     - plugin.toml (manifest: identity, capabilities, perms) │
│     - src/lib.rs (impl Plugin + impl CatalogPlugin)         │
│     - tests/*.rs + tests/fixtures/*.json                    │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼ discovered at ~/.stui/plugins/*/plugin.toml
┌─────────────────────────────────────────────────────────────┐
│  3. Runtime                                                 │
│     runtime/src/discovery.rs — walk+watch ~/.stui/plugins/  │
│     runtime/src/plugin/ (new module dir from today's plugin.rs)│
│        loader.rs     parse manifest, load wasm, validate    │
│        state.rs      PluginStatus + loaded-plugin map       │
│                      (renamed from "registry.rs" — existing │
│                       runtime/src/registry/ is a different  │
│                       concern: remote plugin-repo client)   │
│        dispatcher.rs route verb calls by capability+kind+id_src│
│        supervisor.rs per-plugin permit, rate-limit, reload  │
└─────────────────────────────────────────────────────────────┘
```

**Name disambiguation note:** `runtime/src/registry/` already exists and is the
**remote plugin-repo client** (fetches `plugins.json` from URLs like
`https://plugins.stui.dev` to list installable plugins — related to the Tier-3
inspirational registry work, not this refactor). This refactor does not touch
that module. The in-process plugin-state tracker introduced here is named
`plugin/state.rs` to avoid collision.

### Trait hierarchy

```rust
// sdk/src/lib.rs

pub trait Plugin {
    fn manifest(&self) -> &PluginManifest;
    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> { Ok(()) }
    fn shutdown(&mut self) -> Result<(), PluginError> { Ok(()) }
}

pub trait CatalogPlugin: Plugin {
    // Required
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse>;

    // Optional — default returns Unsupported
    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse>
        { err_not_implemented() }
    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse>
        { err_not_implemented() }
    fn get_artwork(&self, req: ArtworkRequest) -> PluginResult<ArtworkResponse>
        { err_not_implemented() }
    fn get_credits(&self, req: CreditsRequest) -> PluginResult<CreditsResponse>
        { err_not_implemented() }
    fn related(&self, req: RelatedRequest) -> PluginResult<RelatedResponse>
        { err_not_implemented() }
}
```

### Declared-capability ⇔ implemented contract

A plugin declares `verb = true` in `[capabilities.catalog]` *iff* it
actually implements the corresponding method non-default.
`stui plugin lint` enforces this — declared-but-unimpl plugins fail
lint; undeclared-but-impl plugins fail lint.

Default methods return `err_not_implemented()` — a helper that wraps
`PluginResult::err(error_codes::NOT_IMPLEMENTED, "...")`. The
`NOT_IMPLEMENTED` code is semantically distinct from `UNSUPPORTED_SCOPE`:
the former means "this plugin doesn't offer this verb at all"; the
latter means "this plugin offers this verb but not for this scope"
(e.g., a plugin declares `lookup` for `artist` kind only; a lookup
request for `track` kind returns `UNSUPPORTED_SCOPE`).

**Exception for dev iteration:** `verb = { stub = true, reason = "..." }`
passes lint with a warning; dispatch returns `PluginError { code:
"not_implemented", message: "<reason>" }`. `stui plugin build --release`
fails if any stubs exist.

### Runtime layers

- **Discovery** (`runtime/src/discovery.rs`, unchanged from today)
  — walks `~/.stui/plugins/*/plugin.toml`, watches via `notify`,
  debounces 500ms, emits `PluginDiscovered` / `PluginRemoved`. Stays
  at top-level (not moved into the new `runtime/src/plugin/` module dir;
  discovery straddles runtime startup + plugin concerns).
- **Loader** (new, `runtime/src/plugin/loader.rs`) — consumes discovery
  events:
  1. Parse manifest; reject with
     `Failed { reason: "manifest: <specific error>" }` on parse error.
  2. Validate manifest schema (id-source membership, declared-kinds in
     canonical set, required fields present).
  3. Load WASM via wasmtime; run ABI version check; wire host imports.
  4. Resolve `required = true` config from env + user settings.
     Unresolved → `NeedsConfig { missing, hint }` state.
  5. If fully configured → call `init()`. `Ok(())` → `Loaded`.
     `Err(MissingConfig {...})` → `NeedsConfig`. `Err(Fatal(reason))`
     → `Failed`.
  6. Insert into registry. Rebuild dispatch map.
- **State** (`runtime/src/plugin/state.rs`) — keyed by
  `manifest.plugin.name` (authoritative identity, may differ from
  directory basename). Tracks per-plugin state + last-updated
  timestamp. Exposes `get`, `list`, `status`, `set_status`, `reload`,
  `resolve_config`. **When loader resolves config:** precedence is
  user TUI settings > `env_var` (from manifest) > `env` defaults
  (from manifest) > `[[config]] default`.
- **Dispatcher** (`runtime/src/plugin/dispatcher.rs`) — multi-key routing:
  search by `(scope → plugin_ids)`; lookup by `(id_source × kind →
  plugin_ids)`; enrich by `(partial_kind → plugin_ids)`; etc.
  **Tie-breaking for multi-plugin matches:** single-result verbs
  (`lookup`, `get_artwork`, `get_credits`, `related`) fan out to all
  candidates in parallel with a bounded-concurrency cap, return the
  first successful response; all-fail returns the last error. This
  mirrors the per-scope partial-deadline + hard-floor pattern the
  search refactor landed (`runtime/src/engine/search_scoped.rs`).
  Multi-result verbs (`search`, `enrich` when batched) fan out and
  merge results — already handled by the search refactor's existing
  `search_scoped` pipeline.
- **Supervisor** (`runtime/src/plugin/supervisor.rs`) — per-plugin WASM
  wrapper. Owns token-bucket rate limiter. Owns crash detection +
  auto-reload. Holds permit acquisition for the plugin-call semaphore
  established in the search refactor.

### Architectural invariants

1. **Capability declared in manifest ⇔ trait method implemented.**
   Enforced by lint.
2. **Manifest shape is the single source of truth for "what can this
   plugin do."** Runtime doesn't probe by calling verbs speculatively.
3. **Plugin status is always precisely one of 4 states.** Transitions
   are explicit (loader → Loaded / NeedsConfig / Failed; user →
   Disabled; unload → remove).
4. **Config resolution is declarative-first.** The loader checks
   manifest `required = true` fields before touching code. Plugin
   `init()` only runs if all required config is resolved.
5. **Rate limits are enforced at the host-import layer.** Plugin author
   never calls `sleep()` for throttling; host does it.

## 3. Plugin Contract (ABI)

### Root types

```rust
pub struct InitContext<'a> {
    pub env: &'a HashMap<String, String>,
    pub config: &'a HashMap<String, toml::Value>,
    pub cache_dir: &'a Path,
    pub logger: &'a dyn PluginLogger,
}

pub enum PluginInitError {
    MissingConfig { fields: Vec<String>, hint: Option<String> },
    Fatal(String),
}

pub struct PluginError {
    pub code: String,       // stable machine-readable
    pub message: String,    // human text
    pub retry_after_ms: Option<u64>,
}

pub enum PluginResult<T> {
    Ok(T),
    Err(PluginError),
}
```

### Entry type

```rust
pub struct PluginEntry {
    pub id: String,                                  // primary id in source namespace
    pub source: String,                              // plugin name (e.g. "tmdb")
    pub external_ids: HashMap<String, String>,       // cross-namespace
    pub kind: EntryKind,
    pub title: String,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub rating: Option<f32>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub duration: Option<u32>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub track_number: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}
```

### Verb signatures

#### `search(SearchRequest) -> SearchResponse` — required

Unchanged from search refactor.

```rust
pub struct SearchRequest {
    pub query: String,
    pub scope: SearchScope,
    pub page: u32,
    pub limit: u32,
    pub per_scope_limit: Option<u32>,
    pub locale: Option<String>,
}
pub struct SearchResponse { pub items: Vec<PluginEntry>, pub total: u32 }
```

#### `lookup(LookupRequest) -> LookupResponse` — optional

```rust
pub struct LookupRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub locale: Option<String>,
}
pub struct LookupResponse { pub entry: PluginEntry }
```

Single-id only. Dispatcher picks id-source-compatible plugin.

#### `enrich(EnrichRequest) -> EnrichResponse` — optional

```rust
pub struct EnrichRequest {
    pub partial: PluginEntry,
    pub prefer_id_source: Option<String>,
}
pub struct EnrichResponse {
    pub entry: PluginEntry,
    pub confidence: f32,  // 0.0 - 1.0
}
```

Given a partial entity (title + artist_name + album_name etc., no id
required), plugin performs its own match heuristics and returns the
best-effort full entity.

#### `get_artwork(ArtworkRequest) -> ArtworkResponse` — optional

```rust
pub struct ArtworkRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub size: ArtworkSize,
}
pub enum ArtworkSize { Thumbnail, Standard, HiRes, Any }
pub struct ArtworkResponse {
    pub variants: Vec<ArtworkVariant>,
}
pub struct ArtworkVariant {
    pub size: ArtworkSize,
    pub url: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
```

**URL not bytes.** Plugin returns URLs; host fetches + caches binary at
`~/.stui/cache/artwork/<hash>`.

#### `get_credits(CreditsRequest) -> CreditsResponse` — optional

```rust
pub struct CreditsRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
}
pub struct CreditsResponse {
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
}
pub struct CastMember {
    pub name: String,
    pub role: CastRole,
    pub character: Option<String>,
    pub instrument: Option<String>,
    pub billing_order: Option<u32>,
    pub external_ids: HashMap<String, String>,
}
pub enum CastRole {
    Actor, Vocalist, FeaturedArtist, GuestAppearance, Other(String),
}
pub struct CrewMember {
    pub name: String,
    pub role: CrewRole,
    pub department: Option<String>,
    pub external_ids: HashMap<String, String>,
}
pub enum CrewRole {
    Director, Writer, Producer, ExecutiveProducer,
    Cinematographer, Editor, Composer,
    Songwriter, Lyricist, Arranger, Instrumentalist,
    ProductionDesigner, ArtDirector, CostumeDesigner,
    SoundDesigner, VfxSupervisor,
    Other(String),
}
```

Enum roles make UI filters like "all works by this DoP" machine-safe
without string-matching "Director of Photography" vs "Cinematographer".
`Other(String)` is the fallback for upstream roles we haven't named.
SDK ships a `normalize_crew_role("Director of Photography") →
CrewRole::Cinematographer` helper.

#### `related(RelatedRequest) -> RelatedResponse` — optional

```rust
pub struct RelatedRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub relation: RelationKind,
    pub limit: u32,
}
pub enum RelationKind {
    SameArtist, SameDirector, SameStudio, Similar, Sequel, Compilation, Any,
}
pub struct RelatedResponse { pub items: Vec<PluginEntry> }
```

### Canonical error codes

```rust
// sdk/src/error_codes.rs
pub const NOT_IMPLEMENTED: &str    = "not_implemented";
pub const UNSUPPORTED_SCOPE: &str  = "unsupported_scope";
pub const UNKNOWN_ID: &str         = "unknown_id";
pub const RATE_LIMITED: &str       = "rate_limited";
pub const TRANSIENT: &str          = "transient";
pub const INVALID_REQUEST: &str    = "invalid_request";
pub const REMOTE_ERROR: &str       = "remote_error";
```

### Canonical id-sources (closed set)

```rust
// sdk/src/id_sources.rs
pub const TMDB: &str        = "tmdb";
pub const IMDB: &str        = "imdb";
pub const TVDB: &str        = "tvdb";
pub const MUSICBRAINZ: &str = "musicbrainz";
pub const DISCOGS: &str     = "discogs";
pub const ANILIST: &str     = "anilist";
pub const KITSU: &str       = "kitsu";
pub const MYANIMELIST: &str = "myanimelist";
```

Loader rejects unknown id-sources at manifest load. Extending requires
an SDK version bump.

**Note on `IMDB` id-source vs IMDb as a scraping target:** `imdb` in
this registry is a namespace label for IMDb IDs (the `tt1234567`
format). Carrying an IMDb ID in `PluginEntry.external_ids` does NOT
imply hitting imdb.com — databases like TMDB cross-reference IMDb IDs
and expose them via their own APIs (`/external_ids` in TMDB). The
dropped `imdb-provider` plugin was a scraper that made HTTP requests
to imdb.com itself; that's different from using IMDb as an id
namespace label. `tmdb-provider`'s `lookup.id_sources = ["tmdb", "imdb"]`
means it accepts IMDb IDs as input and resolves them through TMDB's
external-id endpoint — no IMDb HTTP traffic involved.

### ABI wire format

JSON serialized through `stui_alloc` / `stui_free`, packed `i64`
(`ptr << 32 | len`) return values. `stui_export_plugin!(MyType)` macro
generates the glue for every new verb — plugin authors write
`impl CatalogPlugin for MyType { ... }` and get FFI for free.

## 4. Manifest Schema

### Complete example (TMDB, reference implementation)

```toml
[plugin]
name         = "tmdb"
version      = "1.0.0"
abi_version  = 1
entrypoint   = "plugin.wasm"
description  = "The Movie Database — movies, TV series, episodes"

[meta]
author       = "stui"
license      = "MIT"
homepage     = "https://github.com/Ozogorgor/stui_plugins"
repository   = "https://github.com/Ozogorgor/stui_plugins"
tags         = ["movies", "tv", "metadata"]

[env]
TMDB_API_KEY = ""

[[config]]
key       = "api_key"
label     = "TMDB API Key"
hint      = "Get a free key at themoviedb.org/settings/api"
masked    = true
required  = true
env_var   = "TMDB_API_KEY"

[permissions]
network = ["api.themoviedb.org", "image.tmdb.org"]

[permissions.rate_limit]
requests_per_second = 4
burst               = 10

[capabilities.catalog]
kinds = ["movie", "series", "episode"]

search  = true
lookup  = { id_sources = ["tmdb", "imdb"] }
enrich  = true
artwork = { sizes = ["thumbnail", "standard", "hires"] }
credits = true
related = true
```

### Minimum viable (MusicBrainz — no API key)

```toml
[plugin]
name        = "musicbrainz"
version     = "1.0.0"
abi_version = 1
description = "MusicBrainz — open music encyclopedia"

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://musicbrainz.org"

[permissions]
network = ["musicbrainz.org", "coverartarchive.org"]

[permissions.rate_limit]
requests_per_second = 1
burst               = 2

[capabilities.catalog]
kinds = ["artist", "album", "track"]

search  = true
lookup  = { id_sources = ["musicbrainz"] }
enrich  = true
artwork = { sizes = ["thumbnail", "standard", "hires"] }
credits = true
related = { stub = true, reason = "related-releases endpoint pending" }
```

### Schema rules (enforced at load)

- **`[plugin]`** — `name`, `version`, `abi_version` required.
  `abi_version` mismatch → `Failed` with clear error. `name` is
  authoritative (may differ from directory basename).
- **`[meta]`** — all fields optional.
- **`[env]`** — optional env var defaults; user-override via shell or
  TUI settings.
- **`[[config]]`** — array-of-tables. Required: `key`, `label`. Optional:
  `hint`, `masked`, `required`, `default`, `env_var`.
  `env_var` lets a field resolve from env if not set in TUI settings.
  `required = true` + unresolved → `NeedsConfig`.
- **`[permissions]`** — `network`: exact hostnames only (no globs,
  no paths). Filesystem paths are **rejected** for metadata plugins.
- **`[permissions.rate_limit]`** — optional. `requests_per_second`
  (required if block present). `burst` (optional; defaults to RPS).
  Global scope — per-host deferred.
- **`[capabilities.catalog]`** — `kinds`: strict subset of
  `["artist","album","track","movie","series","episode"]`. Per-verb
  declaration: `bool` or sub-table. `search = true` required;
  others default false.

### Unknown-field policy

- Top-level unknown keys → `Failed` (strict).
- Unknown keys inside `[capabilities.catalog]` → warning (future verbs
  tolerated; `stui plugin lint` flags).
- Unknown keys inside `[[config]]` → warning.
- `[permissions.X]` where X ≠ `rate_limit` → strict fail.

### Backward-compat policy

No silent tolerance of legacy forms. Plugins that worked during the
search refactor's transitional period now fail loudly on legacy
manifests with a specific "legacy manifest: please migrate" message.

**Legacy fields explicitly removed** — loader rejects manifests
containing any of these:

- `[plugin] type = "..."` (e.g., `type = "metadata"`, `type = "stream-provider"`).
  Plugin type is inferred from declared capabilities now (catalog,
  streams, subtitles, etc.).
- `[capabilities] catalog = true` (bool form) — must migrate to
  `[capabilities.catalog] kinds = [...]` typed form.
- `[capabilities] metadata = true`, `music = true`, `anime = true`,
  `search = true`, `resolve = true` — free-floating booleans that landed
  in `_extra` in the search refactor's transitional schema. Not
  meaningful; removed.
- `[permissions] network = true` (bool) — must migrate to
  `network = ["hostname1", "hostname2", ...]` explicit allowlist.
  Empty array = no network.
- `[permissions] filesystem = [...]` — metadata plugins do not need
  filesystem access; loader rejects this field for metadata plugins.
  (Other plugin types' filesystem needs are a media-source-refactor
  concern.)

## 5. Per-Plugin Audit

### Keepers — migrate to new spec

**Common migration actions** (apply to every keeper; per-plugin sections
below add the plugin-specific items):

- Normalize manifest `[plugin] name` to short form per §1 end-state
  table (drop `-provider` suffix if present; e.g.,
  `name = "tmdb-provider"` → `name = "tmdb"`). Already short for
  `lastfm`, `discogs`, `kitsu` — verify each during migration.
- Remove legacy manifest fields: `[plugin] type = "..."`,
  free-floating capability bools (`metadata = true`, `music = true`,
  `anime = true`, `search = true`, `resolve = true`),
  `[permissions] network = true` bool, `[permissions] filesystem = [...]`
  if present.
- Migrate `[permissions] network = true` → explicit hostname allowlist
  (use the hosts the plugin actually calls, read from the plugin's
  HTTP call sites).
- Add `[permissions.rate_limit]` per plugin-specific RPS value below.
- Add `[meta]` block (author, license, homepage, tags) if missing.
- Update Cargo.toml dependencies to the new SDK trait names
  (`CatalogPlugin` trait, new request/response types).


#### tmdb-provider

- **Purpose:** TMDB API v3 for movies, TV series, episodes.
- **API:** `https://api.themoviedb.org/3` (free tier ~40 req/10s).
- **Current:** `search()` only; returns posters, vote_average, title,
  year, overview. Episode scope collapsed to Series.
- **Issues:** `imdb_id` always `None` despite API supporting external-id
  lookup; no episode-to-season-episode translation; no rate limit
  declaration; genre lookup hardcoded as static map.
- **Migration actions:**
  - Rename manifest `[plugin] name = "tmdb-provider"` → `"tmdb"`
    (short form, matches end-state table in §1).
  - Remove legacy fields: `type = "metadata"`, `tags` (migrate to
    `[meta] tags`), `network = true` bool → hostname list.
  - Add `[permissions.rate_limit] requests_per_second = 4`.
  - Implement `lookup` with id_sources `["tmdb", "imdb"]`; use
    `/external_ids` endpoint to populate `external_ids` map
    (cross-namespace).
  - Implement `enrich` — partial entity → match by
    title + year + kind.
  - Implement `get_artwork` — hi-res poster + backdrop + multi-size.
  - Implement `get_credits` — `/movie/{id}/credits` + `/tv/{id}/credits`.
  - Implement `related` — `/movie/{id}/recommendations` +
    `/tv/{id}/recommendations`.
  - Properly resolve Episode scope via
    `/tv/{id}/season/{s}/episode/{e}`.
  - Remove hardcoded genre map; fetch from API and cache in plugin.

#### omdb-provider

- **Purpose:** OMDb REST API for movie/TV search + single-title lookup.
- **API:** `https://www.omdbapi.com/` (free tier 1000/day = 0.01/s).
- **Current:** `search()` only; returns title, year (parses "2020–2023"
  ranges), poster, IMDb ID. Missing genre + rating.
- **Migration actions:**
  - Add `[permissions.rate_limit] requests_per_second = 1` (generous
    for burst; over a day, daily cap applies independently).
  - Implement `lookup` with id_sources `["imdb"]` via `?t=&y=` endpoint
    for richer detail (plot, genre, ratings).
  - `related = false` (OMDb has no related endpoint).

#### anilist-provider

- **Purpose:** AniList GraphQL for anime metadata (movie + series).
- **API:** `https://graphql.anilist.co` (public, rate-limited).
- **Current:** `search()` via GraphQL; trending + query paths.
- **Issues:** trending query missing pagination variables; anime-type
  conflated (`Movie` and `Series` scopes both return mixed).
- **Migration actions:**
  - Fix pagination in `TRENDING_QUERY`.
  - Filter by upstream `type`: Movie scope → `type: MOVIE`; Series
    scope → `type: TV, ONA`.
  - Add `[permissions.rate_limit] requests_per_second = 0.5`.
  - Implement `lookup` with id_sources `["anilist", "myanimelist"]`
    (AniList exposes MAL IDs as externalLinks).

#### kitsu

- **Purpose:** Kitsu REST for anime metadata.
- **API:** `https://kitsu.io/api/edge` (free; optional bearer token).
- **Current:** Custom `http_get_with_bearer` inline helper (WASM-only;
  non-WASM panics); show_type parsed but ignored.
- **Migration actions:**
  - Factor bearer-token logic into SDK-level helper or justify custom
    impl with a comment.
  - Respect upstream show_type: filter Movie vs Series.
  - Add `[permissions.rate_limit] requests_per_second = 1`.
  - Implement `lookup` with id_sources `["kitsu", "myanimelist"]`.

#### discogs-provider

- **Purpose:** Discogs API for music/vinyl metadata.
- **API:** `https://api.discogs.com` (free tier 60 authenticated,
  25 unauthenticated req/min).
- **Current:** Track scope searches releases (semantically wrong —
  Discogs has no track API). Album/Release distinction collapsed.
  Genre concat combines genre/style/format/country/label ugly.
- **Migration actions:**
  - **Drop Track scope** — Discogs has no track API. `kinds =
    ["artist", "album"]`.
  - Separate genre vs format/label — use `description` field for
    non-genre metadata.
  - Add `[permissions.rate_limit] requests_per_second = 1`.
  - Implement `lookup` with id_sources `["discogs"]` (potentially
    `["discogs", "musicbrainz"]` if Discogs release pages carry MBID
    links — cheap follow-up).

#### lastfm-provider

- **Purpose:** Last.fm (via Libre.fm) for music discovery.
- **API:** `https://libre.fm/2.0` (free tier ~5 req/min).
- **Current:** Config bloat (api_secret, username, token unused);
  listeners count as genre field (wrong); album only populated in one
  of two response types.
- **Migration actions:**
  - Simplify `[[config]]` — drop unused fields; keep `api_key` only.
  - Reorganize fields: listeners → description or drop; album →
    `album_name`.
  - Add `[permissions.rate_limit] requests_per_second = 1`.
  - Implement `enrich` — partial → Last.fm's
    `track.getInfo` / `album.getInfo`.
  - `lookup = false` — Last.fm has no stable IDs.

### New plugin — write from scratch

#### musicbrainz-provider

- **Purpose:** MusicBrainz metadata for music — artist, release, recording
  lookup + search. Replaces ListenBrainz's metadata role and unblocks
  MusicBrainz-integrated tag normalization (memory
  `project_stui_roadmap.md`).
- **API:** `https://musicbrainz.org/ws/2/` (JSON format); artwork via
  `https://coverartarchive.org/`.
- **Capabilities:** `kinds = ["artist", "album", "track"]`; search,
  lookup (id_sources `["musicbrainz"]`), enrich, get_artwork (Cover
  Art Archive), get_credits (recording → artist/performer
  relationships), related (release-groups → releases).
- **Rate limit:** 1 req/s (MB public limit).
- **User-agent:** MB requires a descriptive User-Agent with contact info;
  plugin hard-codes:
  `"stui-musicbrainz-provider/<version> (https://github.com/Ozogorgor/stui)"`
  — `<version>` comes from `env!("CARGO_PKG_VERSION")` at compile time.
  Missing / generic UAs get rate-limited or banned by MB.
- **Confidence scoring for enrich:** MB's string similarity for fuzzy
  matching (title + artist + album overlap).

### Dropped

| Plugin | Drop rationale |
|---|---|
| `imdb-provider` | Scraper; IMDb ToS prohibits; fragile selector chains. TMDB + OMDb cover the use case via official APIs. |
| `javdb` | Adult-video scraper; niche; ToS risk. |
| `r18` | Adult-video scraper; niche; ToS risk. |
| `listenbrainz-provider` | Scrobbling ≠ metadata. New `musicbrainz-provider` covers the metadata role directly. ListenBrainz stays as future scrobbling-plugin concern (BACKLOG Tier 3). |
| `subscene`, `kitsunekko`, `yify-subs`, `torrentio-rpc` | Non-metadata (subtitles/streams). Already exist in `stui_plugins/` repo; bundled versions are leftovers from a prior move. |

## 6. Bundled Repo Cleanup

### Delete from `plugins/`

- `imdb-provider/` — scraper, dropped.
- `javdb/` — scraper, adult, dropped.
- `r18/` — scraper, adult, dropped.
- `listenbrainz-provider/` — scrobbling, not metadata.
- `subscene/` — leftover, already in `stui_plugins`.
- `kitsunekko/` — leftover, already in `stui_plugins`.
- `yify-subs/` — leftover, already in `stui_plugins`.
- `torrentio-rpc/` — leftover, already in `stui_plugins`.

### Workspace member update (`plugins/Cargo.toml`)

```toml
[workspace]
resolver = "2"
members = [
    "tmdb-provider",
    "omdb-provider",
    "anilist-provider",
    "kitsu",
    "discogs-provider",
    "lastfm-provider",
    "musicbrainz-provider",
]
```

### Optional follow-up (non-blocking)

Scrapers (`imdb-provider`, `javdb`, `r18`) can be lifted into
`stui_plugins/` verbatim if someone wants to maintain them there.
Deleting from bundled doesn't destroy the code; git history preserves.

### Orphaned user config

Dropped plugins may leave orphaned `~/.stui/config/plugins/<name>.toml`
entries. Silent-ignore for now (invisible in TUI). Manual cleanup
documented in migration release notes.

## 7. Migration Sequencing

### Chunk 1 — SDK + runtime foundation

- Add `Plugin` + `CatalogPlugin` traits alongside existing `StuiPlugin`.
  **Do not delete `StuiPlugin` yet** — non-metadata plugins in
  `stui_plugins/` (subscene, torrentio-rpc, soundcloud, etc.) still
  use it and will until the media-source plugin refactor. Mark
  `StuiPlugin` as deprecated with a doc comment pointing to the new
  trait hierarchy; the media-source refactor will finish the migration.
- Add verb request/response types + `InitContext` + `PluginInitError`.
- Add `sdk::id_sources` + extend `sdk::error_codes` (with `NOT_IMPLEMENTED`).
- Strict `PluginManifest` schema parsing in `runtime::plugin::loader`.
- Convert `runtime/src/plugin.rs` (single file) into `runtime/src/plugin/`
  module directory with `loader.rs`, `state.rs`, `dispatcher.rs`,
  `supervisor.rs` (rate-limit addition).
- 4-state `PluginStatus` end-to-end.

**Risk acknowledgment:** bundled `plugins/*` do not build after this
chunk (they're still using the old trait until Chunks 3-5 migrate
them). `stui_plugins/` plugins continue to work against `StuiPlugin`.
Mitigation: Chunk 3 (TMDB reference) lands quickly after Chunk 1 to
prove the new shape; a rollback means reverting Chunks 1-3 together.
If the risk feels too high mid-chunk, Chunk 1 and Chunk 3 can be
collapsed into a single atomic landing ("land new traits + migrate
TMDB as proof in one PR").

**Cross-repo coordination heads-up:** once the strict loader ships
(this chunk), manifests in `stui_plugins/` that still carry legacy
fields (`type = "..."`, `network = true`, `metadata = true`,
`music = true`, etc.) will also start failing to load with
specific legacy-field errors. The `stui_plugins/` repo may need a
sweep — either migrate those manifests in lockstep, or keep the
strict loader behind a feature flag until the media-source refactor
tackles them. Recommended: migrate `stui_plugins/` manifests to the
new schema as a parallel PR against that repo, so both repos
converge when this refactor lands.

### Chunk 2 — CLI tooling

- `stui plugin` subcommand: `init`, `build`, `test`, `lint`,
  `install --dev`.
- Canonical template (basis: `example-provider` from `stui_plugins`).
- Mocked-host test harness (fixture-based HTTP, cache).
- Lint rules codified.

### Chunk 3 — Reference plugin: TMDB

- Full verb surface; establishes the pattern.
- Integration-tested end-to-end.

### Chunk 4 — Remaining plugin migrations (parallel-safe)

- `omdb-provider`, `anilist-provider`, `kitsu`, `discogs-provider`,
  `lastfm-provider`. One commit per plugin.

### Chunk 5 — MusicBrainz new plugin

- Full verb surface. Unblocks tag normalization (memory
  `project_stui_roadmap.md`).

### Chunk 6 — Cleanup

- Delete dropped + leftover plugins.
- Update workspace members.

### Chunk 7 — Integration + verification

- Runtime smoke: all 7 plugins reach `Loaded`.
- Dispatcher smoke: scoped search routes correctly; lookup by
  id_source; enrich by partial kind.
- Rate-limit smoke: token-bucket enforced.
- Status smoke: `NeedsConfig` → `Loaded` via TUI config flow.
- `--release` gate test.
- Audit verification + docs updated.

## 8. Testing Strategy

- **Per-plugin unit tests** — `plugins/<name>/src/lib.rs` `#[cfg(test)]`
  modules. SDK mock host helpers.
- **Plugin integration tests** — `stui plugin test` with fixtures in
  `plugins/<name>/tests/fixtures/*.json`.
- **SDK tests** — `sdk/tests/` for manifest parsing, error codes,
  id-source validation.
- **Runtime integration tests** — `runtime/tests/` for loader state
  transitions, dispatcher routing, rate limiting, supervisor reload.
- **TUI teatest coverage** — plugin list shows status; NeedsConfig
  flow works end-to-end; detail view shows per-verb impl status.

### Risk mitigation

- Chunk boundaries are checkpoints.
- Each plugin migration individually revertable.
- Old SDK shape preserved in git history.
- Integration tests catch loader / dispatcher regressions.

## 9. Non-Goals Reiteration

| Item | Owner |
|---|---|
| Plugin signing, registry, `publish`, `sign` | BACKLOG Tier 3 |
| Cache TTL declarations in manifest | Caching project (next) |
| Per-host rate limits | Future; global-only in v1 |
| Media-source plugin refactor | Follow-up project |
| Non-WASM SDKs | BACKLOG Tier 3 |
| End-to-end smoke against real APIs in CI | Manual smoke only; fixtures in CI |

## 10. Open Questions

Resolved during spec review loop:

- Runtime module naming — `plugin/state.rs` (not `registry.rs`) to avoid
  collision with existing remote plugin-repo client in
  `runtime/src/registry/`. Resolved in §2.
- `StuiPlugin::resolve()` fate — deprecated but kept alive in SDK until
  the media-source plugin refactor completes migration of non-metadata
  plugins in `stui_plugins/`. Resolved in §7 Chunk 1.
- Config precedence — user TUI settings > env_var > env default >
  config default. Resolved in §2 State module description.
- Dispatcher tie-breaking for single-result verbs — parallel fan-out,
  first-success, all-fail returns last error. Mirrors search refactor's
  `search_scoped` partial-deadline pattern. Resolved in §2 Dispatcher
  description.
- MusicBrainz UA — hard-coded to
  `stui-musicbrainz-provider/<version> (https://github.com/Ozogorgor/stui)`.
  Resolved in §5.
- Plugin naming — directory basename may include `-provider` suffix;
  manifest `name` is the short form and is authoritative for runtime
  identity. Resolved in §1 end-state table.

Remaining open (to surface during implementation):

- **`stui plugin gc` subcommand** for orphaned user config after plugin
  drops — deferred to §1 Item 7 extension if the TUI surface makes the
  need obvious during Chunk 7 smoke. Low priority.
- **`stui_plugins/` `example-provider` template migration** — when
  Chunk 2's `stui plugin init` lands with a canonical template based
  on the new trait hierarchy, `stui_plugins/example-provider` needs to
  be updated in lockstep. Mechanically a separate PR against the
  plugin repo, coordinated with Chunk 5 or later.
- **Rate-limiter integration test detail** — spec names rate-limit
  testing in §8 but doesn't detail the token-bucket test shape.
  Implementation plan (writing-plans phase) will specify.
