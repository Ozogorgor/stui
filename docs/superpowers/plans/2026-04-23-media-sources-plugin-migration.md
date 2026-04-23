# Media-sources plugin SDK migration — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `jackett-provider`, `prowlarr-provider`, and `opensubtitles-provider` from the deprecated `StuiPlugin` trait to the current `Plugin + CatalogPlugin` traits so they compile, package, publish via the release workflow, install cleanly in stui, and (torrent canaries only) resolve into playable streams end-to-end.

**Architecture:** Approach B from the design spec — real search logic lives in `CatalogPlugin::search`; `StuiPlugin::search` becomes a never-dispatched stub; `StuiPlugin::resolve` stays verbatim because it's still the path the `stui_resolve` ABI calls. Manifests drop the deprecated `[plugin] type = "..."` field and declare capabilities explicitly.

**Tech Stack:** Rust, `stui-plugin-sdk` (path dep from `stui_plugins/sdk/`), `wasm32-wasip1` target, `stui_export_plugin!` macro, GitHub Actions release workflow.

**Design spec:** [2026-04-23-media-sources-plugin-migration-design.md](../specs/2026-04-23-media-sources-plugin-migration-design.md)

---

## Commit policy for this plan

The user has explicitly asked that no commits land until all fundamental plugin work (this plan plus any immediate follow-up in the session) is complete. The standard "commit after every task" step from the writing-plans skill is deliberately absent. Instead, all commits are consolidated in **Chunk 6 Task 6.4** at the end of the plan. Do NOT insert ad-hoc commits.

---

## File structure

### Files modified in `stui_plugins/`

| File | Responsibility | Chunk |
|---|---|---|
| `jackett-provider/plugin.toml` | New manifest: drop `type`, add `[capabilities]` + `streams = true` + `[capabilities.catalog]` + `[meta]` | 1 |
| `jackett-provider/src/lib.rs` | Approach B trait migration | 2 |
| `prowlarr-provider/plugin.toml` | Same shape as jackett's | 1 |
| `prowlarr-provider/src/lib.rs` | Approach B trait migration | 3 |
| `opensubtitles-provider/plugin.toml` | Drop `type`, add `[capabilities.subtitles]` (opaque) + `[meta]` | 1 |
| `opensubtitles-provider/src/lib.rs` | Approach B trait migration | 4 |
| `kitsunekko/plugin.toml` | Triage: drop `type`, add `[capabilities.subtitles]` + `[meta]`. Source untouched. | 1 |
| `subscene/plugin.toml` | Same triage as kitsunekko | 1 |
| `yify-subs/plugin.toml` | Same triage as kitsunekko | 1 |
| `.github/workflows/release.yml` | Extend `plugin` matrix with 3 canaries | 5 |

### Files NOT modified

- `torrentio-rpc/*` — Python plugin, no SDK migration needed.
- `sdk/*` — the shared SDK is unchanged; we're migrating plugins onto its existing shape.
- `example-provider/*`, `example-resolver/*` — already on the current SDK and published.

---

## Chunk 1: Manifest triage (all 6 plugins)

**Goal:** Every external plugin's `plugin.toml` parses cleanly under the current SDK validator. Removes the deprecated `[plugin] type = "..."` field, declares capabilities explicitly, moves metadata fields to a `[meta]` section.

**Why this chunk is first:** Source migration (chunks 2-4) uses `parse_manifest(include_str!("../plugin.toml"))` inside `Default::default()`. That panics at compile time if the manifest doesn't parse. Doing manifests first means every subsequent compile is a real validation of the manifest shape.

### SDK types reference (for this and all later chunks)

Do NOT assume `use stui_plugin_sdk::prelude::*` imports these — the prelude only exports the search/resolve surface (`PluginEntry`, `SearchRequest`, `ResolveRequest`, etc.). You need explicit imports for:

```rust
use stui_plugin_sdk::{
    parse_manifest, PluginManifest,
    Plugin, CatalogPlugin,
    EntryKind, SearchScope,
};
```

`CatalogCapability` is an untagged `enum` (not a struct) with variants:
- `Typed { kinds: Vec<EntryKind>, search, lookup, enrich, artwork, credits, related }` — the new `[capabilities.catalog]` shape.
- `Enabled(bool)` — legacy `catalog = true`.

Field access is NOT direct (`m.capabilities.catalog.kinds` does not compile). Use the accessor:
```rust
m.capabilities.catalog.kinds() // -> &[EntryKind]
```

`EntryKind` is `#[serde(rename_all = "snake_case")]` so TOML strings like `"movie"`, `"series"` deserialize to `EntryKind::Movie`, `EntryKind::Series`.

### Task 1.1: Replace `jackett-provider/plugin.toml`

**Files:**
- Modify: `stui_plugins/jackett-provider/plugin.toml`

- [ ] **Step 1: Read the current file to confirm shape**

Run:
```bash
cat stui_plugins/jackett-provider/plugin.toml
```

Expected: top `[plugin]` block has `type = "stream-provider"` + `tags = [...]`. `[env]`, `[config]`, `[permissions]`, `[search]` sections follow. No `[meta]` section. No `[capabilities]` block.

- [ ] **Step 2: Rewrite the file**

Overwrite `stui_plugins/jackett-provider/plugin.toml` with:

```toml
# jackett-provider — plugin manifest
# Install at: ~/.stui/plugins/jackett-provider/plugin.toml

[plugin]
id          = "jackett"
name        = "jackett-provider"
version     = "0.1.0"
abi_version = 1
description = "Search torrents via a local Jackett instance"
tags        = ["streams", "movies", "tv", "anime", "music"]

[capabilities]
streams = true

[capabilities.catalog]
kinds  = ["movie", "series"]
search = true

[env]
JACKETT_URL     = "http://localhost:9117"
JACKETT_API_KEY = ""

[[config]]
key       = "url"
label     = "Jackett URL"
hint      = "Base URL of your Jackett instance"
masked    = false
required  = true
default   = "http://localhost:9117"

[[config]]
key       = "api_key"
label     = "Jackett API Key"
hint      = "Jackett → Dashboard → API Key"
masked    = true
required  = true

[permissions]
# Jackett is typically on localhost — no external internet permission needed.
# If your Jackett instance is remote, add its hostname here.
network = ["localhost", "127.0.0.1", "::1"]

[search]
movie_categories  = "2000,2010,2020"
series_categories = "5000,5040,5070"
music_categories  = "3000,3010,3040"

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

Key changes:
- Remove `type = "stream-provider"` (deprecated; validator rejects).
- Add top-level `[capabilities]` with `streams = true` (flat bool, NOT a subtable — see spec Layer 1).
- Add `[capabilities.catalog]` with `kinds = ["movie", "series"]` (NOT `["music"]` — prevents torrent clutter in the Music tab per spec risk #2).
- Add `id = "jackett"` (required by new validator).
- Move `author`/`license`/`homepage` into a new `[meta]` block.

- [ ] **Step 3: Validate parse**

Write a throwaway test file `stui_plugins/jackett-provider/tests/manifest_parses.rs`:

```rust
use stui_plugin_sdk::{parse_manifest, EntryKind};

#[test]
fn plugin_toml_parses() {
    let m = parse_manifest(include_str!("../plugin.toml"))
        .expect("plugin.toml parses");
    assert_eq!(m.plugin.name, "jackett-provider");
    assert!(m.capabilities.streams, "streams capability missing");
    assert_eq!(
        m.capabilities.catalog.kinds(),
        &[EntryKind::Movie, EntryKind::Series],
    );
}
```

Note: `.kinds()` is an accessor method on the `CatalogCapability` enum, not a struct field. Don't write `.kinds` (no parens) — that won't compile.

Run:
```bash
cd stui_plugins && cargo test -p jackett-provider --test manifest_parses 2>&1 | tail -10
```

Expected: `test plugin_toml_parses ... ok` in the output, exit 0.

If the test is the first to reference `parse_manifest`, confirm it's `pub` in the SDK at `stui_plugins/sdk/src/lib.rs:68`. Already is.

Do NOT delete this test file — it stays as a regression guard.

### Task 1.2: Replace `prowlarr-provider/plugin.toml`

**Files:**
- Modify: `stui_plugins/prowlarr-provider/plugin.toml`

- [ ] **Step 1: Read the current file**

```bash
cat stui_plugins/prowlarr-provider/plugin.toml
```

- [ ] **Step 2: Rewrite the file**

Overwrite with (identical structure to jackett, different id/name/description/env vars):

```toml
# prowlarr-provider — plugin manifest

[plugin]
id          = "prowlarr"
name        = "prowlarr-provider"
version     = "0.1.0"
abi_version = 1
description = "Search torrents via a Prowlarr indexer"
tags        = ["streams", "movies", "tv", "anime", "music"]

[capabilities]
streams = true

[capabilities.catalog]
kinds  = ["movie", "series"]
search = true

[env]
PROWLARR_URL     = "http://localhost:9696"
PROWLARR_API_KEY = ""

[[config]]
key       = "url"
label     = "Prowlarr URL"
hint      = "Base URL of your Prowlarr instance"
masked    = false
required  = true
default   = "http://localhost:9696"

[[config]]
key       = "api_key"
label     = "Prowlarr API Key"
hint      = "Prowlarr → Settings → General → API Key"
masked    = true
required  = true

[permissions]
network = ["localhost", "127.0.0.1", "::1"]

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

Preserve the existing [env] / [config] / [permissions] values from prowlarr's current plugin.toml — only the `type` removal, `[capabilities]`, and `[meta]` section additions are the migration. If prowlarr's current manifest has a `[search]` section with category IDs, keep it verbatim.

- [ ] **Step 3: Validate parse**

Create `stui_plugins/prowlarr-provider/tests/manifest_parses.rs` (same pattern as jackett, assert `m.plugin.name == "prowlarr-provider"`).

Run:
```bash
cd stui_plugins && cargo test -p prowlarr-provider --test manifest_parses 2>&1 | tail -10
```

Expected: pass.

### Task 1.3: Replace `opensubtitles-provider/plugin.toml`

**Files:**
- Modify: `stui_plugins/opensubtitles-provider/plugin.toml`

- [ ] **Step 1: Read current file**

```bash
cat stui_plugins/opensubtitles-provider/plugin.toml
```

- [ ] **Step 2: Rewrite**

```toml
# opensubtitles-provider — plugin manifest

[plugin]
id          = "opensubtitles"
name        = "opensubtitles-provider"
version     = "0.1.0"
abi_version = 1
description = "Subtitle search and download via OpenSubtitles REST API v1"
tags        = ["subtitles"]

[capabilities.subtitles]
# Opaque capability — flattens into the SDK Capabilities struct's _extra
# HashMap. Stays compatible with the validator; runtime dispatch for
# subtitles is a separate follow-up (see design spec deferrals).

[env]
OS_API_KEY  = ""
OS_USERNAME = ""
OS_PASSWORD = ""
OS_LANGUAGE = "en"

[[config]]
key       = "api_key"
label     = "OpenSubtitles API Key"
hint      = "Create one at https://www.opensubtitles.com/en/consumers"
masked    = true
required  = true

[[config]]
key       = "username"
label     = "OpenSubtitles Username"
hint      = "Optional — unlocks >5 downloads/day"
required  = false

[[config]]
key       = "password"
label     = "OpenSubtitles Password"
hint      = "Optional — paired with username for auth"
masked    = true
required  = false

[[config]]
key       = "language"
label     = "Preferred Subtitle Languages"
hint      = "Comma-separated BCP-47 codes, e.g. en,es,fr"
required  = false
default   = "en"

[permissions]
network = ["api.opensubtitles.com", "vip-api.opensubtitles.com", "dl.opensubtitles.com"]

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

If the existing manifest has additional config rows (e.g. cache settings), preserve them.

- [ ] **Step 3: Validate parse**

Create `stui_plugins/opensubtitles-provider/tests/manifest_parses.rs`:

```rust
use stui_plugin_sdk::parse_manifest;

#[test]
fn plugin_toml_parses() {
    let m = parse_manifest(include_str!("../plugin.toml"))
        .expect("plugin.toml parses");
    assert_eq!(m.plugin.name, "opensubtitles-provider");
    // subtitles capability is intentionally opaque — no typed field assertion.
    // Just confirm the manifest parses and exposes no typed streams/catalog.
    assert!(!m.capabilities.streams);
    // `.kinds()` accessor method, NOT `.kinds` field — CatalogCapability
    // is an untagged enum. Empty-kinds is consistent with "subtitles only"
    // — opensubtitles declares no catalog block at all.
    assert!(m.capabilities.catalog.kinds().is_empty());
}
```

Run:
```bash
cd stui_plugins && cargo test -p opensubtitles-provider --test manifest_parses 2>&1 | tail -10
```

Expected: pass.

### Task 1.4: Triage `kitsunekko`, `subscene`, `yify-subs` manifests

**Files:**
- Modify: `stui_plugins/kitsunekko/plugin.toml`
- Modify: `stui_plugins/subscene/plugin.toml`
- Modify: `stui_plugins/yify-subs/plugin.toml`

These three plugins are NOT in the canary scope and NOT in the CI matrix. Their manifests still need to parse under the current SDK so that `cargo metadata --no-deps` on the workspace doesn't trip on a sibling. Source stays on the legacy trait — we don't touch `src/lib.rs`.

- [ ] **Step 1: For each of the three, apply the same pattern**

For each `<plugin>` in {`kitsunekko`, `subscene`, `yify-subs`}:

1. Read `stui_plugins/<plugin>/plugin.toml`.
2. Remove the line `type = "subtitle-provider"` (or whatever legacy value is there).
3. Add (if absent) `[capabilities.subtitles]` immediately after `[plugin]`.
4. If fields `author`/`license`/`homepage` are under `[plugin]`, move them into a `[meta]` section at the end of the file.

Example for `kitsunekko`:

```toml
[plugin]
id          = "kitsunekko"
name        = "kitsunekko"
version     = "0.1.0"
abi_version = 1
description = "Anime subtitle scraper for kitsunekko.net"
tags        = ["subtitles", "anime"]

[capabilities.subtitles]

# ... existing [env], [permissions], etc. unchanged ...

[meta]
author   = "stui"
license  = "MIT"
homepage = "https://github.com/Ozogorgor/stui_plugins"
```

- [ ] **Step 2: Verify workspace still resolves**

```bash
cd stui_plugins && cargo metadata --no-deps --format-version 1 > /dev/null
```

Expected: exit 0. A manifest error on any member would fail this.

---

## Chunk 2: jackett-provider source migration (canary 1)

**Goal:** `jackett-provider` implements `Plugin + CatalogPlugin` (new traits) with real search code in `CatalogPlugin::search`, stub `StuiPlugin::search`, verbatim `StuiPlugin::resolve`. Compiles to `wasm32-wasip1` and exports all 7 `stui_*` ABI symbols via `stui_export_plugin!`.

### Task 2.1: Migrate `jackett-provider/src/lib.rs`

**Files:**
- Modify: `stui_plugins/jackett-provider/src/lib.rs`

- [ ] **Step 1: Add explicit SDK imports**

The existing file has `use stui_plugin_sdk::prelude::*;` which does NOT re-export `parse_manifest`, `PluginManifest`, `Plugin`, `CatalogPlugin`, `EntryKind`, or `SearchScope`. Add them explicitly. Right below the existing prelude import line, add:

```rust
use stui_plugin_sdk::{
    parse_manifest, PluginManifest,
    Plugin, CatalogPlugin,
    EntryKind, SearchScope,
};
```

- [ ] **Step 2: Replace the plugin struct**

Find the existing:

```rust
#[derive(Default)]
pub struct JackettProvider;
```

Replace with:

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

- [ ] **Step 3: Add `Plugin` impl before the existing `impl StuiPlugin for JackettProvider`**

Insert:

```rust
impl Plugin for JackettProvider {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
    // init/shutdown use default no-op impls from the trait
}
```

- [ ] **Step 4: Rewrite the `search` logic as `CatalogPlugin::search`**

Replace the entire `impl StuiPlugin for JackettProvider { ... }` block with two separate impls. The `CatalogPlugin::search` implementation holds the real search code. `StuiPlugin` keeps only the ABI-required stubs + the verbatim `resolve`.

Replace:

```rust
impl StuiPlugin for JackettProvider {
    fn name(&self) -> &str { "jackett-provider" }
    fn version(&self) -> &str { "0.1.0" }
    fn plugin_type(&self) -> PluginType { PluginType::Provider }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // ... existing search body that uses req.tab ...
    }

    fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse> {
        // ... existing resolve body ...
    }
}
```

With:

```rust
impl CatalogPlugin for JackettProvider {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let cfg = match Config::load() {
            Ok(c) => c,
            Err(e) => return PluginResult::err("CONFIG_ERROR", &e),
        };

        // Map the new SearchScope enum back to the Newznab category strings
        // Jackett's API expects. Music scopes fan out to Jackett's
        // 3000-range categories even though our manifest advertises only
        // movie/series kinds — if the runtime ever dispatches a music
        // scope to this plugin (e.g. manual test, future music support),
        // do the right thing instead of silently returning movies.
        let categories = match req.scope {
            SearchScope::Movie => "2000,2010,2020,2030",
            SearchScope::Series | SearchScope::Episode => "5000,5020,5040,5070,5080",
            SearchScope::Track | SearchScope::Artist | SearchScope::Album => "3000,3010,3020,3040",
            _ => "2000,5000",
        };

        let query_enc = url_encode(&req.query);
        let url = format!(
            "{}/api/v2.0/indexers/all/results?apikey={}&Query.SearchTerm={}&{}",
            cfg.base_url, cfg.api_key, query_enc, build_cat_params(categories),
        );

        plugin_info!("jackett: searching — {}", url);

        let raw = match http_get(&url) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let envelope: JackettEnvelope = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("jackett: parse error: {e}");
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        plugin_info!("jackett: {} results", envelope.results.len());

        let kind = match req.scope {
            SearchScope::Series | SearchScope::Episode => EntryKind::Series,
            SearchScope::Track | SearchScope::Artist | SearchScope::Album => EntryKind::Track,
            _ => EntryKind::Movie,
        };

        let items: Vec<PluginEntry> = envelope
            .results
            .into_iter()
            .take(req.limit as usize)
            .map(|r| r.into_entry(kind))
            .collect();

        let total = items.len() as u32;
        PluginResult::ok(SearchResponse { items, total })
    }

    // lookup/enrich/get_artwork/get_credits/related use the default
    // NOT_IMPLEMENTED returns from the trait — jackett is a torrent
    // search plugin, not a metadata source.
}

#[allow(deprecated)]
impl StuiPlugin for JackettProvider {
    fn name(&self) -> &str { &self.manifest.plugin.name }
    fn version(&self) -> &str { &self.manifest.plugin.version }
    fn plugin_type(&self) -> PluginType { PluginType::Provider }

    // Never dispatched — stui_search routes through CatalogPlugin::search
    // via the stui_export_plugin! macro. Kept as a trait stub so the
    // macro's bound `$plugin_ty: StuiPlugin` is satisfied.
    fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
        PluginResult::err("LEGACY_UNUSED", "search dispatches via CatalogPlugin")
    }

    fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse> {
        // The entry ID is "{info_hash}|{magnet_uri}|{link}" packed by into_entry().
        let (info_hash, magnet_uri, link) = parse_entry_id(&req.entry_id);

        let stream_url = if !magnet_uri.is_empty() {
            magnet_uri
        } else if !link.is_empty() {
            link
        } else if !info_hash.is_empty() {
            format!("magnet:?xt=urn:btih:{}&dn=torrent", info_hash)
        } else {
            return PluginResult::err("RESOLVE_ERROR", "no MagnetUri, Link, or InfoHash");
        };

        let truncated: String = stream_url.chars().take(80).collect();
        plugin_info!("jackett: resolve → {}", truncated);

        PluginResult::ok(ResolveResponse {
            stream_url,
            quality: None,
            subtitles: vec![],
        })
    }
}
```

Note: `#[allow(deprecated)]` suppresses the `StuiPlugin` deprecation warning on the impl — required for this transition pattern.

- [ ] **Step 5: Update `JackettResult::into_entry` for the new `PluginEntry` shape**

Find:

```rust
impl JackettResult {
    fn into_entry(self) -> PluginEntry {
        let quality = extract_quality(&self.title);
        let size_str = humanize_bytes(self.size);
        let leechers = (self.peers - self.seeders).max(0);
        let meta = format!(
            "{size_str}  ↑{} ↓{}  {}",
            self.seeders, leechers, self.tracker,
        );

        let id = format!("{}|{}|{}", self.info_hash, self.magnet_uri, self.link);

        let imdb_id = self.imdb.filter(|&i| i > 0).map(|i| format!("tt{:07}", i));

        PluginEntry {
            id,
            title: self.title,
            year: None,
            genre: Some(meta),
            rating: quality,   // string — wrong type under the new SDK
            description: None,
            poster_url: None,
            imdb_id,
            duration: None,
        }
    }
}
```

Replace with:

```rust
impl JackettResult {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let quality = extract_quality(&self.title);
        let size_str = humanize_bytes(self.size);
        let leechers = (self.peers - self.seeders).max(0);
        let meta = format!(
            "{size_str}  ↑{} ↓{}  {}",
            self.seeders, leechers, self.tracker,
        );

        // Pack the three resolution handles into the ID so resolve() needs
        // no second network call. Delimiters: '|' separates hash, magnet,
        // and link. Fields may be empty strings.
        let id = format!("{}|{}|{}", self.info_hash, self.magnet_uri, self.link);

        let imdb_id = self.imdb.filter(|&i| i > 0).map(|i| format!("tt{:07}", i));

        // Put the non-numeric quality tag into description alongside the
        // size/seeders/tracker meta — the new PluginEntry.rating is f32 and
        // "1080p"/"4K" aren't ratings. Ordering: quality first so the row
        // remains scannable.
        let description = match quality {
            Some(q) => Some(format!("{q} · {meta}")),
            None => Some(meta),
        };

        PluginEntry {
            id,
            kind,
            title: self.title,
            description,
            imdb_id,
            // All other Option fields and new ones (artist_name, album_name,
            // track_number, season, episode, original_language, genre,
            // rating, year, poster_url, duration, external_ids) default
            // to None/empty — jackett has no metadata beyond title + size.
            ..Default::default()
        }
    }
}
```

- [ ] **Step 6: Confirm no leftover `req.tab` references**

The old code used `req.tab.as_str()` which no longer exists on `SearchRequest`. Grep the file:

```bash
grep -n "req\.tab\|\.tab\b" stui_plugins/jackett-provider/src/lib.rs
```

Expected: no matches. If any remain, replace them with `req.scope` mappings consistent with the match above.

- [ ] **Step 7: Ensure the `stui_export_plugin!` call remains**

Bottom of file should still have:

```rust
stui_export_plugin!(JackettProvider);
```

The macro has a trait bound `$plugin_ty: CatalogPlugin` + `$plugin_ty: StuiPlugin` — both must be implemented (which we just did). No change needed to the macro call itself.

- [ ] **Step 8: Compile for WASM**

Run:
```bash
cd stui_plugins && cargo build -p jackett-provider --target wasm32-wasip1 --release 2>&1 | tail -20
```

Expected:
- No errors from `jackett-provider` itself. Warnings from outside the `stui_plugins/` workspace (e.g. the monorepo runtime) are out of scope and may appear with non-zero count; only errors matter here.
- Last line: `Finished \`release\` profile ...`
- `target/wasm32-wasip1/release/jackett_provider.wasm` exists.

If a `PluginEntry` field is still missing: re-check that every `PluginEntry { .. }` construction in the file uses `..Default::default()`.

If `PluginManifest`, `CatalogPlugin`, `EntryKind`, or `SearchScope` are unresolved: Step 1's explicit `use` list is missing or incomplete.

If `StuiPlugin` is flagged as unresolved at a trait bound (not as a deprecated warning): confirm the `use` list in Step 1 keeps importing it via `prelude::*` — `StuiPlugin` IS in the prelude.

- [ ] **Step 9: Re-run the manifest parse test**

Run:
```bash
cd stui_plugins && cargo test -p jackett-provider --test manifest_parses 2>&1 | tail -10
```

Expected: still passes (manifest hasn't changed since Chunk 1).

---

## Chunk 3: prowlarr-provider source migration (canary 2)

**Goal:** Identical shape to jackett. Only the config keys, the Newznab endpoint URL, and the parse schema differ.

### Task 3.1: Migrate `prowlarr-provider/src/lib.rs`

**Files:**
- Modify: `stui_plugins/prowlarr-provider/src/lib.rs`

- [ ] **Step 1: Read the current source to identify the prowlarr-specific categories**

```bash
cat stui_plugins/prowlarr-provider/src/lib.rs | head -100
```

Look for the `match req.tab.as_str()` block and note the exact Newznab category strings prowlarr uses (they may differ from jackett's). These numbers stay as-is; only the match variable changes from `req.tab` to `req.scope`.

- [ ] **Step 2: Apply Task 2.1 Steps 1-7 (the source-edit steps)**

Perform Task 2.1 Steps 1 through 7 against `stui_plugins/prowlarr-provider/src/lib.rs`, substituting `JackettProvider` → `ProwlarrProvider` and `jackett` → `prowlarr` throughout. Task 2.1's Steps 8 and 9 (compile + manifest test) are covered by this task's own Steps 3 and 4 below.

- Task 2.1 Step 1 (SDK imports) — same block, same imports.
- Task 2.1 Step 2 (struct) — change identifiers.
- Task 2.1 Step 3 (`Plugin` impl) — change identifier.
- Task 2.1 Step 4 (`CatalogPlugin::search` + stubbed `StuiPlugin`) — same structure; the scope→category match uses prowlarr's category strings from the read in Step 1 above. The `kind` derivation and `plugin_type()` return (`PluginType::Provider`) are identical to jackett.
- Task 2.1 Step 5 (`into_entry`) — the prowlarr result type has different fields; update the same shape using `..Default::default()`.
- Task 2.1 Step 6 (grep for `req.tab`) — same check.
- Task 2.1 Step 7 (`stui_export_plugin!` remains) — same.

- [ ] **Step 3: Compile for WASM**

Run:
```bash
cd stui_plugins && cargo build -p prowlarr-provider --target wasm32-wasip1 --release 2>&1 | tail -10
```

Expected: `Finished` and `target/wasm32-wasip1/release/prowlarr_provider.wasm` exists.

- [ ] **Step 4: Re-run manifest parse test**

Run:
```bash
cd stui_plugins && cargo test -p prowlarr-provider --test manifest_parses 2>&1 | tail -10
```

Expected: pass.

---

## Chunk 4: opensubtitles-provider source migration (canary 3)

**Goal:** Same Approach B trait migration as the torrents, but with `PluginType::Subtitle` in `StuiPlugin::plugin_type()` and `EntryKind::Movie`/`Series` mapped from `req.scope` exactly the same way — opensubtitles's API takes the same scope shape.

### Task 4.1: Migrate `opensubtitles-provider/src/lib.rs`

**Files:**
- Modify: `stui_plugins/opensubtitles-provider/src/lib.rs`

- [ ] **Step 1: Read the current source**

```bash
cat stui_plugins/opensubtitles-provider/src/lib.rs
```

Identify the existing `StuiPlugin::search` method (does a GET to `/api/v1/subtitles?...&languages={lang}` using either `imdb_id` or `query`), the existing `StuiPlugin::resolve` method (POSTs to `/download` with `file_id`, returns subtitle file URL), and the existing into_entry-style function that maps API responses to `PluginEntry`.

- [ ] **Step 2: Apply the jackett pattern (Task 2.1 Steps 1-7, the source-edit steps) with these opensubtitles-specific substitutions**

Task 2.1's Steps 8 and 9 (compile + manifest test) are covered by this task's own Steps 3 and 4 below — don't run them twice.

Same structure as jackett, substituting `JackettProvider` → `OpenSubtitlesProvider` and `jackett` → `opensubtitles`. Differences:

**Scope → OpenSubtitles `type` query-param mapping in `CatalogPlugin::search`:**
```rust
let type_param = match req.scope {
    SearchScope::Series | SearchScope::Episode => "episode",
    SearchScope::Movie => "movie",
    _ => {
        return PluginResult::err(
            "UNSUPPORTED_SCOPE",
            "opensubtitles only supports movie and series/episode scopes",
        );
    }
};
```

Return an error (not a silent fallback) for music scopes. They'd produce wrong results and mask a runtime dispatch bug.

**`into_entry`:** Take an `EntryKind` parameter (derived from `req.scope` exactly as in jackett's mapping). Wrap the existing field construction in `PluginEntry { id, kind, title, description, ..Default::default() }`. Preserve whatever description text the existing code builds (typically language + release name + format) — it remains useful for the user's subtitle pick.

**`StuiPlugin::plugin_type`:** returns `PluginType::Subtitle` (NOT `PluginType::Provider` — this is the one place where torrents and this plugin differ).

**`StuiPlugin::resolve`:** copy verbatim from the existing source. It POSTs `/download` and returns the subtitle file URL as `ResolveResponse.stream_url`; the `subtitles: Vec<SubtitleTrack>` field of `ResolveResponse` stays `vec![]`. The returned `stream_url` IS a subtitle file that stui feeds to aria2 as a download source — it is not a media stream that happens to have attached subtitles.

**JWT caching:** the existing source uses `cache_get("os_jwt")` / `cache_set("os_jwt", ...)` to persist the OpenSubtitles login token across WASM invocations. Keep this caching logic verbatim — do not re-architect it during the migration.

- [ ] **Step 3: Compile for WASM**

Run:
```bash
cd stui_plugins && cargo build -p opensubtitles-provider --target wasm32-wasip1 --release 2>&1 | tail -10
```

Expected: `Finished` and `target/wasm32-wasip1/release/opensubtitles_provider.wasm` exists.

- [ ] **Step 4: Re-run manifest parse test**

Run:
```bash
cd stui_plugins && cargo test -p opensubtitles-provider --test manifest_parses 2>&1 | tail -10
```

Expected: pass.

---

## Chunk 5: CI matrix + release tag

**Goal:** A tagged release publishes all 3 canaries to `ozogorgor.github.io/stui_plugins/plugins.json` with valid binaries + checksums.

### Task 5.1: Extend the CI matrix

**Files:**
- Modify: `stui_plugins/.github/workflows/release.yml`

- [ ] **Step 1: Read current matrix**

Run:
```bash
sed -n '/^  build-wasm:/,/^  build-rpc:/p' stui_plugins/.github/workflows/release.yml
```

Expected: `matrix: plugin:` has `example-provider` and `example-resolver` uncommented, plus commented entries for everything else.

- [ ] **Step 2: Uncomment the 3 canaries in the matrix**

Edit the `plugin` list so it contains exactly:

```yaml
plugin:
  - example-provider
  - example-resolver
  - jackett-provider
  - prowlarr-provider
  - opensubtitles-provider
  # - spotify
  # - tidal
  # - qobuz
  # - roon
  # - soundcloud
```

(Streaming services stay commented — they're a separate session.)

- [ ] **Step 3: Verify the workflow file still parses as valid YAML**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('stui_plugins/.github/workflows/release.yml'))"
```

Expected: exit 0 (no YAML error).

### Task 5.2: Tag + push the release

**Files:**
- No file changes — just git ops in `stui_plugins/`.

- [ ] **Step 1: Confirm all 3 canaries build from a clean state**

Run:
```bash
cd stui_plugins && cargo build --target wasm32-wasip1 --release \
    -p jackett-provider \
    -p prowlarr-provider \
    -p opensubtitles-provider \
    -p example-provider \
    -p example-resolver 2>&1 | tail -5
```

Expected: `Finished` with no errors. Warnings are acceptable — the `StuiPlugin` deprecation is suppressed by the `#[allow(deprecated)]` on the impl blocks, but the macro expansion may still emit them. What matters: exit status 0, and all 5 `.wasm` artifacts present.

If any plugin fails, fix before tagging. Tag pushes trigger CI; a local build failure is guaranteed to fail in CI too.

- [ ] **Step 2: Commit the plan's changes and push**

Per the commit policy at the top of this plan, commits are deferred to Chunk 6. Skip this step — the tag is created AFTER Chunk 6's single commit.

### Task 5.3: (Moved into Chunk 6 — see Task 6.5)

The tag push is sequenced after the consolidated commit.

---

## Chunk 6: Consolidated commit, tag, release verification, install smoke test

**Goal:** Single commit captures all work from chunks 1-5, tag triggers CI, release + plugins.json are validated, canaries install and smoke-test in stui.

### Task 6.1: Pre-commit sanity

- [ ] **Step 1: Verify every canary (and the previously-working plugins) still builds in isolation**

`--workspace` is NOT usable here — it aborts at the first failure, and non-canary streaming plugins (spotify, tidal, qobuz, roon, soundcloud) are known-broken and intentionally out of scope. Build the canary set explicitly:

```bash
cd stui_plugins && cargo build --target wasm32-wasip1 --release \
    -p example-provider \
    -p example-resolver \
    -p jackett-provider \
    -p prowlarr-provider \
    -p opensubtitles-provider 2>&1 | tail -10
```

Expected: `Finished \`release\` profile ...` with all 5 artifacts present in `target/wasm32-wasip1/release/`.

Then sanity-check the non-canary subtitle plugins didn't newly break from the Chunk 1 manifest edits — they're NOT in this build but should at least still be parseable by cargo:

```bash
cd stui_plugins && cargo metadata --no-deps --format-version 1 > /dev/null && echo "workspace parses OK"
```

Expected: exit 0, "workspace parses OK". A failure here means a `plugin.toml` change in Chunk 1 Task 1.4 introduced a syntax error.

If a non-canary subtitle plugin (`kitsunekko`, `subscene`, `yify-subs`) newly fails `cargo build -p <name>` because of the manifest changes in Chunk 1, that's out of scope for this plan — their source is on the legacy trait and they aren't in the CI matrix. Leave the failure in place; it'll be cleaned up in the fleet rollout session.

- [ ] **Step 2: Run all manifest parse tests**

```bash
cd stui_plugins && cargo test --test manifest_parses 2>&1 | tail -20
```

Expected: 3 passes (jackett, prowlarr, opensubtitles). Non-canary plugins don't have this test.

### Task 6.2: Review the staged diff

- [ ] **Step 1: Look at the full diff before committing**

Run:
```bash
cd stui_plugins && git diff --stat HEAD 2>&1
```

Expected files changed:
- `jackett-provider/plugin.toml`
- `jackett-provider/src/lib.rs`
- `jackett-provider/tests/manifest_parses.rs` (new)
- `prowlarr-provider/plugin.toml`
- `prowlarr-provider/src/lib.rs`
- `prowlarr-provider/tests/manifest_parses.rs` (new)
- `opensubtitles-provider/plugin.toml`
- `opensubtitles-provider/src/lib.rs`
- `opensubtitles-provider/tests/manifest_parses.rs` (new)
- `kitsunekko/plugin.toml`
- `subscene/plugin.toml`
- `yify-subs/plugin.toml`
- `.github/workflows/release.yml`

NOT expected: any changes in `sdk/*` or `torrentio-rpc/*`. If you see them, revert — they're out of scope.

### Task 6.3: Confirm with the human before committing

- [ ] **Step 1: STOP and confirm commit with the user**

The user has explicitly asked for no commits until all fundamental plugin work is done. This is the consolidation moment. Before running `git commit`, post the expected file list from Task 6.2 back to the user and ask whether this plan's output is the entirety of "fundamental plugin work" or whether there's more in flight that should be bundled into the same commit.

If they confirm: proceed to Task 6.4.
If they want to wait: stop here. The binaries are local; no CI triggers until the tag push.

### Task 6.4: Single commit

- [ ] **Step 1: Stage + commit**

Run:
```bash
cd stui_plugins && git add -A && git status --short
```

Verify the staged list matches expectations from Task 6.2.

Run:
```bash
cd stui_plugins && git commit -m "$(cat <<'EOF'
feat(plugins): migrate jackett/prowlarr/opensubtitles to current SDK

Approach B from the design spec — real search code lives in
CatalogPlugin::search; StuiPlugin::search is a never-dispatched
stub (required by the trait bound on stui_export_plugin!);
StuiPlugin::resolve stays verbatim because it's still the path
the stui_resolve ABI calls.

Manifest updates drop the deprecated [plugin] type field and
declare capabilities explicitly (streams = true + catalog for
torrents; opaque [capabilities.subtitles] for opensubtitles).
Non-canary subtitle plugins (kitsunekko, subscene, yify-subs)
get manifest triage so the workspace stays parseable; their
source stays on the legacy trait and they remain out of the
release matrix.

CI matrix extends to the 3 canaries.

Spec: docs/superpowers/specs/2026-04-23-media-sources-plugin-migration-design.md
Plan: docs/superpowers/plans/2026-04-23-media-sources-plugin-migration.md

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit created cleanly.

- [ ] **Step 2: Push**

Run:
```bash
cd stui_plugins && git push origin main 2>&1 | tail -3
```

Expected: push succeeds.

### Task 6.5: Tag + push the release

- [ ] **Step 1: Tag v0.2.0**

Run:
```bash
cd stui_plugins && git tag -a v0.2.0 -m "v0.2.0 — jackett, prowlarr, opensubtitles (canary migration)"
cd stui_plugins && git push origin v0.2.0 2>&1 | tail -3
```

Expected: tag pushed.

- [ ] **Step 2: Wait for CI + verify the release**

Run (poll until done):
```bash
gh run list --repo Ozogorgor/stui_plugins --limit 1 2>&1
```

Expected: within ~2-5 minutes, a run for tag `v0.2.0` shows `completed success`.

Then:
```bash
gh release view v0.2.0 --repo Ozogorgor/stui_plugins 2>&1 | grep "^asset:"
```

Expected assets:
- `example-provider-*.tar.gz`
- `example-resolver-*.tar.gz`
- `jackett-provider-*.tar.gz`
- `prowlarr-provider-*.tar.gz`
- `opensubtitles-provider-*.tar.gz`
- `torrentio-rpc-*.tar.gz`
- `plugins.json`

- [ ] **Step 3: Verify plugins.json on Pages**

Run:
```bash
curl -sL https://ozogorgor.github.io/stui_plugins/plugins.json \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(f'{len(d)} entries'); [print(' ', e['name'], e['version']) for e in d]"
```

Expected: 6 entries (5 wasm + 1 rpc), including the three canaries.

### Task 6.6: Install + smoke test in stui

- [ ] **Step 1: Refresh Available in Plugin Manager**

Launch stui, open Plugin Manager, press `r` in Available tab.

Expected: jackett-provider, prowlarr-provider, opensubtitles-provider all appear.

- [ ] **Step 2: Install each canary**

For each of the 3: highlight → `enter` → install. Watch for the toast. Switch to Installed tab.

Expected: all 3 appear in Installed with `status = "loaded"` and `enabled = true`.

- [ ] **Step 3: Configure + smoke test jackett**

Open jackett-provider's settings (plugin manager → installed → enter → plugin settings if that path exists, otherwise via `~/.stui/config/stui.toml` `[plugins.jackett]` section).

Set `url = "http://localhost:9117"` and `api_key = "<your Jackett API key>"`.

Back in stui's main grid, search for a title known to have torrents. Open the stream picker.

Expected: jackett entries appear in the ranked stream list with seeder/size/tracker info in the description. Selecting one resolves to a magnet URL and queues in aria2.

If no entries appear: check the runtime log (`~/.config/stui/runtime.log`) for a `plugin=jackett` line. If the plugin errored (HTTP or parse), the error message will be there.

- [ ] **Step 4: Smoke test prowlarr**

Same flow as jackett with `PROWLARR_URL` + `PROWLARR_API_KEY`.

- [ ] **Step 5: Verify opensubtitles install-only**

Installed tab: opensubtitles-provider shows `status = "loaded"`, `enabled = true`. No search attempt fires (runtime has no subtitle dispatch yet — expected per spec). No crashes, no error toasts.

### Task 6.7: Session handoff

- [ ] **Step 1: Note the next session's work**

The following are now unblocked:
- Runtime subtitle dispatch pipeline (immediate follow-up the user flagged).
- Fleet migration of kitsunekko/subscene/yify-subs (same Approach B template).
- Streaming service plugins (spotify, tidal, qobuz, roon, soundcloud) — larger scope; revisit.

---

## Out-of-band risks to watch for during execution

1. **Compile error cascades in non-canary subtitle plugins.** Their source still uses `StuiPlugin` but the manifest shape changed. If `include_str!("../plugin.toml")` isn't in their source (they probably don't use it — they were written against older SDKs), this is a non-issue. If it is, the plugin may panic at compile time and need the Chunk 2 treatment. Skip to fleet rollout if so; don't fix in this plan.

2. **The `stui_export_plugin!` macro might emit warnings about `StuiPlugin` being deprecated.** The `#[allow(deprecated)]` on the `impl StuiPlugin` block suppresses the per-impl warning but not necessarily the per-call warning from inside the macro expansion. If CI fails due to `-D warnings`, add `#![allow(deprecated)]` at the top of the plugin's `lib.rs`.

3. **OpenSubtitles API auth flow.** The existing source may cache the JWT token from `POST /login`. That cache persistence through WASM is handled by `cache_get` / `cache_set` in the SDK — don't change it; keep the existing caching logic verbatim in the resolve path.

4. **SDK drift between `stui_plugins/sdk/` and `stui/sdk/`.** If the monorepo's SDK has a type the plugin crate's SDK copy doesn't, you'll hit a compile error naming a missing type. Fix by rsyncing the missing file from monorepo → plugin repo (first time this has happened, per the existing duplicated-SDK policy memory). Do NOT publish the SDK to crates.io mid-plan — that's a separate migration.
