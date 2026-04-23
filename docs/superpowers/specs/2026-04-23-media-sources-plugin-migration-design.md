# Media-sources plugin SDK migration — design

**Date:** 2026-04-23
**Status:** Design approved. Ready for implementation planning.

## Goal

Migrate the external (non-bundled) media-source plugins in `stui_plugins` to the current SDK so they compile, package, publish via the release workflow, and install cleanly through Plugin Manager. Two torrent plugins verified end-to-end (search → resolve → playback); one subtitle plugin verified through install (runtime dispatch is a separate follow-up).

## Scope

| Plugin | Category | Verification depth |
|---|---|---|
| `jackett-provider` | torrent search | End-to-end: search + resolve + smoke play |
| `prowlarr-provider` | torrent search | End-to-end: search + resolve + smoke play |
| `opensubtitles-provider` | subtitles | Compile + install + visible in Installed tab. Runtime dispatch deferred. |
| `torrentio-rpc` | torrent (Python) | Unchanged — not SDK-dependent |
| `kitsunekko`, `subscene`, `yify-subs` | subtitles | Manifest cleanup only (so they build); full migration deferred to a later fleet pass |

### Out of scope

- Runtime subtitle dispatch pipeline (`find_subtitle_providers` currently has zero callers). User-confirmed this is the immediate next-session follow-up, not part of this spec.
- Full migration of non-canary subtitle plugins. Manifests get triage so they continue building; source stays on the legacy `StuiPlugin` trait.
- Removing the deprecated `StuiPlugin` trait from the SDK. Every torrent/subtitle plugin still needs it for the `stui_resolve` ABI.

## Architecture

Three layers change, in order.

### Layer 1 — plugin manifests

New manifests drop the deprecated `[plugin] type = "..."` field (the validator rejects it) and declare capabilities explicitly.

Torrent plugin manifest shape:

```toml
[plugin]
id          = "jackett"
name        = "jackett-provider"
version     = "0.1.0"
abi_version = 1
description = "Search torrents via a local Jackett instance"

[capabilities]
streams = true

[capabilities.catalog]
kinds  = ["movie", "series"]
search = true

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

**Why this shape and not `[capabilities.streams]` as a subtable:** the SDK's `Capabilities` struct types `streams` as `bool` (`sdk/src/manifest.rs`). An empty TOML subtable would deserialize as `{}`, fail bool parsing, and `parse_manifest(include_str!(...))` would panic at compile time. The flat `streams = true` inside `[capabilities]` is the correct form.

Subtitle plugin manifest shape:

```toml
[plugin]
id          = "opensubtitles"
name        = "opensubtitles-provider"
version     = "0.1.0"
abi_version = 1
description = "Subtitle search and download via OpenSubtitles REST API"

[capabilities.subtitles]

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

**Why `[capabilities.subtitles]` is fine even though it isn't a typed field:** the `Capabilities` struct has a `#[serde(flatten)] _extra: HashMap` catch-all that absorbs unknown keys as forward-compat placeholders. Subtitles lands there until runtime dispatch earns it a typed field. The validator doesn't reject it; the runtime's `find_subtitle_providers` already routes on `PluginCapability::Subtitles` (populated via the legacy `[plugin] type` mapping — which we're removing — so subtitles dispatch genuinely requires the follow-up session to land before this plugin is reachable).

Existing `[env]`, `[config]`, `[permissions]`, `[rate_limit]` sections stay untouched.

### Layer 2 — plugin source (trait migration, Approach B)

Every canary plugin implements three traits; real search code lives in `CatalogPlugin::search`, resolve stays on the legacy trait.

**Struct:** carry a parsed manifest.

```rust
pub struct JackettProvider {
    manifest: PluginManifest,
}

impl Default for JackettProvider {
    fn default() -> Self {
        Self {
            manifest: parse_manifest(include_str!("../plugin.toml"))
                .expect("plugin.toml failed to parse at compile time"),
        }
    }
}
```

**`Plugin`:**

```rust
impl Plugin for JackettProvider {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
    // init/shutdown use defaults unless config validation is needed
}
```

**`CatalogPlugin::search`** — real work. Migration of existing body:
- `req.tab.as_str()` → map from `req.scope: SearchScope` enum. `SearchScope::Movie` → `"movies"`, `SearchScope::Series` → `"series"`, `SearchScope::Track | Artist | Album` → `"music"`.
- `rating: quality` (`Option<String>`) → `rating: None`, `description: Some(format!("{quality} · {meta}"))`. Quality never was a numeric rating; goes into description.
- Add `kind: EntryKind::Movie` (or appropriate for scope).
- Fill new PluginEntry fields (`artist_name`, `album_name`, `track_number`, `season`, `episode`, `original_language`) with `..Default::default()`.

**`StuiPlugin::search`** — required-by-trait stub, never dispatched:

```rust
fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
    PluginResult::err("LEGACY_UNUSED", "search dispatches via CatalogPlugin")
}
```

**`StuiPlugin::resolve`** — verbatim; still the path for the `stui_resolve` ABI.

**`StuiPlugin::{name, version, plugin_type}`** — trait requirements, never dispatched. `name` and `version` read from `self.manifest.plugin` (`&self.manifest.plugin.name`, `&self.manifest.plugin.version`). `plugin_type` is hard-coded per canary since the manifest's `plugin_type: Option<String>` is the deprecated field we're removing:
- `jackett-provider`, `prowlarr-provider` → `PluginType::Provider` (matches existing legacy `StuiPlugin::plugin_type` returns).
- `opensubtitles-provider` → `PluginType::Subtitle`.

**Export macro:** `stui_export_plugin!(JackettProvider);` unchanged. It dispatches `stui_search` → `CatalogPlugin::search` and `stui_resolve` → `StuiPlugin::resolve` already.

### Layer 3 — CI matrix

Add canaries to `stui_plugins/.github/workflows/release.yml`:

```yaml
matrix:
  plugin:
    - example-provider
    - example-resolver
    - jackett-provider        # new
    - prowlarr-provider       # new
    - opensubtitles-provider  # new
```

Non-canary subtitle plugins (`kitsunekko`, `subscene`, `yify-subs`) get manifest cleanup but are NOT added to the CI matrix yet — their source hasn't been migrated, so they'd fail `cargo build`. "Continue building" in their context refers only to the workspace staying parseable by `cargo metadata --no-deps` and any member-specific `cargo check` not being broken by the shared `sdk/` changes. They do not produce CI artifacts this pass.

## Verification plan

### Build gate (local)

- `cargo build -p <canary> --target wasm32-wasip1 --release` exits 0 for each canary.
- Workspace builds as a whole.

### Manifest gate (local)

- `parse_manifest(include_str!(...))` at compile time succeeds (the panic fires at compile evaluation).
- Host-side unit test in each canary: `parse_manifest(...)` then `validate()` both succeed.

### CI gate

- Push tag `v0.2.0` on `stui_plugins`. Matrix builds all entries. `plugins.json` published to `ozogorgor.github.io/stui_plugins` includes the 3 new entries with valid `binary_url` + `checksum`.

### Install gate (stui side)

- Plugin Manager → Available shows all 3 canaries (or their non-installed subset).
- Install succeeds for each; hot-reload watcher picks them up; Installed tab shows them with `status = "loaded"`.

### Smoke gate — torrent canaries only

- User supplies `JACKETT_URL` + `JACKETT_API_KEY` (or prowlarr equivalent) via plugin config.
- Play a searchable title; stui fans out to jackett/prowlarr via `stui_search`; entries appear in the stream ranker; resolve produces magnet URL; aria2 queues download.

### Smoke gate — opensubtitles

- Plugin visible in Installed tab, `enabled = true`.
- No search attempt happens — runtime subtitle dispatch unimplemented. This is expected.

### Regression check

- Existing `example-provider`, `example-resolver`, `torrentio-rpc` still build and publish.
- Bundled plugins still load on runtime startup (unaffected by `stui_plugins` changes).

## Risks

1. **Prowlarr source shape.** Spot-checked during review: prowlarr mirrors jackett's structure (same `StuiPlugin` trait, same `req.tab` branching in search, same packed-ID → magnet pattern in resolve). Template applies directly. Risk resolved — no scope expansion expected.

2. **Torrent category flag choice.** Torrent search for movies + series is the primary use. Music torrents are noisier. Starting with `kinds = ["movie", "series"]` avoids polluting the Music tab with torrent clutter. Adding `"track"` / `"album"` is a one-line follow-up if wanted.

3. **OpenSubtitles compile-only is shallow.** Even passing all gates, first real runtime use (next session) might surface issues. Mitigation: keep the migration additive — minimal source changes — so the follow-up dispatches against code close to what was working before.

4. **SDK drift between `stui/sdk` and `stui_plugins/sdk`.** This migration is the first large test of the manual-sync policy. Mitigation: use `cargo build` as consistency check; mismatches surface as compile errors.

## Testing

- **Unit:** 3 new `parse_manifest + validate` tests, one per canary, host-side.
- **Integration:** none automated — stui is a TUI + WASM + network-dependent stack. Smoke tests are manual post-install.

## Deferrals (explicit)

- Runtime subtitle dispatch pipeline (immediate next session).
- Full migration of `kitsunekko` / `subscene` / `yify-subs` (later fleet pass).
- SDK removal of the deprecated `StuiPlugin` trait (long-horizon — requires dedicated stream/subtitle ABIs first).
