# STUI Backlog

Carried-forward deferrals and inspirational work items, consolidated from
spec/plan commit messages and prior session tracking. Review and reorder
before starting each new project. Items tracked with originating context so
a new session can pick up where the last left off.

Last updated: 2026-04-20 (post search-refactor merge, start of plugin-refactor brainstorm).

---

## Tier 1 — Immediate / next projects

### 1. Metadata plugin refactor (in progress)

Spec in progress this session. Scope confirmed during brainstorm:
- Per-capability trait structure (`Plugin` + `CatalogPlugin`), `CatalogPlugin`
  has the full metadata verb suite (`search`, `lookup`, `enrich`, `get_artwork`,
  `get_credits`, `related`).
- Closed canonical id-source registry; strict validation at load.
- 4-state plugin status (`Loaded | NeedsConfig | Failed | Disabled`) with
  declarative `required = true` config handled by runtime, init's job is
  wiring only.
- `PluginEntry.external_ids: HashMap<String, String>` for cross-namespace
  id carriage.
- Metadata plugin WASM-only; audit existing plugins + migrate keepers,
  drop/mothball the rest.
- DX: full `stui plugin {init,build,test,lint,install --dev}` CLI.
- Permissions: network allowlist + rate-limit declaration (token-bucket
  enforced at `stui_http_get`). Filesystem dropped from metadata manifests.
- Non-metadata plugins (`subscene`, `kitsunekko`, `yify-subs`, `torrentio-rpc`)
  move out of the bundled repo to their own — leftovers from a prior move.

### 2. Caching overhaul (user-requested, slated next after plugin refactor)

Motivator: search-refactor Task 7.0 deferral #5 — `SearchCache<Vec<CatalogEntry>>`
vs `supervisor_search → Vec<MediaEntry>` type mismatch. Scoped-search results
are not cached today; every search hits plugins live.

Rough scope for when the caching project starts:
- Generalize `SearchCache` to hold `Vec<MediaEntry>` (or a polymorphic entry
  type covering both search and catalog paths).
- Define per-verb TTLs. Plugin manifest gains `[cache]` block declaring
  TTLs per verb (`search_ttl`, `lookup_ttl`, `artwork_ttl`, `enrich_ttl`,
  `credits_ttl`, `related_ttl`). Runtime respects.
- Cache invalidation strategy: TTL expiry + plugin reload/crash + user-action
  refresh.
- Artwork caching is a special case (binary blobs, not JSON) — may need a
  separate content-addressable store under `~/.stui/cache/artwork/`.
- Cross-verb dedup: if `search` returns an entry and `lookup` is called on
  it, we already have some fields cached. Policy for merging.

### 3. Media-source plugin refactor (informed by metadata-plugin refactor)

Stream/subtitle/torrent plugins. User-installable from a dedicated plugin
repo, not bundled. Probably adds:
- Capability traits: `StreamResolver`, `SubtitleProvider`, possibly
  `TorrentProvider`.
- Plugin installation flow (currently `~/.stui/plugins/` drop-in).
- Maybe: RPC-plugin unification with WASM (the adhoc `plugin_rpc/` module
  stays dormant until this refactor).

---

## Tier 2 — Code cleanups (small, can be batched into maintenance PRs)

### Pre-existing auth test failures (surfaced 2026-04-23)

After fixing compile errors introduced by the `original_language` field,
`cargo test -p stui-runtime` runs with 471 passing and 5 failing. All 5
failures are in the auth subsystem:
  - `abi::host::inner_impl::tests::test_auth_receiver_stored_in_host_state`
  - `auth::callback_server::tests::test_allocate_returns_port_and_fires_receiver_on_callback`
  - `auth::tests::test_open_and_wait_denied`
  - `auth::tests::test_open_and_wait_timeout`
  - `plugin_rpc::process::tests::test_auth_phase_allocated_allows_realloc`
Unrelated to this session's catalog/cache/engine/tvdb work — likely
flaky due to port binding / process-spawn timing, or a pre-existing real
regression that slipped under a test-compile outage. Investigate with
`cargo test -p stui-runtime auth:: -- --nocapture` to see the panics.

### Test-literal repairs (post-original_language addition, 2026-04-22) — ✅ DONE 2026-04-23

`CatalogEntry` now derives `Default`; `MediaEntry` now derives `Default`
and `MediaTab` now derives `Default` with `#[default] Movies`. Remaining
struct literals across tests (`catalog.rs`, `catalog_engine/aggregator.rs`,
`ipc_batcher.rs`, `engine/search_scoped.rs`, `mediacache/store.rs`,
`tests/provider_tests.rs`) either got the `original_language: None` field
added explicitly or switched to `..Default::default()`. Engine::new() test
call sites updated to the 3-arg signature. Test suite compiles again.

### Corrupt chafa posters on some AniList entries (2026-04-22)

Repro: browse Series tab past the general-bucket tail into anime entries.
A handful of cards (LIAR GAME, Rent-a-Girlfriend S5) render a noisy/
broken poster instead of the image — title + meta bar below are correct.
Other anime from AniList render cleanly. Hypothesis: the poster URL
returns a non-image payload (404 HTML, or an image format chafa's build
can't decode).

**Partial progress (2026-04-23):** `imageview.go::render()` now captures
chafa stderr and logs `path=X err=Y stderr=Z` when the subprocess fails
or produces empty output. Next step: reproduce the broken card, check
runtime.log for the failure line, and confirm whether it's an HTTP
content-type issue (poster cache stored HTML), an unsupported image
format, or something else.

### UI scroll sluggishness + R-refresh freeze (2026-04-22)

Observed: grid scrolling feels slow; pressing R briefly freezes the UI while
the newly-arrived entries render. The chafa in-memory cache is already
wired (`cardImageViews` map in `tui/internal/ui/components/card.go` +
`ImageView`'s internal `(path,w,h)->lines` cache in `imageview.go`), so
steady-state scrolling isn't re-shelling chafa — the issue is elsewhere.

Three suspects ranked by likelihood:

1. **Synchronous chafa on first-render of freshly-downloaded posters.**
   When R triggers a refresh and 72 entries with novel poster URLs land,
   the first render enqueues all into the poster download pool. Each
   download-complete tick triggers re-render; the first render of each
   card then invokes chafa synchronously inside `ImageView.Lines()` on the
   bubbletea message-loop thread. Bursts of ~10 concurrent chafa calls
   at ~30–80ms each = visible stall. Fix: render chafa in a background
   goroutine per cached path and push the rendered lines back via a
   `ChafaReadyMsg` — the card's first draw shows the placeholder, second
   frame gets the real art. File: `tui/internal/ui/components/imageview.go`
   around `render()`.

2. **Full `View()` recompute on every Msg.** Every keystroke re-renders
   the entire model: grid + topbar + footer + search + status. Grid alone
   is 10 cards × (poster + title style + meta style + genre style + border
   + overlay splice). `lipgloss.Width(line)` (ANSI-aware width calc) runs
   per line of every row inside the scrollbar-attach loop in
   `tui/internal/ui/screens/grid.go`. Profile candidates: memoize rendered
   cards keyed by `(entry.ID, selected)` — only re-render when either
   changes; cache per-row width so the scrollbar loop doesn't re-measure.

3. **Message-loop contention from concurrent IPC.** MPD status ticks,
   player heartbeats, and plugin toasts all dispatch bubbletea messages.
   If any handler holds the loop for more than ~16ms, scroll keypresses
   queue behind them. Diagnostic: instrument `Update` in `ui.go` with a
   duration log per message type; the slowest types are likely the
   optimization targets. File: `tui/internal/ui/ui.go` Update().

Prefer starting with (1) — the fix is bounded (one file, one goroutine
spawn) and targets the most-reported freeze (R + scroll through fresh
results). Revisit (2) and (3) if scroll still feels heavy after (1).

### From caching Phase 1 (2026-04-22)

- **DSP config IPC timeout (30s) exposed once grid refresh became instant.**
  `Request::SetDspConfig` in `runtime/src/main.rs:1121` takes a tokio Mutex on
  the DSP pipeline, mutates config, calls `pipeline.update_config(cfg).await`.
  One of three suspects: (1) `update_config` awaits the output thread holding
  a lock while mutating, (2) the DSP Mutex is held by a long-running MPD-
  bridge op, (3) head-of-line blocking in the IPC loop when another slow
  handler is ahead. Cache didn't cause it — it just stopped masking the
  30s wait behind slower grid fan-outs. Repro by opening Movies + hitting a
  DSP toggle in settings while a search is mid-flight.

### Lastfm provider robustness — ✅ DONE 2026-04-23

`parse_json()` in `plugins/lastfm-provider/src/lib.rs` now detects the
`{"error": N, "message": "..."}` envelope before falling through to the
strongly-typed deserialize. Turns "missing field `tracks`" into
"lastfm API error 10: Invalid API key" which is actionable.

### Post-TVDB integration

- **TVDB extended metadata endpoint** (`/movies/{id}/extended` and series
  equivalent) — types already declared in `runtime/src/tvdb/types.rs`
  (`ExtendedRecord`) with `#[allow(dead_code)]`; wire a `lookup` method on
  `TvdbClient` once there's a caller (Phase 2 of caching or detail-view
  enrichment).
- **TVDB acknowledgement screen** — user requested this as a follow-up
  after TVDB integration lands. Show the TVDB logo + attribution text in a
  credits/about view (location TBD; no such screen today).
  Attribution requirements per https://thetvdb.com/api-information#attribution:
  (a) "Metadata provided by TheTVDB" text, (b) TVDB logo, (c) link to
  https://www.thetvdb.com (or https://thetvdb.com). All three must appear
  wherever TVDB data is displayed or at minimum in a dedicated credits view.
  Assets: grab the logo from the same page, bundle under `tui/assets/` as
  an ANSI block-art variant (chafa-friendly) since we can't render raster
  PNG in the TUI.
- **TVDB trending/discover for empty-query refresh** — `/search` rejects
  empty queries (HTTP 400). TVDB has no direct `/trending` like TMDB, but
  `/movies?page=N` (all-movies by id) or `/movies/filter?sort=score&...`
  (untested, endpoint shape finicky) could provide a curated list. Today
  we skip TVDB when query is empty. Explore the filter endpoint later so
  TVDB contributes to the initial catalog fan-out too.

Open items from search-refactor Task 7.0 that didn't land in that branch:

### From search refactor

- **Task 7.0 #6 — `MediaEntry.provider` vs `MediaEntry.source` consolidation.**
  Decide canonical field, migrate readers, drop the loser. Same on
  `PluginEntry.source` (SDK-side). Touches Go + Rust.
  Owning file start points: `runtime/src/ipc/v1/mod.rs` (MediaEntry),
  `sdk/src/lib.rs` (PluginEntry), `tui/internal/ipc/types.go` (Go MediaEntry).
- **Task 7.0 #11 — MPD search test backfill.**
  `runtime/src/mpd_bridge/search.rs` tests 1 and 2 only exercise pure mapper
  helpers; the `search()` method's real scope-gate logic is untested.
  Needs a live MPD harness (feature-gated integration test module).
- **Task 7.0 #16 — Lazy Sources column for video grids.**
  `catalogbrowser.SourcesCountResolver` is built but not wired into
  Movies/Series/Library grids because no Streams-plugin resolve IPC exists.
  Blocked on: (a) Rust-side IPC method calling `Streams`-capable plugins for
  entry counts, (b) Go `Client.ResolveSourcesCount(ctx, entryID)`, (c) grid
  rendering with lazy hover trigger. Gets unblocked after media-source
  plugin refactor defines the Streams trait.
- **Task 7.0 #17 — `dispatchPersonSearch` migration.**
  Person-mode search (actor/director detail overlay) still uses legacy
  `ipc.SearchResultMsg`. Blocks deletion of `ipc.SearchResult` + `SearchResultMsg`
  types. Migrate to the scoped-search API; then delete legacy types.
  File: `tui/internal/ui/ui.go:~2390`.

### Pre-existing issues, not introduced by search refactor

- **`tui/internal/ui/screens/music_queue_test.go:127/140/152/160`** —
  `queueColWidths` returns 4 values, tests assign to 3. Predates search
  refactor. Owner: whoever owns queue UI.
- **Rust `rustls` auth-callback tests** — 5 tests fail with
  `InconsistentKeys(KeyMismatch)` panic; missing
  `CryptoProvider::install_default()` in test harness setup. Unrelated
  to search or plugin work. `runtime/src/auth/callback_server.rs` +
  related.

---

## Tier 3 — Inspirational (C-territory from plugin-refactor Q1)

Long-term goals from the white paper roadmap, kept as constraints on
current design but not in any current project's scope:

- **Plugin signing + permission manifests.** Signed plugin authenticity;
  runtime validates signature before loading. Permission manifest is
  declarative + runtime-enforced.
- **Plugin registry.** Hosted plugin store where users browse, install,
  update, rate plugins. Starts as "a dedicated GitHub repo of plugins"
  and grows if traction justifies it.
- **`stui plugin {sign,publish,search}` CLI.** Extensions to the
  subcommand tree defined in metadata-plugin refactor.
- **Plugin SDK test harness.** Already scoped into metadata-plugin refactor
  as `stui plugin test`; inspirational extensions: golden-file fixtures,
  property tests, contract tests against a known plugin registry.
- **Non-WASM plugin SDKs.** Python / Go / Deno / JS. Requires RPC/IPC
  protocol standardization (today's `plugin_rpc/` is an adhoc Python-only
  precursor).
- **Distributed / P2P plugin clusters (Phase 3 long-term).** Plugins
  running remotely across a user's devices. Out of scope indefinitely.
- **Theming engine.** Separate from plugins. No design work yet.

---

## Tier 4 — Already landed (for audit)

Items that used to be deferrals but were resolved in recent branches.
Kept here for traceability; can prune after a cleanup pass.

### search-refactor branch (merged commit `b599334`, 56 commits)

- Task 7.0 items 1, 2 (ABI sync) — landed in `66d6b55`.
- Task 7.0 item 3 (legacy Engine::search retirement) — landed in `187669c`.
- Task 7.0 item 4 (Response::SearchResult deprecation) — marked deprecated
  in `bf5b05c`; full deletion awaits #17 migration.
- Task 7.0 items 7, 8, 10 (doc cleanups) — `88caf2b`, `967546b`, `9a660fc`.
- Task 7.0 item 9 (tracing span propagation) — `dbcd825`.
- Task 7.0 items 12, 13 (Go-side robustness) — `2bc0b31`, `a37ca7b`.
- Task 7.0 item 14 (debounce live typing) — `b97abd0`.
- Task 7.0 item 15 (positive-path routing tests) — `81b708d`.
- Final review nit (MpdDataSource mutex) — `b599334`.

---

## Process notes

- Each new project starts a brainstorm → spec → plan → implementation cycle.
- Brainstorm should reference this doc for context.
- When a project finishes, its deferrals land here (Tier 2 or Tier 3 as
  appropriate).
- When an item in Tier 2/3 gets picked up, move it into Tier 1 as "in
  progress," then Tier 4 when done.
