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

### From plugin refactor (Chunk 7 smoke)

- **`stui plugin build --release` does not gate on declared stubs.**
  Plan Task 7.5 expects `--release` to reject manifests that carry
  `verb = { stub = true, ... }`, as the gate for external plugins
  uploaded to the Tier-3 registry. Today `--release` just swaps the
  cargo profile to `release` and runs the same linter that tolerates
  stubs with a warning. Add a strict pass in `cli/src/` commands/build
  that, when `--release` is set, promotes declared-stub warnings to
  errors. Bundled plugins keep shipping stubs via the regular
  `plugin build` path; the gate only bites when an author publishes.



- **Rate-limit declarations are unenforced.** `plugin.toml
  [permissions.rate_limit]` is parsed into `RateLimit` and
  `PluginSupervisor::new` constructs a `TokenBucket` from it, but
  `PluginSupervisor::acquire()` is never awaited from any of the
  `Engine::supervisor_*` verb helpers. Every plugin currently runs
  un-throttled regardless of manifest settings. Wire the acquire into
  the shared `call_verb` pathway so all five per-verb entry points
  inherit it once, plus map a `not acquired within N seconds` outcome
  to a `PluginError { code: "rate_limited", retry_after_ms }` surface
  so plan Task 7.3's rate-limit smoke can pass as written. The
  low-level `TokenBucket` already has paused-clock unit tests; only
  the wire-through + error code are missing.



- **IPC plugin-routing by UUID only; add by-name fallback.** The
  `Request::{Lookup,Enrich,GetArtwork,GetCredits,Related}` variants carry
  a `plugin: String` field that `supervisor_*` resolves against
  `PluginRegistry::get(plugin_id)` — a UUID keyed at load time. Every
  IPC caller (the TUI today, any ad-hoc client tomorrow) has to issue
  `Request::ListPlugins` first and maintain a name→UUID map, which makes
  scripting awkward. Add a name lookup in `Engine::supervisor_lookup`
  and siblings: if `plugin_id` doesn't resolve as a UUID, iterate
  `reg.all()` looking for a manifest name match. Preserve UUID priority
  so TUI behaviour doesn't change; name-routing is a strict superset.



- **Supervisor mis-classifies `PluginResult::Err` responses as WASM traps.**
  Every bundled plugin correctly returns
  `PluginResult::err(UNSUPPORTED_SCOPE, ...)` when the engine dispatches
  a scope the plugin doesn't handle (e.g. TMDB on `Artist` scope during
  catalog-refresh fanout). The supervisor's `call_verb` path treats the
  response as `"trap: WASM execution error: unsupported_scope: ..."` and
  increments `crashes_in_window`, scheduling a reload. At 5 crashes the
  plugin is marked `Failed` and unloaded. Symptoms in chunk 7.1 smoke:
  every `Loaded` plugin got a benign reload during startup catalog fanout.
  Fix: in `runtime/src/abi/supervisor.rs::call_verb` (and `::init`), inspect
  the returned envelope — only `InitError::Abi(_)` / memory / timeout
  errors should count toward the crash window; plugin-side
  `PluginResult::Err` must be surfaced as a plain response error. Affects
  dispatch scale-out, not correctness for the single-scope happy path
  (TMDB/MB IPC search returns real results via socat smoke).

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
