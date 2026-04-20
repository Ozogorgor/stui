# STUI Metadata-Plugin Refactor Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace ad-hoc metadata plugins with one canonical spec — typed
`Plugin` + `CatalogPlugin` trait hierarchy, strict manifest schema, 4-state
plugin status, audited-and-migrated keepers, `musicbrainz-provider` written
from scratch, and a `stui plugin` CLI for future authors.

**Architecture:** SDK defines traits, types, error codes, id-source constants,
and proc-macros. Runtime splits today's monolithic `plugin.rs` into a
`plugin/` module directory (loader, state, dispatcher, supervisor). Each of
7 bundled plugins becomes a self-contained crate implementing
`CatalogPlugin` with manifest declaring its verb surface. A new `stui` CLI
binary scaffolds, builds, tests, lints, and dev-installs plugins.

**Tech Stack:** Rust (runtime + SDK + plugins + CLI), WASM (wasmtime
supervisor), TOML (manifests), Go (TUI — unchanged for this refactor),
proc-macros for plugin export glue.

**Spec:** `docs/superpowers/specs/2026-04-20-plugin-refactor-design.md`
**Backlog:** `docs/superpowers/BACKLOG.md`

---

## File Structure

### SDK (`sdk/`)

Extended — existing files modified, new files added.

- `sdk/src/lib.rs` — trait hierarchy: add `Plugin` + `CatalogPlugin`;
  keep `StuiPlugin` deprecated-alive for non-metadata plugins in
  `stui_plugins/`. Re-export new modules.
- `sdk/src/kinds.rs` — existing (from search refactor). No changes.
- `sdk/src/id_sources.rs` — **new**. Canonical id-source constants
  (closed set): `TMDB`, `IMDB`, `TVDB`, `MUSICBRAINZ`, `DISCOGS`,
  `ANILIST`, `KITSU`, `MYANIMELIST`.
- `sdk/src/lib.rs` — existing inline `pub mod error_codes { ... }`
  block gains new constants: `NOT_IMPLEMENTED`, `UNKNOWN_ID`,
  `RATE_LIMITED`, `TRANSIENT`, `REMOTE_ERROR` (plus existing
  `UNSUPPORTED_SCOPE`, `INVALID_REQUEST`). Module stays inline — no
  separate `error_codes.rs` file.
- `sdk/src/capabilities.rs` — **new**. Request/response types per verb:
  `LookupRequest/Response`, `EnrichRequest/Response`,
  `ArtworkRequest/Response` + `ArtworkSize` + `ArtworkVariant`,
  `CreditsRequest/Response` + `CastMember/CastRole/CrewMember/CrewRole`,
  `RelatedRequest/Response` + `RelationKind`. Plus `InitContext`,
  `PluginInitError`, `err_not_implemented()` helper,
  `normalize_crew_role()` helper. Also contains a **slim
  `validate_manifest()`** function mirroring the schema-only subset of
  the runtime's full validator — used by `stui plugin lint` /
  `stui plugin build` without pulling in the runtime crate.
- `sdk/src/host.rs` — **new**. `PluginLogger` trait. Mocked-host helpers
  for `stui plugin test` harness (mock `stui_http_get`, `stui_cache_*`).
- `sdk/src/macros.rs` — **new or extend**. `stui_export_plugin!` macro
  generates the FFI glue for all 6 `CatalogPlugin` verbs (today's macro
  covers only `search` + `resolve`).
- `sdk/Cargo.toml` — no changes expected.

### Runtime (`runtime/`)

- `runtime/src/plugin.rs` (existing, single file) → **convert to
  `runtime/src/plugin/` module directory** with:
  - `runtime/src/plugin/mod.rs` — re-exports + shared types.
  - `runtime/src/plugin/manifest.rs` — `PluginManifest`, `Capabilities`,
    `CatalogCapability`, `NetworkPermission`, `RateLimit`, strict-validate
    helpers.
  - `runtime/src/plugin/loader.rs` — parse + validate + instantiate WASM +
    resolve config + call init; returns initial `PluginStatus`.
  - `runtime/src/plugin/state.rs` — `PluginStatus` 4-state enum,
    per-plugin state map, transition helpers, `get/list/status/set_status/
    reload/resolve_config`.
  - `runtime/src/plugin/dispatcher.rs` — routing: `plugins_for_scope`,
    `plugins_for_lookup(id_source, kind)`, `plugins_for_enrich(kind)`,
    `plugins_for_artwork(kind)`, `plugins_for_credits(kind)`,
    `plugins_for_related(kind)`. Rebuild on state change.
  - `runtime/src/plugin/supervisor.rs` — per-plugin WASM wrapper with
    crash detection, auto-reload, **token-bucket rate limiter**
    (new responsibility).
- `runtime/src/discovery.rs` — **unchanged**. Top-level, emits
  `PluginDiscovered` / `PluginRemoved` events into the loader.
- `runtime/src/registry/mod.rs` — **untouched** (remote plugin-repo client,
  unrelated concern; named to avoid collision with `plugin/state.rs`).
- `runtime/src/abi/types.rs` — extend: add ABI-side `LookupRequest/Response`,
  `EnrichRequest/Response`, `ArtworkRequest/Response`, etc. (mirror SDK
  types for JSON-over-WASM).
- `runtime/src/abi/host.rs` — add host-side call wrappers
  (`WasmHost::lookup`, `::enrich`, `::get_artwork`, `::get_credits`,
  `::related`) — each calls a new `stui_<verb>` WASM export.
- `runtime/src/abi/supervisor.rs` — add token-bucket rate-limit check
  before each `stui_http_get` inside plugin calls.
- `runtime/src/engine/mod.rs` — extend `Engine` with
  `supervisor_lookup(plugin_id, req)`, `supervisor_enrich(...)`,
  `supervisor_get_artwork(...)`, `supervisor_get_credits(...)`,
  `supervisor_related(...)` — same pattern as existing
  `supervisor_search` from search refactor.
- `runtime/src/engine/dispatch_map.rs` — extend to cover multi-verb
  routing (today only covers scope→plugin_ids for search).
- `runtime/src/ipc/v1/mod.rs` — add IPC variants for the new verbs
  (so the TUI can invoke them later; not strictly required this
  refactor but unlocked here).
- `runtime/src/main.rs` — dispatcher arms for new `Request::*` variants
  (thin routing to engine helpers).

### CLI (`cli/` — new workspace member)

- `cli/Cargo.toml` — **new**. Binary `stui`. Deps: clap, anyhow,
  stui-plugin-sdk (workspace), walkdir, toml, serde.
- `cli/src/main.rs` — **new**. Entry; `clap` subcommand tree:
  `stui plugin {init,build,test,lint,install}`.
- `cli/src/cmd/mod.rs` — **new**. Subcommand dispatch.
- `cli/src/cmd/init.rs` — **new**. `stui plugin init <name>`:
  scaffolds plugin dir from embedded template.
- `cli/src/cmd/build.rs` — **new**. `stui plugin build [--release]`:
  compiles plugin to wasm32-wasip1; validates manifest; runs lint;
  `--release` fails on stubs.
- `cli/src/cmd/test.rs` — **new**. `stui plugin test`: runs plugin's
  test harness with mocked host.
- `cli/src/cmd/lint.rs` — **new**. Static checks: manifest validates
  against SDK schema; declared capabilities ⇔ implemented; canonical
  id-sources; required config fields.
- `cli/src/cmd/install.rs` — **new**. `stui plugin install --dev`:
  symlinks build output to `~/.stui/plugins/<name>/` (triggers
  hot-reload watcher).
- `cli/src/template/` — **new**. Embedded plugin template files
  (Cargo.toml, src/lib.rs, plugin.toml, tests/fixtures/example.json,
  README.md). Loaded via `include_str!`.
- `Cargo.toml` (workspace root) — add `"cli"` to workspace members.

### Bundled plugins (`plugins/`)

- **Migrated (one commit each):**
  - `plugins/tmdb-provider/src/lib.rs` + `plugin.toml` + Cargo.toml +
    `tests/` + `tests/fixtures/`.
  - `plugins/omdb-provider/` — same shape.
  - `plugins/anilist-provider/` — same shape.
  - `plugins/kitsu/` — same shape.
  - `plugins/discogs-provider/` — same shape.
  - `plugins/lastfm-provider/` — same shape.
- **New:**
  - `plugins/musicbrainz-provider/` — **new**. Cargo.toml,
    plugin.toml, src/lib.rs, tests/, tests/fixtures/.
- **Deleted (Chunk 6):**
  - `plugins/imdb-provider/`, `plugins/javdb/`, `plugins/r18/`,
    `plugins/listenbrainz-provider/`, `plugins/subscene/`,
    `plugins/kitsunekko/`, `plugins/yify-subs/`,
    `plugins/torrentio-rpc/`.
- `plugins/Cargo.toml` — update workspace members (Chunk 6).

---

## Chunk 1 — SDK + Runtime Foundation

Lays the type + trait + manifest foundation. Non-metadata plugins continue
using deprecated `StuiPlugin`; bundled plugins break until Chunk 3.

### Task 1.1: Add `sdk::id_sources` module

**Files:**
- Create: `sdk/src/id_sources.rs`
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Create `sdk/src/id_sources.rs`**

```rust
//! Canonical id-source constants for plugin manifests and lookup requests.
//!
//! Closed set; adding a new id source requires an SDK version bump. Runtime
//! rejects unknown id-sources at manifest load.

pub const TMDB: &str        = "tmdb";
pub const IMDB: &str        = "imdb";
pub const TVDB: &str        = "tvdb";
pub const MUSICBRAINZ: &str = "musicbrainz";
pub const DISCOGS: &str     = "discogs";
pub const ANILIST: &str     = "anilist";
pub const KITSU: &str       = "kitsu";
pub const MYANIMELIST: &str = "myanimelist";

/// Whether a given string is a canonical id-source.
pub fn is_canonical(source: &str) -> bool {
    matches!(
        source,
        TMDB | IMDB | TVDB | MUSICBRAINZ | DISCOGS | ANILIST | KITSU | MYANIMELIST
    )
}

/// All canonical id-sources as a slice (useful for iteration, tests).
pub const ALL: &[&str] = &[
    TMDB, IMDB, TVDB, MUSICBRAINZ, DISCOGS, ANILIST, KITSU, MYANIMELIST,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_values_stable() {
        assert_eq!(TMDB, "tmdb");
        assert_eq!(MUSICBRAINZ, "musicbrainz");
    }

    #[test]
    fn is_canonical_rejects_unknown() {
        assert!(is_canonical("tmdb"));
        assert!(!is_canonical("unknown"));
        assert!(!is_canonical(""));
    }

    #[test]
    fn all_contains_every_constant() {
        assert_eq!(ALL.len(), 8);
        for s in ALL { assert!(is_canonical(s)); }
    }
}
```

- [ ] **Step 2: Re-export in `sdk/src/lib.rs`**

Add near existing `pub mod kinds;`:

```rust
pub mod id_sources;
```

- [ ] **Step 3: Run tests**

```
cd sdk && cargo test --lib id_sources
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```
git add sdk/src/id_sources.rs sdk/src/lib.rs
git commit -m "feat(sdk): canonical id_sources module"
```

### Task 1.2: Extend `sdk::error_codes`

**Files:**
- Modify: `sdk/src/lib.rs` (error_codes module, added in search refactor Task 1.4)

- [ ] **Step 1: Locate existing `error_codes` module**

```
grep -n "error_codes\|UNSUPPORTED_SCOPE" sdk/src/lib.rs
```

Expected: find `pub mod error_codes` with `UNSUPPORTED_SCOPE` and
`INVALID_REQUEST` constants already defined.

- [ ] **Step 2: Add new constants**

In the `error_codes` module body, after existing constants:

```rust
pub const NOT_IMPLEMENTED: &str = "not_implemented";
pub const UNKNOWN_ID: &str      = "unknown_id";
pub const RATE_LIMITED: &str    = "rate_limited";
pub const TRANSIENT: &str       = "transient";
pub const REMOTE_ERROR: &str    = "remote_error";
```

- [ ] **Step 3: Write failing test**

In the existing SDK tests module:

```rust
#[test]
fn new_error_codes_are_stable() {
    use super::error_codes::*;
    assert_eq!(NOT_IMPLEMENTED, "not_implemented");
    assert_eq!(RATE_LIMITED, "rate_limited");
    assert_eq!(UNKNOWN_ID, "unknown_id");
}
```

- [ ] **Step 4: Run tests**

```
cd sdk && cargo test
```

Expected: new test passes plus existing pass.

- [ ] **Step 5: Commit**

```
git add sdk/src/lib.rs
git commit -m "feat(sdk): extend error_codes with NOT_IMPLEMENTED, RATE_LIMITED, UNKNOWN_ID, TRANSIENT, REMOTE_ERROR"
```

### Task 1.3: Add `sdk::capabilities` module — request/response types

**Files:**
- Create: `sdk/src/capabilities.rs`
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Create the file**

```rust
//! Request/response types for `CatalogPlugin` verbs.

use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::kinds::{EntryKind, SearchScope};
use crate::{PluginEntry, PluginError, PluginResult};

// ── InitContext ───────────────────────────────────────────────────────────────

/// Context passed to `Plugin::init`. Carries resolved env, config, cache dir,
/// and a logger handle.
pub struct InitContext<'a> {
    pub env: &'a HashMap<String, String>,
    pub config: &'a HashMap<String, toml::Value>,
    pub cache_dir: &'a PathBuf,
    pub logger: &'a dyn PluginLogger,
}

/// Logging surface exposed to plugins (backed by `stui_log` host import at runtime,
/// no-op or stdout in test harness).
pub trait PluginLogger {
    fn debug(&self, msg: &str);
    fn info(&self, msg: &str);
    fn warn(&self, msg: &str);
    fn error(&self, msg: &str);
}

/// Result of `Plugin::init`. `MissingConfig` is soft — user-fixable via TUI;
/// `Fatal` is hard — code bug or trap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginInitError {
    MissingConfig { fields: Vec<String>, hint: Option<String> },
    Fatal(String),
}

// ── Lookup ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupResponse {
    pub entry: PluginEntry,
}

// ── Enrich ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichRequest {
    pub partial: PluginEntry,
    pub prefer_id_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichResponse {
    pub entry: PluginEntry,
    pub confidence: f32,  // 0.0..=1.0
}

// ── Artwork ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkSize { Thumbnail, Standard, HiRes, Any }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub size: ArtworkSize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkVariant {
    pub size: ArtworkSize,
    pub url: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkResponse {
    pub variants: Vec<ArtworkVariant>,
}

// ── Credits ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastRole {
    Actor,
    Vocalist,
    FeaturedArtist,
    GuestAppearance,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    pub role: CastRole,
    pub character: Option<String>,
    pub instrument: Option<String>,
    pub billing_order: Option<u32>,
    pub external_ids: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrewRole {
    Director, Writer, Producer, ExecutiveProducer,
    Cinematographer, Editor, Composer,
    Songwriter, Lyricist, Arranger, Instrumentalist,
    ProductionDesigner, ArtDirector, CostumeDesigner,
    SoundDesigner, VfxSupervisor,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewMember {
    pub name: String,
    pub role: CrewRole,
    pub department: Option<String>,
    pub external_ids: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsResponse {
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
}

/// Normalize upstream crew-role strings into canonical `CrewRole` variants.
/// Unrecognized strings map to `CrewRole::Other(s)`.
pub fn normalize_crew_role(s: &str) -> CrewRole {
    match s.to_lowercase().as_str() {
        "director" => CrewRole::Director,
        "writer" | "screenplay" | "screenwriter" => CrewRole::Writer,
        "producer" => CrewRole::Producer,
        "executive producer" => CrewRole::ExecutiveProducer,
        "cinematographer" | "director of photography" | "dp" | "dop" => CrewRole::Cinematographer,
        "editor" => CrewRole::Editor,
        "composer" | "original music composer" => CrewRole::Composer,
        "songwriter" => CrewRole::Songwriter,
        "lyricist" => CrewRole::Lyricist,
        "arranger" => CrewRole::Arranger,
        "instrumentalist" | "session musician" => CrewRole::Instrumentalist,
        "production designer" => CrewRole::ProductionDesigner,
        "art director" => CrewRole::ArtDirector,
        "costume designer" => CrewRole::CostumeDesigner,
        "sound designer" => CrewRole::SoundDesigner,
        "vfx supervisor" | "visual effects supervisor" => CrewRole::VfxSupervisor,
        _ => CrewRole::Other(s.to_string()),
    }
}

// ── Related ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    SameArtist, SameDirector, SameStudio, Similar, Sequel, Compilation, Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub relation: RelationKind,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedResponse {
    pub items: Vec<PluginEntry>,
}

// ── err_not_implemented helper ────────────────────────────────────────────────

/// Canonical helper for default-method bodies on optional `CatalogPlugin`
/// verbs. Returns a `PluginResult::Err` with the `NOT_IMPLEMENTED` code.
pub fn err_not_implemented<T>() -> PluginResult<T> {
    PluginResult::err(
        crate::error_codes::NOT_IMPLEMENTED,
        "verb not implemented by this plugin",
    )
}

// ── Slim manifest validator (used by CLI lint/build) ──────────────────────────

/// Schema-only manifest validation. Covers:
/// - Legacy fields rejected ([plugin] type, [permissions] network=bool, filesystem).
/// - Canonical id-sources in [capabilities.catalog] lookup.id_sources.
/// - Required verb presence (search = true on CatalogPlugin).
/// - Declared-kinds are recognized EntryKind values.
///
/// The runtime's full validator in `runtime::plugin::manifest::validate()`
/// is a superset that adds runtime-only concerns (e.g., network allowlist
/// resolution against real DNS, filesystem ACL checks). This slim version
/// is sufficient for static checks in `stui plugin lint` / `stui plugin build`.
pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), ManifestValidationError> {
    // Legacy [plugin] type field
    if manifest.plugin.plugin_type.is_some() {
        return Err(ManifestValidationError::LegacyField(
            "[plugin] type = \"...\" is no longer supported; plugin type is inferred from [capabilities.*]".into(),
        ));
    }

    // Legacy [permissions] network = true bool
    if let Some(perms) = &manifest.permissions {
        if matches!(perms.network, Some(NetworkPermission::Bool(_))) {
            return Err(ManifestValidationError::LegacyField(
                "[permissions] network = true is no longer supported; use network = [\"host1\", ...]".into(),
            ));
        }
        if perms.filesystem.is_some() {
            return Err(ManifestValidationError::LegacyField(
                "[permissions] filesystem is not supported for metadata plugins".into(),
            ));
        }
    }

    // Canonical id-sources in lookup.id_sources
    if let Some(catalog) = &manifest.capabilities.catalog {
        if let Some(lookup_cfg) = catalog.lookup.as_ref().and_then(|v| v.as_detail()) {
            if let Some(sources) = &lookup_cfg.id_sources {
                for source in sources {
                    if !crate::id_sources::is_canonical(source) {
                        return Err(ManifestValidationError::UnknownIdSource(source.clone()));
                    }
                }
            }
        }

        // Required verb: search must be declared true (not stub, not false)
        let search_ok = catalog.search.as_ref()
            .map(|v| v.is_enabled() && !v.is_stub())
            .unwrap_or(false);
        if !search_ok {
            return Err(ManifestValidationError::MissingRequiredVerb("search".into()));
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum ManifestValidationError {
    #[error("legacy manifest field: {0}")]
    LegacyField(String),

    #[error("unknown id-source: {0} (see sdk::id_sources for canonical set)")]
    UnknownIdSource(String),

    #[error("required verb not declared: {0}")]
    MissingRequiredVerb(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_crew_role_common_aliases() {
        assert!(matches!(normalize_crew_role("Director"), CrewRole::Director));
        assert!(matches!(normalize_crew_role("director of photography"), CrewRole::Cinematographer));
        assert!(matches!(normalize_crew_role("DOP"), CrewRole::Cinematographer));
        assert!(matches!(normalize_crew_role("Original Music Composer"), CrewRole::Composer));
    }

    #[test]
    fn normalize_crew_role_unknown_is_other() {
        match normalize_crew_role("Foley Artist") {
            CrewRole::Other(s) => assert_eq!(s, "Foley Artist"),
            _ => panic!("expected Other variant"),
        }
    }

    #[test]
    fn plugin_init_error_serde_tagged() {
        let e = PluginInitError::MissingConfig {
            fields: vec!["api_key".into()],
            hint: Some("Get a key at example.com".into()),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"kind\":\"missing_config\""));
        assert!(s.contains("api_key"));
    }

    #[test]
    fn cast_role_other_variant_serializes() {
        let r = CastRole::Other("Extra".to_string());
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("Extra"));
    }
}
```

- [ ] **Step 2: Re-export in `sdk/src/lib.rs`**

```rust
pub mod capabilities;
pub use capabilities::{
    InitContext, PluginLogger, PluginInitError,
    LookupRequest, LookupResponse,
    EnrichRequest, EnrichResponse,
    ArtworkRequest, ArtworkResponse, ArtworkSize, ArtworkVariant,
    CreditsRequest, CreditsResponse, CastMember, CastRole, CrewMember, CrewRole,
    RelatedRequest, RelatedResponse, RelationKind,
    err_not_implemented, normalize_crew_role,
};
```

- [ ] **Step 3: Build**

```
cd sdk && cargo build --lib
```

Expected: clean build.

- [ ] **Step 4: Run tests**

```
cd sdk && cargo test --lib capabilities
```

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```
git add sdk/src/capabilities.rs sdk/src/lib.rs
git commit -m "feat(sdk): capabilities module — verb request/response types + err_not_implemented + normalize_crew_role"
```

### Task 1.4: Extend `PluginEntry` with `external_ids`

**Files:**
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Locate current `PluginEntry` struct**

```
grep -n "pub struct PluginEntry" sdk/src/lib.rs
```

- [ ] **Step 2: Write failing test**

Add to SDK tests:

```rust
#[test]
fn plugin_entry_carries_external_ids() {
    use std::collections::HashMap;
    let mut external = HashMap::new();
    external.insert("imdb".to_string(), "tt1234567".to_string());
    external.insert("musicbrainz".to_string(), "uuid-1".to_string());

    let entry = PluginEntry {
        id: "tmdb-100".into(),
        kind: EntryKind::Movie,
        title: "Test".into(),
        source: "tmdb".into(),
        external_ids: external,
        ..Default::default()
    };
    let s = serde_json::to_string(&entry).unwrap();
    assert!(s.contains("\"external_ids\""));
    assert!(s.contains("tt1234567"));
}
```

- [ ] **Step 3: Run test, expect compile failure**

```
cd sdk && cargo test plugin_entry_carries_external_ids
```

Expected: compile failure on missing field.

- [ ] **Step 4: Add field to `PluginEntry`**

Immediately after `source` and before the optional fields:

```rust
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub external_ids: std::collections::HashMap<String, String>,
```

- [ ] **Step 5: Run tests**

```
cd sdk && cargo test --lib
```

Expected: pass.

- [ ] **Step 6: Commit**

```
git add sdk/src/lib.rs
git commit -m "feat(sdk): PluginEntry.external_ids for cross-namespace ids"
```

### Task 1.5: Add `Plugin` + `CatalogPlugin` traits

**Files:**
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Locate existing `StuiPlugin` trait**

```
grep -n "pub trait StuiPlugin" sdk/src/lib.rs
```

- [ ] **Step 2: Add `Plugin` and `CatalogPlugin` traits**

After the existing `StuiPlugin` trait (which stays for non-metadata
plugins, marked `#[deprecated]`):

```rust
/// Root trait every plugin implements — identity + lifecycle.
pub trait Plugin {
    fn manifest(&self) -> &PluginManifest;
    fn init(&mut self, _ctx: &InitContext) -> Result<(), PluginInitError> { Ok(()) }
    fn shutdown(&mut self) -> Result<(), PluginError> { Ok(()) }
}

/// Metadata catalog capability. Plugins opt into this trait when they expose
/// `[capabilities.catalog]` in their manifest. All verbs except `search` are
/// optional; default impls return `NOT_IMPLEMENTED`.
pub trait CatalogPlugin: Plugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse>;

    fn lookup(&self, _req: LookupRequest) -> PluginResult<LookupResponse>
        { err_not_implemented() }
    fn enrich(&self, _req: EnrichRequest) -> PluginResult<EnrichResponse>
        { err_not_implemented() }
    fn get_artwork(&self, _req: ArtworkRequest) -> PluginResult<ArtworkResponse>
        { err_not_implemented() }
    fn get_credits(&self, _req: CreditsRequest) -> PluginResult<CreditsResponse>
        { err_not_implemented() }
    fn related(&self, _req: RelatedRequest) -> PluginResult<RelatedResponse>
        { err_not_implemented() }
}
```

Also add `#[deprecated]` attribute on the existing `StuiPlugin` trait,
with a note pointing to `Plugin` + `CatalogPlugin`:

```rust
#[deprecated(
    since = "0.2.0",
    note = "Use `Plugin` + `CatalogPlugin` instead. StuiPlugin remains for non-metadata plugins in stui_plugins/ pending the media-source plugin refactor."
)]
pub trait StuiPlugin { /* existing body unchanged */ }
```

- [ ] **Step 3: Write compile-time assertion test**

```rust
#[cfg(test)]
#[test]
fn plugin_trait_compiles() {
    fn assert_plugin<T: Plugin>() {}
    fn assert_catalog<T: CatalogPlugin>() {}
    // Compile-time only; no runtime assertions needed.
}
```

- [ ] **Step 4: Build**

```
cd sdk && cargo build --lib
```

Expected: build clean; deprecation warnings are expected on `StuiPlugin`
usages but not errors.

- [ ] **Step 5: Commit**

```
git add sdk/src/lib.rs
git commit -m "feat(sdk): add Plugin + CatalogPlugin traits; deprecate StuiPlugin"
```

### Task 1.6: Extend `stui_export_plugin!` macro for new verbs

**Files:**
- Modify: `sdk/src/lib.rs` (the macro definition)

- [ ] **Step 1: Locate current macro**

```
grep -n "stui_export_plugin\|macro_rules" sdk/src/lib.rs
```

Expected: find the existing `macro_rules! stui_export_plugin` that
exports `stui_abi_version`, `stui_alloc`, `stui_free`, `stui_search`,
`stui_resolve`.

- [ ] **Step 2: Add new ABI export functions to the macro body**

The macro should expand to include:

```
#[no_mangle]
pub extern "C" fn stui_lookup(ptr: i32, len: i32) -> i64 {
    $crate::abi_call_wrapper::<$ty, _, _>(ptr, len, |p, req: $crate::LookupRequest| {
        <$ty as $crate::CatalogPlugin>::lookup(&p, req)
    })
}

#[no_mangle]
pub extern "C" fn stui_enrich(ptr: i32, len: i32) -> i64 { /* same shape */ }

#[no_mangle]
pub extern "C" fn stui_get_artwork(ptr: i32, len: i32) -> i64 { /* same shape */ }

#[no_mangle]
pub extern "C" fn stui_get_credits(ptr: i32, len: i32) -> i64 { /* same shape */ }

#[no_mangle]
pub extern "C" fn stui_related(ptr: i32, len: i32) -> i64 { /* same shape */ }
```

`abi_call_wrapper` is a helper the SDK already has (from search refactor);
reuse it. If it doesn't exist, extract it from the existing
`stui_search` export into a reusable generic.

- [ ] **Step 3: Compile check**

```
cd sdk && cargo build --lib
```

Expected: clean. (Macro not actually expanded against a test plugin yet;
that happens in Chunk 3.)

- [ ] **Step 4: Commit**

```
git add sdk/src/lib.rs
git commit -m "feat(sdk): stui_export_plugin! generates FFI for all 6 CatalogPlugin verbs"
```

### Task 1.7: Convert `runtime/src/plugin.rs` to `plugin/` module dir

**Files:**
- Delete: `runtime/src/plugin.rs`
- Create: `runtime/src/plugin/mod.rs`, `runtime/src/plugin/manifest.rs`,
  `runtime/src/plugin/loader.rs`, `runtime/src/plugin/state.rs`,
  `runtime/src/plugin/dispatcher.rs`, `runtime/src/plugin/supervisor.rs`

This is a big task — do it in clean moves.

- [ ] **Step 1: Create `runtime/src/plugin/mod.rs` skeleton**

```rust
//! Plugin subsystem: loader, state, dispatcher, supervisor.
//! See docs/superpowers/specs/2026-04-20-plugin-refactor-design.md §2
//! for architecture.

pub mod manifest;
pub mod loader;
pub mod state;
pub mod dispatcher;
pub mod supervisor;

// Re-export the types callers outside this module need.
pub use manifest::{PluginManifest, Capabilities, CatalogCapability, NetworkPermission, RateLimit};
pub use state::{PluginStatus, PluginState, StateStore};
pub use dispatcher::Dispatcher;
pub use loader::{LoaderError, load_from_dir};
```

- [ ] **Step 2: Move manifest types to `runtime/src/plugin/manifest.rs`**

Extract from today's `runtime/src/plugin.rs`:

```rust
// Contents: PluginManifest, PluginMeta, Capabilities, CatalogCapability,
// NetworkPermission, Permissions, RateLimit, PluginConfigField,
// deserialize_config_fields helper, and all related tests.
//
// Start by copying today's runtime/src/plugin.rs contents, then split.
```

- [ ] **Step 3: Strict-validate additions to `manifest.rs`**

Add validation logic (new — not in today's plugin.rs):

```rust
/// Validate a freshly-parsed manifest against the new canonical schema.
///
/// Returns a `ManifestValidationError` describing what's wrong so the loader
/// can surface an actionable message.
pub fn validate(manifest: &PluginManifest) -> Result<(), ManifestValidationError> {
    // 1. Legacy fields rejected
    if manifest.plugin.plugin_type.is_some() {
        return Err(ManifestValidationError::LegacyField(
            "[plugin] type = \"...\" is no longer supported; plugin type is inferred from [capabilities.*]"
                .to_string(),
        ));
    }
    // 2. network permission must be an allowlist (not bool)
    if let Some(Permissions { network: Some(NetworkPermission::Bool(_)), .. }) = manifest.permissions {
        return Err(ManifestValidationError::LegacyField(
            "[permissions] network = true is no longer supported; use network = [\"host1\", ...]"
                .to_string(),
        ));
    }
    // 3. filesystem permission rejected for metadata plugins
    if let Some(Permissions { filesystem: Some(_), .. }) = manifest.permissions {
        return Err(ManifestValidationError::LegacyField(
            "[permissions] filesystem is not supported for metadata plugins".to_string(),
        ));
    }
    // 4. capabilities.catalog.kinds are valid EntryKinds (already typed via serde)
    // 5. capabilities.catalog.lookup.id_sources are canonical
    if let Some(catalog) = &manifest.capabilities.catalog {
        if let Some(lookup) = &catalog.lookup {
            for source in &lookup.id_sources {
                if !stui_plugin_sdk::id_sources::is_canonical(source) {
                    return Err(ManifestValidationError::UnknownIdSource(source.clone()));
                }
            }
        }
    }
    // 6. catalog.search must be true (the one required verb)
    if let Some(catalog) = &manifest.capabilities.catalog {
        if !catalog.search.unwrap_or(false) {
            return Err(ManifestValidationError::MissingRequiredVerb("search".to_string()));
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestValidationError {
    #[error("legacy manifest field: {0}")]
    LegacyField(String),
    #[error("unknown id-source: {0} (see sdk::id_sources for canonical set)")]
    UnknownIdSource(String),
    #[error("required verb not declared: {0}")]
    MissingRequiredVerb(String),
}
```

Add tests exercising each validation branch.

- [ ] **Step 4: Move loader logic to `runtime/src/plugin/loader.rs`**

Extract `load_manifest` and WASM instantiation code from today's
`runtime/src/plugin.rs`. Rewrite to use `validate()` after parsing:

```rust
pub fn load_from_dir(dir: &Path) -> Result<LoadedPlugin, LoaderError> {
    let manifest = parse_manifest(dir)?;
    manifest::validate(&manifest)?;
    // ... WASM instantiation, config resolution, init() call ...
}
```

- [ ] **Step 5: Move `PluginStatus` + state map to `runtime/src/plugin/state.rs`**

Today's `PluginRegistry` in `runtime/src/plugin.rs` renames to
`StateStore` and expands to track the 4-state `PluginStatus` enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PluginStatus {
    Loaded,
    NeedsConfig { missing: Vec<String>, hint: Option<String> },
    Failed { reason: String, at: SystemTime },
    Disabled,
}
```

`StateStore` holds `HashMap<String, PluginState>` keyed by manifest
`name`. Add methods: `get`, `list`, `status`, `set_status`, `reload`,
`resolve_config`.

**`resolve_config` precedence (spec §2):** user TUI settings >
`env_var` (from `[[config]]` definition) > `env` defaults (from
`[env]` manifest section) > `[[config]] default`. Add a unit test
that exercises all four levels to lock the order.

Also, **add a `VerbConfig::is_stub() -> bool`** method on the
verb-config untagged-enum type (`bool | { stub: true, reason: "..." } |
{ id_sources: [...] } | { sizes: [...] }`). Used by
`stui plugin build --release` to reject stubs. Unit test: each variant
returns the expected value.

- [ ] **Step 6: Extract dispatcher to `runtime/src/plugin/dispatcher.rs`**

Today's dispatch-map logic in `runtime/src/engine/dispatch_map.rs` stays
where it is for search-scope routing. Add new in `plugin/dispatcher.rs`:

```rust
pub struct Dispatcher {
    // maps rebuilt on state change
    by_scope:   HashMap<SearchScope, Vec<String>>,
    by_lookup:  HashMap<(String /* id_source */, EntryKind), Vec<String>>,
    by_enrich:  HashMap<EntryKind, Vec<String>>,
    by_artwork: HashMap<EntryKind, Vec<String>>,
    by_credits: HashMap<EntryKind, Vec<String>>,
    by_related: HashMap<EntryKind, Vec<String>>,
}

impl Dispatcher {
    pub fn rebuild(plugins: &[LoadedPluginSummary]) -> Self { ... }
    pub fn plugins_for_scope(&self, scope: SearchScope) -> Vec<String> { ... }
    pub fn plugins_for_lookup(&self, id_source: &str, kind: EntryKind) -> Vec<String> { ... }
    // ... etc ...
}
```

`LoadedPluginSummary` is a lightweight view (id + declared capabilities)
that the state store exposes to dispatcher rebuild.

- [ ] **Step 7: Extract supervisor to `runtime/src/plugin/supervisor.rs`**

Today's `runtime/src/abi/supervisor.rs` stays for WASM-instance-level
concerns. The new `plugin/supervisor.rs` wraps it with the per-plugin
**rate limiter** (token bucket):

```rust
pub struct PluginSupervisor {
    wasm: Arc<abi::supervisor::WasmSupervisor>,
    rate_limit: Option<TokenBucket>,
}

impl PluginSupervisor {
    pub async fn call<F, T>(&self, call: F) -> Result<T, PluginCallError>
    where
        F: FnOnce() -> Future<Output = Result<T, PluginCallError>>,
    {
        if let Some(bucket) = &self.rate_limit {
            bucket.acquire().await?;  // returns RATE_LIMITED error if empty
        }
        self.wasm.call(call).await
    }
}
```

- [ ] **Step 8: Delete `runtime/src/plugin.rs`**

```
rm runtime/src/plugin.rs
```

- [ ] **Step 9: Fix `runtime/src/lib.rs` or wherever `mod plugin;` lives**

Should already work — `mod plugin;` resolves to `plugin/mod.rs` now.

- [ ] **Step 10: Build**

```
cd runtime && cargo build --lib
```

Expected: clean. Some deprecation warnings are OK (StuiPlugin usage).

- [ ] **Step 11: Run all existing plugin tests**

```
cd runtime && cargo test --lib plugin
```

Expected: pre-existing tests pass (perhaps moved into sub-modules).

- [ ] **Step 12: Commit**

```
git add runtime/src/plugin/ runtime/src/plugin.rs  # git detects the move
git commit -m "refactor(runtime): split plugin.rs into plugin/ module (loader, state, dispatcher, supervisor, manifest); add strict-validate"
```

### Task 1.8: Extend ABI types in `runtime/src/abi/types.rs`

**Files:**
- Modify: `runtime/src/abi/types.rs`

ABI-side types mirror SDK types (for JSON-over-WASM).

- [ ] **Step 1: Add ABI types for new verbs**

```rust
// Mirror SDK types — same fields, separate struct because of
// serde_json ↔ wasmtime boundary concerns. Keep in lock-step with SDK.

#[derive(Serialize, Deserialize)]
pub struct LookupRequest { /* fields as in SDK */ }
#[derive(Serialize, Deserialize)]
pub struct LookupResponse { /* ... */ }
// EnrichRequest/Response, ArtworkRequest/Response + ArtworkSize + ArtworkVariant,
// CreditsRequest/Response + CastMember/CastRole/CrewMember/CrewRole,
// RelatedRequest/Response + RelationKind
```

- [ ] **Step 2: Build**

```
cd runtime && cargo build --lib
```

- [ ] **Step 3: Commit**

```
git add runtime/src/abi/types.rs
git commit -m "feat(abi): add lookup/enrich/artwork/credits/related types"
```

### Task 1.9: Extend `WasmHost` and `WasmSupervisor` with new verb calls

**Files:**
- Modify: `runtime/src/abi/host.rs`
- Modify: `runtime/src/abi/supervisor.rs`

- [ ] **Step 1: Add to `WasmHost`**

Following the existing `search()` pattern:

```rust
impl WasmHost {
    pub async fn lookup(&mut self, req: &LookupRequest) -> Result<LookupResponse, AbiError> { /* ... */ }
    pub async fn enrich(&mut self, req: &EnrichRequest) -> Result<EnrichResponse, AbiError> { /* ... */ }
    pub async fn get_artwork(&mut self, req: &ArtworkRequest) -> Result<ArtworkResponse, AbiError> { /* ... */ }
    pub async fn get_credits(&mut self, req: &CreditsRequest) -> Result<CreditsResponse, AbiError> { /* ... */ }
    pub async fn related(&mut self, req: &RelatedRequest) -> Result<RelatedResponse, AbiError> { /* ... */ }
}
```

Each calls `stui_<verb>(ptr, len)` on the wasm instance. Copy the
existing `search()` body as a template.

- [ ] **Step 2: Add to `WasmSupervisor` — timeout-wrapped**

```rust
impl WasmSupervisor {
    pub async fn lookup(&self, req: &LookupRequest) -> Result<LookupResponse, AbiError> {
        // same pattern as search()
    }
    // ... etc ...
}
```

- [ ] **Step 3: Build**

```
cd runtime && cargo build --lib
```

- [ ] **Step 4: Commit**

```
git add runtime/src/abi/host.rs runtime/src/abi/supervisor.rs
git commit -m "feat(abi): wire lookup/enrich/artwork/credits/related through WasmHost + Supervisor"
```

### Task 1.10: Extend `Engine` with verb-specific helpers

**Files:**
- Modify: `runtime/src/engine/mod.rs`

- [ ] **Step 1: Add helpers mirroring `supervisor_search`**

```rust
impl Engine {
    pub async fn supervisor_lookup(
        &self,
        plugin_name: &str,
        req: LookupRequest,
    ) -> Result<PluginEntry, PluginCallError> {
        // 1. Acquire rate-limit permit + plugin_semaphore permit
        // 2. Look up plugin by name in state store
        // 3. Call supervisor.lookup(req)
        // 4. Return .entry from response (or map errors)
    }
    pub async fn supervisor_enrich(&self, plugin_name: &str, req: EnrichRequest) -> /* ... */;
    pub async fn supervisor_get_artwork(&self, plugin_name: &str, req: ArtworkRequest) -> /* ... */;
    pub async fn supervisor_get_credits(&self, plugin_name: &str, req: CreditsRequest) -> /* ... */;
    pub async fn supervisor_related(&self, plugin_name: &str, req: RelatedRequest) -> /* ... */;
}
```

- [ ] **Step 2: Add unit tests** with a mocked supervisor (or
  integration test scaffold — may need to defer to Chunk 7's
  integration tests if supervisor-mocking is too invasive).

- [ ] **Step 3: Build**

```
cd runtime && cargo build --lib
```

- [ ] **Step 4: Commit**

```
git add runtime/src/engine/mod.rs
git commit -m "feat(engine): per-verb supervisor helpers (lookup, enrich, artwork, credits, related)"
```

### Task 1.11: Extend IPC `Request`/`Response` for new verbs (Rust only)

**Files:**
- Modify: `runtime/src/ipc/v1/mod.rs`

Go-side IPC mirrors are intentionally deferred to a later TUI-surface
task. The new verbs have no TUI consumer in this refactor (tag
normalization uses lookup/enrich directly via internal engine helpers
in `runtime/src/mediacache/`). Adding Go-side types now would create
dead client code. Deferred to the media-source-plugin refactor or
whichever task first surfaces the new verbs in the TUI.

- [ ] **Step 1: Add Rust IPC variants**

In `Request` enum:

```rust
Lookup(LookupIpcRequest),
Enrich(EnrichIpcRequest),
GetArtwork(ArtworkIpcRequest),
GetCredits(CreditsIpcRequest),
Related(RelatedIpcRequest),
```

And corresponding `Response` variants. The IPC wrappers carry a `query_id`
and plugin-targeting info on top of the SDK request.

- [ ] **Step 2: Build**

```
cd runtime && cargo build --lib
```

Expected: clean.

- [ ] **Step 3: Commit**

```
git add runtime/src/ipc/
git commit -m "feat(ipc): add verb-specific Request/Response variants (lookup/enrich/artwork/credits/related)"
```

### Task 1.12: Dispatcher integration for new verbs in `main.rs`

**Files:**
- Modify: `runtime/src/main.rs`

- [ ] **Step 1: Find `Request::Search` arm**

Model new arms on it. Each arm looks up the plugin by name, calls the
Engine helper, returns the Response variant.

- [ ] **Step 2: Add stub arms for `Lookup`, `Enrich`, `GetArtwork`,
  `GetCredits`, `Related`**

Each stub routes to `engine.supervisor_<verb>()`.

- [ ] **Step 3: Build**

```
cd runtime && cargo build
```

- [ ] **Step 4: Commit**

```
git add runtime/src/main.rs
git commit -m "feat(ipc): dispatch new verb requests to engine helpers"
```

---

## Chunk 1 Review Checkpoint

After all Chunk 1 tasks land:

- [ ] Full build: `cargo build` from workspace root. Clean.
- [ ] Tests: `cargo test --lib`. Only pre-existing failures (rustls/auth). No new failures.
- [ ] Plugin crates do NOT build (expected — they're still using old `StuiPlugin`).
- [ ] `stui_plugins/` remote plugins still build against deprecated `StuiPlugin` (run their CI separately).
- [ ] **File a mirror manifest-migration PR against `stui_plugins/`**
  before merging Chunk 1 to main. The strict loader will reject any
  remaining legacy fields in external plugin manifests. Coordinated
  landing: Chunk 1 merge + `stui_plugins/` sweep merge at the same
  time (or the latter first). See spec §7 Chunk 1 cross-repo note.

---

## Chunk 2 — CLI Tooling (`stui plugin ...`)

Creates the new `cli/` workspace member and `stui` binary with 5 subcommands.

### Task 2.1: Scaffold `cli/` crate

**Files:**
- Create: `cli/Cargo.toml`
- Create: `cli/src/main.rs`
- Modify: `Cargo.toml` (workspace root, add `"cli"` to members)

- [ ] **Step 1: Create `cli/Cargo.toml`**

```toml
[package]
name = "stui"
version = "0.1.0"
edition = "2021"
description = "STUI plugin author CLI"

[[bin]]
name = "stui"
path = "src/main.rs"

[dependencies]
anyhow           = { workspace = true }
clap             = { version = "4", features = ["derive"] }
serde            = { workspace = true }
toml             = "0.8"
walkdir          = "2"
tracing          = { workspace = true }
tracing-subscriber = "0.3"
stui-plugin-sdk  = { path = "../sdk" }
```

- [ ] **Step 2: Create `cli/src/main.rs` with clap skeleton**

```rust
use clap::{Parser, Subcommand};

mod cmd;

#[derive(Parser)]
#[command(name = "stui", version, about = "STUI plugin author CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(subcommand)]
    Plugin(cmd::plugin::PluginCmd),
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Plugin(plugin_cmd) => cmd::plugin::run(plugin_cmd),
    }
}
```

- [ ] **Step 3: Add to workspace members**

In workspace `Cargo.toml`:

```toml
members = [
    "runtime",
    "sdk",
    "aria2",
    "cli",       # new
    "plugins/*",
]
```

- [ ] **Step 4: Build**

```
cargo build -p stui
```

Expected: clean.

- [ ] **Step 5: Commit**

```
git add cli/ Cargo.toml Cargo.lock
git commit -m "feat(cli): scaffold stui binary with clap subcommand tree"
```

### Task 2.2: `stui plugin init <name>` — scaffold template

**Files:**
- Create: `cli/src/cmd/mod.rs`
- Create: `cli/src/cmd/plugin.rs`
- Create: `cli/src/cmd/init.rs`
- Create: `cli/src/template/*` — embedded template files

- [ ] **Step 1: Subcommand enum in `cli/src/cmd/plugin.rs`**

```rust
use clap::Subcommand;

#[derive(Subcommand)]
pub enum PluginCmd {
    /// Scaffold a new plugin skeleton.
    Init {
        /// Plugin name (short form; will be manifest.plugin.name).
        name: String,
        /// Target directory (defaults to ./<name>-provider).
        #[arg(short, long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Build the plugin to wasm32-wasip1.
    Build {
        #[arg(long)]
        release: bool,
    },
    /// Run plugin tests with the mocked host harness.
    Test,
    /// Lint the plugin manifest + impl surface.
    Lint,
    /// Install the built plugin to ~/.stui/plugins/<name>/ (dev-mode: symlink).
    Install {
        #[arg(long)]
        dev: bool,
    },
}

pub fn run(cmd: PluginCmd) -> anyhow::Result<()> {
    match cmd {
        PluginCmd::Init { name, dir } => crate::cmd::init::run(name, dir),
        PluginCmd::Build { release } => crate::cmd::build::run(release),
        PluginCmd::Test => crate::cmd::test::run(),
        PluginCmd::Lint => crate::cmd::lint::run(),
        PluginCmd::Install { dev } => crate::cmd::install::run(dev),
    }
}
```

- [ ] **Step 2: Create embedded template files**

Under `cli/src/template/`:
- `Cargo.toml.template` — contains `{{PLUGIN_NAME}}` placeholder
- `plugin.toml.template`
- `src/lib.rs.template`
- `tests/basic.rs.template`
- `tests/fixtures/example.json`
- `README.md.template`

Template `plugin.toml.template` — **must be loadable + pass lint as-is**
so `stui plugin init <name> && stui plugin build && stui plugin lint`
succeeds out of the box:

```toml
[plugin]
name        = "{{PLUGIN_NAME}}"
version     = "0.1.0"
abi_version = 1
description = "{{PLUGIN_NAME}} metadata provider"

[meta]
author  = "you"
license = "MIT"

[permissions]
network = []  # add API hostnames here

[permissions.rate_limit]
requests_per_second = 1

[capabilities.catalog]
kinds = ["movie"]   # scope the plugin handles; replace with real kinds

search = true        # required verb — implemented as empty-result stub
# lookup  = false     # uncomment + implement once ready
# enrich  = false
# artwork = false
# credits = false
# related = false
```

Template `src/lib.rs.template` — produces a runnable plugin that
lint-passes:

```rust
//! {{PLUGIN_NAME}} — stui metadata plugin.

use stui_plugin_sdk::{
    Plugin, CatalogPlugin,
    PluginManifest, PluginResult,
    SearchRequest, SearchResponse,
    stui_export_plugin,
};

pub struct {{PLUGIN_TYPE}} {
    manifest: PluginManifest,
}

impl {{PLUGIN_TYPE}} {
    pub fn new() -> Self {
        Self {
            manifest: include_manifest!("plugin.toml"),
        }
    }
}

impl Plugin for {{PLUGIN_TYPE}} {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
}

impl CatalogPlugin for {{PLUGIN_TYPE}} {
    fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
        // Empty-result stub — replace with real implementation.
        PluginResult::Ok(SearchResponse { items: vec![], total: 0 })
    }
    // Other verbs: default impls return NOT_IMPLEMENTED. Uncomment the
    // declarations in plugin.toml and override here when ready.
}

stui_export_plugin!({{PLUGIN_TYPE}});
```

`{{PLUGIN_TYPE}}` substitution = manifest name converted to CamelCase
(e.g., `musicbrainz` → `MusicbrainzPlugin`). `include_manifest!` is an
SDK macro (add as part of Task 1.6) that reads `plugin.toml` at
compile time and returns a `PluginManifest`.

- [ ] **Step 3: Implement `cli/src/cmd/init.rs`**

```rust
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const CARGO_TOML: &str = include_str!("../template/Cargo.toml.template");
const PLUGIN_TOML: &str = include_str!("../template/plugin.toml.template");
const LIB_RS: &str = include_str!("../template/src/lib.rs.template");
const BASIC_TEST: &str = include_str!("../template/tests/basic.rs.template");
const FIXTURE: &str = include_str!("../template/tests/fixtures/example.json");
const README: &str = include_str!("../template/README.md.template");

pub fn run(name: String, dir: Option<PathBuf>) -> Result<()> {
    let dir = dir.unwrap_or_else(|| PathBuf::from(format!("{}-provider", name)));
    if dir.exists() {
        anyhow::bail!("target directory already exists: {}", dir.display());
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::create_dir_all(dir.join("tests").join("fixtures"))?;

    let subst = |s: &str| s.replace("{{PLUGIN_NAME}}", &name);
    std::fs::write(dir.join("Cargo.toml"), subst(CARGO_TOML))?;
    std::fs::write(dir.join("plugin.toml"), subst(PLUGIN_TOML))?;
    std::fs::write(dir.join("src/lib.rs"), subst(LIB_RS))?;
    std::fs::write(dir.join("tests/basic.rs"), subst(BASIC_TEST))?;
    std::fs::write(dir.join("tests/fixtures/example.json"), FIXTURE)?;
    std::fs::write(dir.join("README.md"), subst(README))?;

    println!("Scaffolded plugin '{}' at {}", name, dir.display());
    println!("Next: cd {} && stui plugin build && stui plugin lint", dir.display());
    Ok(())
}
```

- [ ] **Step 4: Test**

```
cargo run -p stui -- plugin init testplugin --dir /tmp/testplugin
ls /tmp/testplugin
rm -rf /tmp/testplugin
```

Expected: directory created with all files; listing shows Cargo.toml,
plugin.toml, src/lib.rs, tests/basic.rs, tests/fixtures/, README.md.

- [ ] **Step 5: Commit**

```
git add cli/src/
git commit -m "feat(cli): stui plugin init scaffolds canonical template"
```

### Task 2.3: `stui plugin build` — compile + validate manifest

**Files:**
- Create: `cli/src/cmd/build.rs`

- [ ] **Step 1: Implement `build.rs`**

```rust
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(release: bool) -> Result<()> {
    // 1. Locate Cargo.toml in cwd.
    // 2. Run: cargo build --target wasm32-wasip1 [--release].
    // 3. On success, locate the .wasm file in target/wasm32-wasip1/{debug,release}/.
    // 4. Read + validate plugin.toml. Bubble manifest::validate errors with
    //    actionable messages.
    // 5. Run lint (re-uses cmd::lint::run internally).
    // 6. If --release: abort if any stub verbs declared.

    let cwd = std::env::current_dir()?;
    if !cwd.join("plugin.toml").exists() {
        anyhow::bail!("no plugin.toml in current directory");
    }
    if !cwd.join("Cargo.toml").exists() {
        anyhow::bail!("no Cargo.toml in current directory");
    }

    let mut cmd = Command::new("cargo");
    cmd.args(&["build", "--target", "wasm32-wasip1"]);
    if release { cmd.arg("--release"); }
    let status = cmd.status().context("cargo build failed")?;
    if !status.success() { anyhow::bail!("cargo build exited non-zero"); }

    // Validate manifest against the schema-only validator shipped in
    // stui_plugin_sdk::capabilities::validate_manifest() (Task 1.3).
    // The full validator in runtime/src/plugin/manifest.rs is a
    // superset with runtime-only concerns; the SDK slim version
    // covers legacy-field rejection, id-source canonicality, and
    // required-verb presence — sufficient for CLI lint/build.
    let manifest_text = std::fs::read_to_string(cwd.join("plugin.toml"))?;
    let manifest: stui_plugin_sdk::PluginManifest = toml::from_str(&manifest_text)
        .context("plugin.toml is not valid TOML against PluginManifest schema")?;
    stui_plugin_sdk::capabilities::validate_manifest(&manifest)
        .context("plugin.toml failed manifest validation")?;

    // Run lint
    crate::cmd::lint::run()?;

    if release && has_stubs(&manifest) {
        anyhow::bail!("--release build rejected: plugin has stubbed verbs. Remove stubs or drop --release.");
    }

    println!("Build OK.");
    Ok(())
}

fn has_stubs(manifest: &stui_plugin_sdk::PluginManifest) -> bool {
    manifest.capabilities.catalog.as_ref().map_or(false, |cat| {
        [&cat.search, &cat.lookup, &cat.enrich, &cat.artwork, &cat.credits, &cat.related]
            .iter()
            .any(|v| v.as_ref().map_or(false, |c| c.is_stub()))
    })
}
```

`is_stub()` is a method on the verb-config untagged-enum
(`VerbConfig::Bool(b) | VerbConfig::Detail { stub: bool, ... }`)
added in Task 1.7 Step 2/3 on the manifest types so every verb-config
carries uniform stub detection.

- [ ] **Step 2: Test with the template from Task 2.2**

```
cd /tmp/testplugin-build
stui plugin init test --dir .
stui plugin build   # should succeed (or fail gracefully if template has issues)
```

- [ ] **Step 3: Commit**

```
git add cli/src/cmd/build.rs
git commit -m "feat(cli): stui plugin build compiles + validates manifest"
```

### Task 2.4: `stui plugin lint` — static checks

**Files:**
- Create: `cli/src/cmd/lint.rs`

- [ ] **Step 1: Implement lint checks**

```rust
pub fn run() -> Result<()> {
    // 1. Read + parse plugin.toml. Report invalid TOML.
    // 2. Run manifest::validate (schema rules).
    // 3. For each declared `[capabilities.catalog]` verb = true (non-stub),
    //    verify the trait impl in src/lib.rs is non-default. How:
    //    - Parse src/lib.rs with syn crate OR invoke `cargo check --tests`
    //      and look for a compile-time marker (e.g., a build.rs that asserts
    //      declared-verbs match).
    //    - Simpler: rely on a convention that plugins must include a test
    //      calling each declared verb and expecting non-`NOT_IMPLEMENTED`.
    // 4. Report stubs (declared but `{ stub = true }`) as warnings.
    // 5. Validate id-sources are canonical.
    // 6. Validate required config fields have `label` and `hint`.

    // Minimum viable: items 1, 2, 4, 5, 6. Item 3 is syn-based and
    // non-trivial; implement as a followup if the test-based convention
    // isn't sufficient.

    Ok(())
}
```

- [ ] **Step 2: Build + test**

```
cargo run -p stui -- plugin lint
```

- [ ] **Step 3: Commit**

```
git add cli/src/cmd/lint.rs
git commit -m "feat(cli): stui plugin lint validates manifest schema + declared capabilities"
```

### Task 2.5: `stui plugin test` — run plugin tests

**Files:**
- Create: `cli/src/cmd/test.rs`

- [ ] **Step 1: Implement**

```rust
pub fn run() -> Result<()> {
    // Plugins' tests live in tests/*.rs (not wasm target).
    // Run: cargo test.
    let status = std::process::Command::new("cargo")
        .args(&["test"])
        .status()?;
    if !status.success() { anyhow::bail!("tests failed"); }
    Ok(())
}
```

`cargo test` for plugins compiles to native (not wasm) so SDK's
mocked-host helpers in `sdk::host` are usable directly.

- [ ] **Step 2: Test**

```
cd /tmp/testplugin && cargo run -p stui -- plugin test
```

- [ ] **Step 3: Commit**

```
git add cli/src/cmd/test.rs
git commit -m "feat(cli): stui plugin test runs cargo test with mocked host"
```

### Task 2.6: `stui plugin install --dev` — dev-mode symlink

**Files:**
- Create: `cli/src/cmd/install.rs`

- [ ] **Step 1: Implement**

```rust
pub fn run(dev: bool) -> Result<()> {
    if !dev {
        anyhow::bail!("--dev is required for now; non-dev install is a future release feature");
    }
    let cwd = std::env::current_dir()?;
    let manifest: stui_plugin_sdk::PluginManifest = toml::from_str(
        &std::fs::read_to_string(cwd.join("plugin.toml"))?
    )?;
    let name = &manifest.plugin.name;
    let home = dirs::home_dir().context("no home dir")?;
    let target = home.join(".stui/plugins").join(name);
    if target.exists() {
        std::fs::remove_file(&target).ok();
        std::fs::remove_dir_all(&target).ok();
    }
    std::fs::create_dir_all(target.parent().unwrap())?;
    std::os::unix::fs::symlink(&cwd, &target)?;
    println!("Symlinked {} → {}", cwd.display(), target.display());
    println!("Hot-reload watcher will pick it up within 500ms.");
    Ok(())
}
```

Add `dirs = "5"` to `cli/Cargo.toml` deps.

- [ ] **Step 2: Test**

```
cd /tmp/testplugin && cargo run -p stui -- plugin install --dev
ls -la ~/.stui/plugins/test
```

Expected: symlink pointing to /tmp/testplugin.

- [ ] **Step 3: Commit**

```
git add cli/src/cmd/install.rs cli/Cargo.toml Cargo.lock
git commit -m "feat(cli): stui plugin install --dev symlinks to ~/.stui/plugins/"
```

---

## Chunk 2 Review Checkpoint

- [ ] `stui --help` shows `plugin` subcommand.
- [ ] `stui plugin --help` shows `init, build, test, lint, install`.
- [ ] `stui plugin init testplugin && cd testplugin-provider && stui plugin build`
  produces a .wasm.
- [ ] `stui plugin lint` passes on the scaffolded template.

---

## Chunk 3 — TMDB Reference Migration

First real plugin under the new spec. Establishes the migration pattern.

### Task 3.1: Migrate `plugins/tmdb-provider/plugin.toml`

**Files:**
- Modify: `plugins/tmdb-provider/plugin.toml`

- [ ] **Step 1: Read current**

```
cat plugins/tmdb-provider/plugin.toml
```

- [ ] **Step 2: Rewrite to canonical schema**

```toml
[plugin]
name         = "tmdb"
version      = "1.0.0"
abi_version  = 1
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

- [ ] **Step 3: Commit manifest-only change**

```
git add plugins/tmdb-provider/plugin.toml
git commit -m "refactor(plugin/tmdb): manifest to canonical schema"
```

### Task 3.2: Migrate `plugins/tmdb-provider/src/lib.rs` — trait impl

**Files:**
- Modify: `plugins/tmdb-provider/src/lib.rs`

Large change: convert from `impl StuiPlugin` to `impl Plugin + impl CatalogPlugin`;
implement all six verbs; use TMDB API endpoints.

- [ ] **Step 1: Replace `use` + trait impls**

```rust
use std::collections::HashMap;

use stui_plugin_sdk::{
    Plugin as _, CatalogPlugin,
    PluginManifest, PluginEntry, PluginError, PluginResult, PluginInitError,
    SearchRequest, SearchResponse, SearchScope,
    LookupRequest, LookupResponse,
    EnrichRequest, EnrichResponse,
    ArtworkRequest, ArtworkResponse, ArtworkSize, ArtworkVariant,
    CreditsRequest, CreditsResponse, CastMember, CastRole, CrewMember, CrewRole,
    RelatedRequest, RelatedResponse, RelationKind,
    InitContext, EntryKind, stui_export_plugin, normalize_crew_role,
    error_codes, id_sources,
};
```

- [ ] **Step 2: Implement `Plugin`**

```rust
pub struct TmdbPlugin {
    manifest: PluginManifest,
    api_key: String,
}

impl Plugin for TmdbPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        // api_key is required=true; loader will have short-circuited if missing.
        self.api_key = ctx.config.get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        Ok(())
    }
}
```

- [ ] **Step 3: Implement `CatalogPlugin::search`**

Port existing `StuiPlugin::search` logic. Keep the TMDB search endpoint
handling; tweak to use the new `SearchResponse` shape and populate
`PluginEntry.external_ids["imdb"]` via `/external_ids` endpoint (see
migration actions in spec §5 TMDB block).

- [ ] **Step 4: Implement `CatalogPlugin::lookup`**

```rust
fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
    match req.id_source.as_str() {
        id_sources::TMDB => {
            // Direct TMDB id lookup: GET /3/movie/{id} or /tv/{id}
        }
        id_sources::IMDB => {
            // External-id find: GET /3/find/{imdb_id}?external_source=imdb_id
        }
        _ => return PluginResult::err(error_codes::UNSUPPORTED_SCOPE, "unsupported id_source"),
    }
    // ... build PluginEntry, wrap in LookupResponse ...
}
```

- [ ] **Step 5: Implement `enrich` / `get_artwork` / `get_credits` / `related`**

For each verb, call the relevant TMDB endpoint:
- `enrich`: search by title + year + kind; pick best match.
- `get_artwork`: GET `/3/movie/{id}/images`; build `ArtworkVariant` list
  per size.
- `get_credits`: GET `/3/movie/{id}/credits`; map to `CastMember` +
  `CrewMember` with `normalize_crew_role()`.
- `related`: GET `/3/movie/{id}/recommendations`.

- [ ] **Step 6: Add `stui_export_plugin!(TmdbPlugin);`** at the end.

- [ ] **Step 7: Build**

```
cd plugins && cargo build --target wasm32-wasip1 -p tmdb-provider
```

Expected: clean.

- [ ] **Step 8: Run unit tests**

```
cd plugins && cargo test -p tmdb-provider
```

Expected: existing tests pass (maybe need adjustments); new tests if any.

- [ ] **Step 9: Lint**

```
cd plugins/tmdb-provider && cargo run -p stui -- plugin lint
```

Expected: clean (all declared verbs implemented; no stubs).

- [ ] **Step 10: Commit**

```
git add plugins/tmdb-provider/src/
git commit -m "refactor(plugin/tmdb): migrate to Plugin + CatalogPlugin; implement all 6 verbs; populate external_ids"
```

### Task 3.3: Add TMDB unit tests with mocked host

**Files:**
- Create: `plugins/tmdb-provider/tests/basic.rs`
- Create: `plugins/tmdb-provider/tests/fixtures/*.json`

- [ ] **Step 1: Write integration tests using SDK's mocked host**

```rust
use stui_plugin_sdk::{testing::*, *};

#[test]
fn search_movie_returns_entries() {
    let host = MockHost::new()
        .with_fixture_response(
            "GET https://api.themoviedb.org/3/search/movie?api_key=fake&query=inception",
            include_str!("fixtures/search_movie_inception.json"),
        );
    let plugin = tmdb_provider::TmdbPlugin::new_for_test("fake");
    host.install();

    let resp = plugin.search(SearchRequest {
        query: "inception".into(),
        scope: SearchScope::Movie,
        page: 0, limit: 10,
        per_scope_limit: None, locale: None,
    }).expect("search ok");

    assert!(resp.items.len() > 0);
    assert_eq!(resp.items[0].kind, EntryKind::Movie);
    assert_eq!(resp.items[0].source, "tmdb");
}

#[test]
fn lookup_by_imdb_id_uses_find_endpoint() { /* ... */ }

#[test]
fn get_artwork_returns_multi_size() { /* ... */ }
```

- [ ] **Step 2: Create fixtures**

Canned TMDB responses saved from real API. Scrub for any sensitive data.

- [ ] **Step 3: Run**

```
cd plugins && cargo test -p tmdb-provider
```

- [ ] **Step 4: Commit**

```
git add plugins/tmdb-provider/tests/
git commit -m "test(plugin/tmdb): unit tests for all 6 verbs with mocked host"
```

---

## Chunk 3 Review Checkpoint

- [ ] TMDB builds clean on wasm32-wasip1.
- [ ] TMDB passes lint.
- [ ] TMDB tests pass.
- [ ] Dev-install: `stui plugin install --dev` from TMDB dir; runtime
  discovers + loads TMDB to `Loaded` state.
- [ ] Issue search IPC against TMDB via the runtime; see results.

---

## Chunk 4 — Remaining Plugin Migrations

One commit per plugin. All mirror TMDB's pattern. See spec §5 for per-plugin
details and migration actions.

### Task 4.1: Migrate `plugins/omdb-provider/`

**Files:**
- Modify: `plugins/omdb-provider/plugin.toml`, `src/lib.rs`
- Create: `plugins/omdb-provider/tests/`

- [ ] **Step 1: Rewrite manifest (see spec §5 OMDb block)**
- [ ] **Step 2: Rewrite lib.rs; impl `Plugin` + `CatalogPlugin::search + lookup` (no enrich/artwork/credits/related per spec)**
- [ ] **Step 3: Add tests**
- [ ] **Step 4: Build + lint + test**
- [ ] **Step 5: Commit** — `refactor(plugin/omdb): migrate to canonical spec`

### Task 4.2: Migrate `plugins/anilist-provider/`

- [ ] **Step 1: Rewrite manifest (kinds = ["movie","series"], rate limit 0.5 rps)**
- [ ] **Step 2: Fix pagination bug in `TRENDING_QUERY`; fix anime-type mapping (Movie scope → `type: MOVIE`; Series scope → `type: TV, ONA`)**
- [ ] **Step 3: Implement `search + lookup (id_sources=["anilist","myanimelist"])`**
- [ ] **Step 4: Implement `enrich + get_artwork + get_credits + related` as far as AniList API supports**
- [ ] **Step 5: Tests + lint**
- [ ] **Step 6: Commit** — `refactor(plugin/anilist): migrate to canonical spec + fix type mapping`

### Task 4.3: Migrate `plugins/kitsu/`

- [ ] **Step 1: Rewrite manifest**
- [ ] **Step 2: Factor bearer-token logic out of inline helper** (promote to SDK `host::BearerAuth` helper or justify-and-comment)
- [ ] **Step 3: Respect upstream `show_type` — filter Movie vs Series**
- [ ] **Step 4: Implement `search + lookup`**
- [ ] **Step 5: Tests + lint**
- [ ] **Step 6: Commit** — `refactor(plugin/kitsu): migrate to canonical spec + show_type filtering`

### Task 4.4: Migrate `plugins/discogs-provider/`

- [ ] **Step 1: Rewrite manifest — drop Track scope; kinds = ["artist","album"]**
- [ ] **Step 2: Separate genre vs format/label in response mapping**
- [ ] **Step 3: Implement `search + lookup`**
- [ ] **Step 4: Tests + lint**
- [ ] **Step 5: Commit** — `refactor(plugin/discogs): migrate + drop Track scope + separate genre/format`

### Task 4.5: Migrate `plugins/lastfm-provider/`

- [ ] **Step 1: Rewrite manifest — simplify config (drop api_secret, username, token); clarify Last.fm → Libre.fm redirection**
- [ ] **Step 2: Fix listeners-as-genre bug; move to description**
- [ ] **Step 3: Implement `search + enrich`; explicit `lookup = false`**
- [ ] **Step 4: Tests + lint**
- [ ] **Step 5: Commit** — `refactor(plugin/lastfm): migrate to canonical spec + simplify config`

---

## Chunk 4 Review Checkpoint

- [ ] All 5 migrated plugins build clean on wasm32-wasip1.
- [ ] All pass lint.
- [ ] All pass their unit tests.
- [ ] Dev-install each, verify runtime loads to `Loaded` (or `NeedsConfig`
  for ones needing API keys).

---

## Chunk 5 — MusicBrainz New Plugin

### Task 5.1: Scaffold `plugins/musicbrainz-provider/`

- [ ] **Step 1: `stui plugin init musicbrainz --dir plugins/musicbrainz-provider`**

- [ ] **Step 2: Update `plugin.toml` with real values**

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

- [ ] **Step 3: Commit scaffold** — `feat(plugin/musicbrainz): scaffold from template`

### Task 5.2: Implement `Plugin` + `CatalogPlugin` for MusicBrainz

- [ ] **Step 1: Implement MB User-Agent header hard-coded**

```rust
const USER_AGENT: &str = concat!(
    "stui-musicbrainz-provider/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Ozogorgor/stui)"
);
```

- [ ] **Step 2: Implement `search`** — MB search endpoint
  (`/ws/2/release?query=...`); map to `PluginEntry`.

- [ ] **Step 3: Implement `lookup`** — MB lookup by MBID
  (`/ws/2/release/{mbid}?inc=artist-credits+media+tags`); full detail
  in response.

- [ ] **Step 4: Implement `enrich`** — search by title + artist_name +
  album_name; confidence score from string-similarity.

- [ ] **Step 5: Implement `get_artwork`** — Cover Art Archive
  (`https://coverartarchive.org/release/{mbid}`). Return multiple sizes.

- [ ] **Step 6: Implement `get_credits`** — MB release → recording →
  artist-credit + work-relationships. Map to `CastMember` (vocalists,
  performers) + `CrewMember` (Composer, Songwriter, Producer,
  Instrumentalist).

- [ ] **Step 7: `related` stays stubbed.**

  The `related = { stub = true, reason = "..." }` in the manifest is
  legitimate mid-development state. `stui plugin lint` emits a warning
  but passes. `stui plugin build --release` would reject — but this
  plugin ships bundled (installed from the workspace, not built with
  `--release`); the `--release` gate only applies to external plugins
  prepping for the Tier-3 inspirational registry. Bundled plugins can
  ship with declared stubs as long as the architecture doesn't
  regress.

- [ ] **Step 8: Build, lint, test**

```
cd plugins/musicbrainz-provider
cargo build --target wasm32-wasip1
cargo run -p stui -- plugin lint
cargo test
```

- [ ] **Step 9: Add to `plugins/Cargo.toml` workspace members**

- [ ] **Step 10: Commit** — `feat(plugin/musicbrainz): full implementation (search + lookup + enrich + artwork + credits)`

### Task 5.3: MusicBrainz integration tests

- [ ] **Step 1: Write fixture-based tests using stored MB responses**
- [ ] **Step 2: Run, iterate**
- [ ] **Step 3: Commit** — `test(plugin/musicbrainz): integration tests with fixtures`

---

## Chunk 5 Review Checkpoint

- [ ] MusicBrainz builds + lints + tests pass.
- [ ] Dev-install + runtime-load succeeds to `Loaded`.
- [ ] Search MB for known artist; lookup by MBID returns full detail.
- [ ] Tag normalization pipeline (memory `project_stui_roadmap.md`)
  can be unblocked: its `normalize::lookup::fetch_batch()` can now call
  through the MB plugin.

---

## Chunk 6 — Cleanup

### Task 6.1: Delete dropped + leftover plugins

**Files to delete:**
- `plugins/imdb-provider/`
- `plugins/javdb/`
- `plugins/r18/`
- `plugins/listenbrainz-provider/`
- `plugins/subscene/`
- `plugins/kitsunekko/`
- `plugins/yify-subs/`
- `plugins/torrentio-rpc/`

- [ ] **Step 1: Sanity check each directory is git-tracked**

```
for d in imdb-provider javdb r18 listenbrainz-provider subscene kitsunekko yify-subs torrentio-rpc; do
    if [ -z "$(git ls-files plugins/$d 2>/dev/null | head -1)" ]; then
        echo "WARN: plugins/$d is not git-tracked; use 'rm -r' instead of 'git rm'"
    fi
done
```

- [ ] **Step 2: Remove directories (use `git rm` for tracked, `rm -r` for untracked)**

```
git rm -r plugins/imdb-provider plugins/javdb plugins/r18 plugins/listenbrainz-provider
git rm -r plugins/subscene plugins/kitsunekko plugins/yify-subs plugins/torrentio-rpc
```

- [ ] **Step 2: Verify**

```
ls plugins/
```

Expected: `tmdb-provider`, `omdb-provider`, `anilist-provider`, `kitsu`,
`discogs-provider`, `lastfm-provider`, `musicbrainz-provider`,
`Cargo.toml`. 7 dirs + Cargo.toml + maybe target/.

- [ ] **Step 3: Commit**

```
git commit -m "chore(plugins): delete dropped + leftover plugins (non-metadata already in stui_plugins)"
```

### Task 6.2: Update `plugins/Cargo.toml` workspace members

**Files:**
- Modify: `plugins/Cargo.toml`

- [ ] **Step 1: Update members list**

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

- [ ] **Step 2: Build full plugin workspace**

```
cd plugins && cargo build --target wasm32-wasip1 --workspace
```

Expected: 7 plugins build clean.

- [ ] **Step 3: Commit**

```
git add plugins/Cargo.toml
git commit -m "chore(plugins): update workspace members to 7 metadata plugins"
```

---

## Chunk 7 — Integration + Verification

### Task 7.1: Runtime smoke — all 7 plugins load to `Loaded` (or `NeedsConfig`)

- [ ] **Step 1: Symlink all 7 into `~/.stui/plugins/`**

```
for p in tmdb omdb anilist kitsu discogs lastfm musicbrainz; do
    dir=$(find plugins -maxdepth 1 -type d -name "*$p*")
    cargo run -p stui -- plugin install --dev $dir
done
```

- [ ] **Step 2: Start runtime**

```
cargo run -p stui-runtime
```

- [ ] **Step 3: Query plugin list via IPC**

  Confirm the IPC transport (`grep -n "ipc::serve\|listen" runtime/src/main.rs`)
  — current runtime uses a Unix-domain socket at
  `$XDG_RUNTIME_DIR/stui/runtime.sock` (or per config). Use `netcat`
  or a small helper script to send a JSON-RPC frame:
  `{"type":"list_plugins"}` and pretty-print the response.
  
  Expected response: an array of 7 `PluginInfo` objects, each with
  `name`, `version`, `status` (one of `loaded` / `needs_config` /
  `failed` / `disabled`), and per-plugin capability list.

Expected: 7 plugins; each `Loaded` (if env vars / config present) or
`NeedsConfig { missing: ["api_key"], hint: "..." }` for keyed plugins.

- [ ] **Step 4: Document the smoke results in an integration test**

Create `runtime/tests/plugin_integration_smoke.rs` — programmatically
starts the runtime, queries plugin list, asserts statuses.

- [ ] **Step 5: Commit**

```
git add runtime/tests/plugin_integration_smoke.rs
git commit -m "test(runtime): integration smoke — all 7 plugins load to Loaded or NeedsConfig"
```

### Task 7.2: Dispatcher smoke — each verb routes correctly

- [ ] **Step 1: Test scoped search** — via IPC, issue
  `Request::Search { scope: Movie, query: "matrix" }`. Verify TMDB
  + OMDb both get called (dispatched by scope).

- [ ] **Step 2: Test lookup** — issue
  `Request::Lookup { id: "tt0133093", id_source: "imdb", kind: Movie }`.
  Verify only TMDB (declares `lookup.id_sources = ["tmdb", "imdb"]`)
  + OMDb (declares `["imdb"]`) get called; parallel fan-out;
  first-success wins.

- [ ] **Step 3: Test get_credits** — `kind: Movie` → TMDB gets called
  (it's the only plugin with credits for movies).

- [ ] **Step 4: Encode as integration tests**

- [ ] **Step 5: Commit** — `test(runtime): dispatcher integration — verb routing verified per id_source + kind`

### Task 7.3: Rate-limit smoke

- [ ] **Step 1: Integration test that saturates TMDB's token bucket**

Deterministic timing via `tokio::time::pause()`:

```rust
#[tokio::test(start_paused = true)]
async fn tmdb_rate_limited_after_burst() {
    // TMDB: requests_per_second = 4, burst = 10.
    // Fire 15 calls before any clock advance; bucket starts full at 10.
    let harness = TestHarness::new_with_plugin("tmdb").await;

    // First 10 succeed (burst capacity).
    for i in 0..10 {
        let r = harness.supervisor_search("tmdb", dummy_request(i)).await;
        assert!(r.is_ok(), "call {} should succeed (within burst)", i);
    }

    // 11th fails with rate_limited.
    let r = harness.supervisor_search("tmdb", dummy_request(11)).await;
    match r {
        Err(PluginCallError::PluginError(err)) => {
            assert_eq!(err.code, "rate_limited");
            assert!(err.retry_after_ms.is_some());
        }
        _ => panic!("expected rate_limited, got {:?}", r),
    }

    // After advancing 250ms (one refill at 4 rps), one more slot.
    tokio::time::advance(Duration::from_millis(250)).await;
    let r = harness.supervisor_search("tmdb", dummy_request(12)).await;
    assert!(r.is_ok(), "call after refill should succeed");
}
```

- [ ] **Step 2: Run**

```
cargo test --lib rate_limit
```

- [ ] **Step 3: Commit** — `test(runtime): rate-limit smoke with paused clock + deterministic token-bucket behavior`

### Task 7.4: Status lifecycle smoke (NeedsConfig → Loaded)

- [ ] **Step 1: Install TMDB without API key**
- [ ] **Step 2: Verify it shows `NeedsConfig { missing: ["api_key"] }`**
- [ ] **Step 3: Supply API key via TUI settings** (or directly write to
  `~/.stui/config/plugins/tmdb.toml`)
- [ ] **Step 4: Verify runtime re-initializes and plugin transitions to `Loaded`**
- [ ] **Step 5: Encode as integration test** (may need mock TUI config
  write-through)

### Task 7.5: `--release` gate smoke

- [ ] **Step 1: In a test plugin, add `search = { stub = true, reason = "..." }`**
- [ ] **Step 2: `stui plugin build --release` — expect failure**
- [ ] **Step 3: Remove stub; re-run — expect success**

### Task 7.6: Documentation pass

- [ ] **Step 1: Update `plugins/README.md`** (create if missing) listing
  the 7 bundled plugins + their capabilities + links to stui_plugins for
  others.

- [ ] **Step 2: Update SDK docs** (doc comments on `Plugin`,
  `CatalogPlugin`, each verb request/response).

- [ ] **Step 3: Migration notes in CHANGELOG.md**

- [ ] **Step 4: Commit**

```
git add plugins/README.md sdk/src/ CHANGELOG.md
git commit -m "docs: plugin refactor migration + API notes"
```

---

## Chunk 7 Review Checkpoint

- [ ] All 7 plugins load to `Loaded` (with API keys) or `NeedsConfig`
  (without).
- [ ] Scoped search routes correctly across plugins.
- [ ] Lookup / artwork / credits route by id_source + kind.
- [ ] Rate-limit enforced via `stui_http_get`.
- [ ] `NeedsConfig` → `Loaded` flow works via config update.
- [ ] `--release` gate rejects stubs.
- [ ] `cargo build --workspace` clean.
- [ ] All new tests pass.
- [ ] Documentation updated.

---

## Execution Notes

- **Chunk boundaries are checkpoints.** Review per chunk.
- **Order matters.** Chunks 1 → 2 independent; 3 depends on both;
  4 depends on 3; 5 depends on 3; 6 deletes after Chunks 3-5 land;
  7 verifies everything.
- **Plugin migrations in Chunk 4 are parallel-safe** (each plugin is
  isolated). Can land as separate PRs if you want distinct review per
  plugin, or one merged PR with 5 commits.
- **Cross-repo coordination:** `stui_plugins/` plugins (non-metadata)
  may need a parallel manifest migration if the strict loader rejects
  their legacy manifests. See spec §7 Chunk 1 cross-repo note.
- **No auto-commit.** Per project convention, commit cadence is your
  call. Each `git commit` step is authorial intent, not automatic.
- **Plugin signing, registry, publishing** — not in scope; tracked on
  BACKLOG Tier 3.

---

## Testing Philosophy

- **Per-plugin unit tests** — in-tree, use SDK's `testing::MockHost`.
  Fast; no network.
- **Plugin integration tests (via `stui plugin test`)** — fixture-driven.
- **Runtime integration tests** — load plugins from filesystem; verify
  state + dispatcher + rate-limit + config-lifecycle.
- **TUI teatest coverage** — deferred; TUI is unchanged for this refactor.
- **Manual end-to-end smoke** — run stui, browse, verify each plugin's
  results look right in the actual UI. Do before tagging a release.

---

## Risks + Mitigations

| Risk | Mitigation |
|---|---|
| Chunk 1 leaves bundled plugins non-building | Chunks 3-5 restore quickly; Chunk 1+3 collapsible if mid-chunk risk too high |
| `stui_plugins/` plugins fail to load once strict loader ships | Cross-repo PR migrates them in lockstep |
| MusicBrainz User-Agent rejected if UA format off | Hard-coded format matches MB's docs; verify with test fixture |
| Rate limit test timing-flaky | Use `tokio::time::pause` + `advance` deterministic pattern (search refactor precedent) |
| Fixture drift from real APIs | Fixtures dated; periodic refresh as part of plugin maintenance |
