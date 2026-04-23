# Subtitle pipeline — design

**Date:** 2026-04-23
**Status:** Design approved. Ready for implementation planning.

## Goal

Close the SDK validator gap that blocks subtitle-only manifests, migrate `kitsunekko` / `subscene` / `yify-subs` to the current plugin SDK (Approach B, same as `opensubtitles-provider`), and wire the runtime's subtitle-capability plugins into the play path so external subtitles auto-download and load into mpv when the user starts a stream.

## Scope

| Layer | Change |
|---|---|
| SDK | `CatalogCapability::default()` flips from `Typed { kinds: [], search: None }` to `Enabled(false)`. Subtitle-only / stream-only manifests validate cleanly; existing typed-catalog manifests keep working. |
| Plugins | `kitsunekko`, `subscene`, `yify-subs` migrated to `Plugin + CatalogPlugin` following the opensubtitles canary template. Added to the CI matrix for publication. |
| Runtime | New `engine::fetch_subtitles(entry, language) -> Vec<SubtitleTrack>` that fans out `stui_search` across enabled subtitle-capability plugins, filters/sorts, returns top candidates. |
| Runtime player path | When `config.subtitles.auto_download == true` AND a stream is resolving, runtime calls `fetch_subtitles` in parallel with mpv warmup; first matching-language candidate is downloaded to `$STUI_DATA_DIR/subtitles/` and passed as `--sub-file=...` in mpv launch args. |
| TUI | Toast line: `"Subtitle: <lang> • <provider>"` on success; `"Subtitle search failed: <err>"` on failure. No picker UI this pass. |

### Out of scope

- **Subtitle picker UI.** Deferred with the card-screen review.
- **Mid-playback subtitle selection.** mpv's IPC `sub-add` is plumbed but not integrated — scope is launch-args only.
- **SDH / forced / hearing-impaired variant preferences.** First-candidate-matching-language wins; quality heuristics are a future iteration.
- **Running migrations on `spotify` / `tidal` / `qobuz` / `roon` / `soundcloud`.** Streaming plugins stay commented in the CI matrix.

## Architecture

### Layer 1 — SDK default

Before:
```rust
impl Default for CatalogCapability {
    fn default() -> Self {
        Self::Typed {
            kinds: Vec::new(),
            search: None,
            lookup: None,
            enrich: None,
            artwork: None,
            credits: None,
            related: None,
        }
    }
}
```

After:
```rust
impl Default for CatalogCapability {
    fn default() -> Self {
        // Subtitle-only / stream-only plugins declare no [capabilities.catalog]
        // block, so serde hits this default. Returning `Enabled(false)` is
        // explicit "no catalog capability declared" and skips the validator's
        // typed-catalog branch (which requires `search: Some(true)` for Typed).
        // Plugins that DO declare a typed catalog block override this at
        // deserialize time, so this doesn't affect metadata plugins.
        Self::Enabled(false)
    }
}
```

The `validate` function at `sdk/src/manifest.rs:450` (re-exported as `validate_manifest` via `sdk/src/capabilities.rs:323`) keeps its existing logic — the `MissingRequiredVerb("search")` check only runs when the variant is `Typed`. With the new default, missing-catalog plugins hit `Enabled(false)` and skip that branch entirely.

Validator behaviour for existing plugins is unchanged. All bundled metadata plugins declare explicit `[capabilities.catalog]` blocks that override the default — their validation path is identical pre- and post-change. Only plugins that OMIT `[capabilities.catalog]` (opensubtitles, kitsunekko, etc.) switch from failing validation to passing it.

Update the duplicated SDK in `stui_plugins/sdk/src/manifest.rs` with the same change.

### Layer 2 — Plugin migrations

Three plugins in `stui_plugins/`:

- `kitsunekko` (anime subtitle scraper)
- `subscene` (general-purpose subtitle scraper)
- `yify-subs` (YIFY releases)

Template: the `opensubtitles-provider` migration from the prior plan. Per plugin:

1. Explicit SDK imports (`parse_manifest, PluginManifest, Plugin, CatalogPlugin, EntryKind, SearchScope`).
2. Struct carries `manifest: PluginManifest`, `Default` parses `include_str!("../plugin.toml")`.
3. `Plugin` impl.
4. `CatalogPlugin::search` carries the real HTTP/scrape logic, maps `req.scope` to a plugin-local categorisation.
5. Unsupported scopes (anything that doesn't make sense for that subtitle source) return `PluginResult::err("UNSUPPORTED_SCOPE", ...)` — no silent fallback.
6. `#[allow(deprecated)] impl StuiPlugin` with the same rationale comment block as jackett/opensubtitles.
7. `StuiPlugin::search` stubbed `LEGACY_UNUSED`.
8. `StuiPlugin::resolve` preserved verbatim (returns the subtitle file URL as `ResolveResponse.stream_url`, empty `subtitles: vec![]`).
9. `plugin_type()` returns `PluginType::Subtitle`.
10. Manifest: drop deprecated `type = "..."`, add `[capabilities.subtitles]` (opaque `_extra`), add `[meta]`.
11. Test file `tests/manifest_parses.rs` using `.kinds()` accessor + `_extra.contains_key("subtitles")`. With the Layer 1 SDK fix, this test can ALSO call `validate_manifest(&m).expect(...)` — a capability the opensubtitles test currently lacks.

CI matrix: uncomment all three in `.github/workflows/release.yml`.

### Layer 3 — Runtime dispatch pipeline

New module `runtime/src/engine/subtitles.rs` (~150 lines) that exposes:

```rust
pub struct SubtitleCandidate {
    pub plugin_id: String,
    pub plugin_name: String,
    pub entry: PluginEntry,   // from stui_search
    pub language: Option<String>, // extracted from entry fields or description
}

pub async fn fetch_subtitles(
    engine: &Engine,
    title: &str,
    imdb_id: Option<&str>,
    kind: EntryKind,
    language: &str,   // BCP-47, typically cfg.subtitles.preferred_language
) -> Vec<SubtitleCandidate>
```

Behaviour:

1. Read-lock registry, collect all enabled `Subtitles`-capability plugins.
2. Build a `SearchRequest { query: title, scope: scope_for(kind), per_scope_limit: Some(20), limit: 20, locale: Some(language), ... }`.
3. Fan out `stui_search` with a short JoinSet + per-plugin 10s timeout.
4. Each plugin's `PluginEntry` list becomes `SubtitleCandidate`s. Language is best-effort: check `entry.original_language` first, then regex-match `(en|eng|english|fr|fra|...)` tokens from description.
5. Filter to candidates where language matches `language` (case-insensitive, normalize BCP-47 3-char and full-name forms). Fallback to `entry.original_language == language`.
6. Sort: exact language match beats partial, imdb_id match beats title-only, then by description length (richer metadata first).
7. Cap at 5 and return. If all plugins error, returns empty vec (not an error — caller treats as "no subs found").

New runtime config: none. Existing `config.subtitles.{auto_download, preferred_language}` is the trigger gate + language param.

### Layer 4 — Player path integration

Existing flow: `main.rs:633` dispatches `"play"` → `pipeline::playback::run_play` → `PlayerBridge::play` at `runtime/src/player/bridge.rs:144`. That function already consults an existing sidecar helper `find_subtitle(data_dir, imdb_id)` at `bridge.rs:528` which scans `{data_dir}/subtitles/{imdb_id}/` for pre-existing `.srt`/`.ass` files and passes the first one as `--sub-file=...`.

**Integration point:** insert a subtitle-fetch step immediately BEFORE `find_subtitle` is called. Chain:

1. Check `config.subtitles.auto_download`. If false → skip; the existing `find_subtitle` sidecar path still fires (useful for subs the user manually dropped).
2. Extract `imdb_id` from the play request's media entry. If absent (pure URL play) → skip.
3. Call `engine.fetch_subtitles(entry, cfg.subtitles.preferred_language)`. Bounded at 5s via `tokio::time::timeout` — on timeout, skip; don't block play.
4. If the first candidate returns: call `stui_resolve(entry_id)` to get the subtitle URL.
5. Download via the existing aria2 bridge to `{data_dir}/subtitles/{imdb_id}/{lang}.srt` — matches the layout `find_subtitle` already knows. Timeout 15s.
6. `find_subtitle` fires next and picks up the newly-landed file as a regular sidecar — `--sub-file=...` gets appended naturally. No bespoke mpv arg-mangling needed at this layer.
7. Emit a `SubtitleFetchedEvent` over the out-of-band event channel → TUI shows the toast.

If any step fails: log, skip subtitle, play continues without subs. Subtitle fetching is strictly best-effort and never blocks playback.

**Why this layout matches the existing sidecar convention:** `bridge.rs:530` has `format!("{}/subtitles/{}", data_dir, imdb_id)`. Writing new subs to that same directory means the pipeline and user-dropped files share storage — no cache divergence, no duplicate-search. The `{lang}.srt` basename is plausibly conventional (mpv autoloads subs by matching the video's base filename, but `--sub-file-paths` + explicit `--sub-file` circumvents that). If the naming ever conflicts, the 5s/15s timeouts ensure a latecoming pipeline write doesn't clobber the user's pre-existing choice.

### Layer 5 — TUI

Add a new IPC out-of-band event `SubtitleFetched { language, provider, file_name }` and `SubtitleSearchFailed { reason }`. Handle in the existing toast pipeline.

No new keyboard shortcuts, no picker, no new screens. The existing settings UI already exposes `subtitles.auto_download` toggle + `preferred_language` dropdown.

## Verification

### Layer 1 gate

`cargo test -p stui-runtime manifest::validator` passes (if such a test exists — add one that feeds a subtitle-only manifest to `validate_manifest` and asserts it succeeds).

### Layer 2 gate

Per plugin: `cargo build -p <plugin> --target wasm32-wasip1 --release` exits 0; `cargo test -p <plugin> --test manifest_parses` passes; the test asserts `validate_manifest(&m).is_ok()`.

### Layer 3 gate

New test in `engine/subtitles.rs`: unit test with a stub plugin that returns 3 PluginEntry with mixed languages; verify filter + sort behaviour. Lives behind `#[cfg(test)]`.

### Layer 4 gate

Manual: play a movie with `subtitles.auto_download = true` and `preferred_language = en`; confirm an `.srt` lands in `~/.stui/data/subtitles/` and mpv shows the subtitle track. Negative test: same movie with plugins disabled → no download attempt, play still works.

### Layer 5 gate

Manual: toast fires within ~3s of play-start; language + provider correct.

## Risks

1. **Subscene / kitsunekko as scrapers.** Both sites fingerprint aggressively and may 403 / Cloudflare-challenge. Mitigation: runtime catches plugin errors and falls through to the next provider. Not a correctness risk, just a "subs not found" outcome — acceptable.

2. **Language matching heuristic is rough.** Some providers use 3-char codes (`eng`), some use 2-char (`en`), some use full names (`English`). Normalisation table lives in `engine/subtitles.rs`. Start with the common cases; expand when a real-world miss is reported.

3. **Runtime blocking vs non-blocking.** The 5s cap on `fetch_subtitles` ensures mpv warmup isn't blocked. But if mpv launches BEFORE subs arrive, the first few seconds of playback will be sub-less. Acceptable for v1.

4. **SDK default change is a subtle behavioural shift.** A plugin that previously relied on the `Typed { kinds: [], search: None }` default (e.g. accidentally omitted a catalog block but wrote code assuming catalog dispatch) will now become an `Enabled(false)` plugin and be invisible to catalog dispatch. Mitigation: grep the workspace for manifests that OMIT `[capabilities.catalog]` but have source code calling `CatalogPlugin::search` real-work — the bundled metadata plugins all have explicit catalog blocks, so only subtitle-only / stream-only plugins change state, which is the intent.

## Deferrals

- Subtitle picker UI (card-screen review).
- Mid-playback subtitle add (mpv IPC `sub-add`).
- Heuristic quality scoring for candidates (SDH / forced / HI detection).
- Migration of `spotify` / `tidal` / `qobuz` / `roon` / `soundcloud` (separate session).
- Publishing the SDK to crates.io (deferred until API stabilises — existing roadmap entry).
