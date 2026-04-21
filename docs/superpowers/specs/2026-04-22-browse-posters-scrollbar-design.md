# Browse polish — posters + always-visible scrollbar

**Date:** 2026-04-22
**Scope:** Movies / Series / Music Browse grids (`tui/internal/ui/screens/grid.go` + `components/card.go`).
**Status:** Design approved; ready for implementation plan.

## 1. Problem

Two user-visible gaps in the Browse grids:

- **No posters.** `CatalogEntry.PosterURL` is populated on the wire (TMDB/OMDb plugins emit it), but the TUI grid never fetches or renders remote images. Today cards fall back to `renderPlaceholderPoster` (initials + genre colour) even when a real image is available.
- **Hand-built scrollbar.** `grid.go:119–162` reinvents the scrollbar inline with its own track/thumb math, while `components/scrollbar.go` already provides `ScrollbarStyle(cursor, viewH, total, style)` — stateless, always-visible by design. The in-tree version drifts from the component and isn't consistent with other screens.

Both land in the same files, so one spec, one implementation pass.

## 2. Non-goals

- **Size-based cache eviction / LRU.** Unbounded for now; size policy is part of the broader caching rewrite tomorrow.
- **Moving poster rendering to the Rust runtime side** (`PosterArt` pre-rendered block art). Kept open as a future drop-in upgrade — see §8.
- **Kitty graphics protocol backend.** `ImageView` already has the code path behind a `symbols`-mode hardcode; swap waits on bubbletea issue #163.
- **Plugin manager / Settings screen work.** Separate spec, separate session.
- **Cancel in-flight downloads on tab switch.** Let them complete into the cache so returning to the tab is instant.

## 3. Architecture

```
  ┌─ Movies/Series/Music tab ──────────┐
  │  grid.go RenderGrid                │
  │    for each entry:                 │
  │      components.RenderCard(...)    │  → posts URL to pool on cache miss
  │    components.ScrollbarStyle(...)  │  → swap from hand-built bar
  └────────────────────────────────────┘
                │
                ↓ (enqueue URL)
  ┌─ components/poster/ (new) ─────────┐
  │  Pool: 4 workers draining a queue  │
  │  skip if cache hit; else download  │
  │  → ~/.stui/cache/posters/<hash>    │
  │  emit a debounced refresh tick     │
  │  when any download completes       │
  └────────────────────────────────────┘
```

## 4. New files

### 4.1 `tui/internal/ui/components/poster/cache.go`

Two pure functions — no state, no network, easy to unit-test.

```go
// CacheKey returns a stable filename for a poster URL.
// Format: <sha256-hex>.<extension preserved from URL path>.
// Extension whitelist: jpg, jpeg, png, webp, gif. Unknown → "jpg".
func CacheKey(url string) string

// CachedPath returns the absolute path to the cached file and a bool
// indicating whether it currently exists on disk.
func CachedPath(url string) (path string, hit bool)
```

Cache directory resolution: `<xdg-cache-home>/stui/posters/` falling back to
`~/.stui/cache/posters/` to stay aligned with the pending XDG migration
(memory project_stui_xdg_migration). Created lazily on first successful
download.

### 4.2 `tui/internal/ui/components/poster/pool.go`

Singleton URL-download pool — 4 goroutine workers, process-lifetime.

```go
type Pool struct { /* unexported */ }

// Global() returns the process-wide Pool, lazily-initialised on first call.
// Safe for concurrent use from any goroutine.
func Global() *Pool

// Enqueue posts a URL for background download. Idempotent: if the URL is
// already queued or in-flight, no-op. If the URL is already cached on disk,
// no-op. Returns immediately.
func (p *Pool) Enqueue(url string)

// RefreshChan returns a channel that receives a struct{} each time AT LEAST
// ONE download has completed since the last receive. Debounced to 150ms so
// a burst of completions coalesces into a single notify. The caller is a
// long-lived tea.Cmd that re-runs on each receive.
func (p *Pool) RefreshChan() <-chan struct{}
```

**Concurrency policy.** The queue is an unbounded channel fed by `Enqueue`.
Workers pick up URLs, check `CachedPath` first (another worker may have
raced in), and skip if so. The in-flight set (map keyed by URL) guards
against duplicate enqueues of the same URL within the same session.

**HTTP behaviour.** Uses `net/http` default client with a 15s timeout.
Non-2xx response or any error: log at debug level and drop (cache stays
unpopulated; next render still shows placeholder; next enqueue will
retry).

## 5. Card render change

`components/card.go::Render` adds one branch above the existing placeholder:

```go
var poster string
switch {
case entry.PosterArt != nil && *entry.PosterArt != "":
    // Existing path — runtime pre-rendered block art wins.
    poster = *entry.PosterArt
case entry.PosterURL != nil && *entry.PosterURL != "":
    if cached, hit := poster_pkg.CachedPath(*entry.PosterURL); hit {
        iv := NewImageView(w, CardPosterRows)
        iv.SetImage(cached)
        poster = iv.View()
    } else {
        poster_pkg.Global().Enqueue(*entry.PosterURL)
        poster = renderPlaceholderPoster(entry, w, CardPosterRows)
    }
default:
    poster = renderPlaceholderPoster(entry, w, CardPosterRows)
}
```

`PosterArt` winning first keeps the door open for tomorrow's runtime-side
pre-render path without any TUI changes.

## 6. Refresh plumbing

A single long-lived `tea.Cmd` runs at TUI startup:

```go
func pollPosterRefresh() tea.Cmd {
    return func() tea.Msg {
        <-poster_pkg.Global().RefreshChan()
        return PostersUpdatedMsg{}
    }
}
```

The Browse tab's `Update` handler recognises `PostersUpdatedMsg` as a
"re-draw, no state change" signal:

```go
case PostersUpdatedMsg:
    return m, pollPosterRefresh()  // re-arm
```

Debounced on the pool side (150ms) so a burst of completions triggers one
re-render, not 20.

## 7. Scrollbar swap

`grid.go:119–162` — delete the entire hand-built bar block. Replace with:

```go
bar := components.ScrollbarStyle(cursor, viewH, total, dimStyle)
```

and render `bar` at the existing slot in the layout. No visual tweaks; the
component already matches the design (always-visible track even when the
thumb fills 100%).

## 8. What tomorrow's caching work picks up

- **Size-based eviction.** Today's cache is unbounded; tomorrow's caching
  pass owns the LRU / quota policy.
- **Runtime-side pre-render.** Moving the download + chafa render to the
  Rust runtime and shipping pre-rendered ASCII in `CatalogEntry.PosterArt`
  replaces the TUI-side pool entirely. Drop-in: §5's `PosterArt` branch
  already wins first, so when the runtime starts populating it the
  TUI-side pool simply idles.
- **Kitty graphics protocol.** `ImageView` already has the code path; swap
  the hardcoded `symbols` format once bubbletea #163 lands. No API change.

## 9. Testing

- **Unit** (`poster/cache_test.go`):
  - `CacheKey` — stable across runs, different URLs get different keys,
    extension whitelist works (unknown → `jpg`).
  - `CachedPath` — missing file returns `(path, false)`; present file
    returns `(path, true)`.
- **Integration** (`poster/pool_test.go`):
  - Spin a `httptest.Server` serving 10 distinct PNGs. Enqueue all 10 URLs
    concurrently from multiple goroutines. Assert:
    - 4 concurrent requests in-flight max (via request counter).
    - All 10 files land in cache dir.
    - Re-enqueuing an already-in-flight URL does not issue a duplicate
      request.
    - `RefreshChan` fires at least once with all completions coalesced.
- **Card render** (snapshot): grid with one entry that has `PosterURL`
  but empty cache → renders placeholder; second render after pool
  populates the cache → renders `ImageView` output.

## 10. Risks + mitigations

- **Chafa binary not installed.** `ImageView` falls back to its existing
  error path (renders the placeholder). No new risk; detection already in
  place.
- **Many distinct URLs per tab (pagination).** Pool queue is unbounded, so
  a huge result set won't block. Workers drain over time; users see
  placeholders filling in as they scroll through.
- **URL changes but content doesn't.** Hash is over URL, not content, so
  a CDN reshuffle would re-download. Acceptable until tomorrow's work
  adds content hashing.
- **Disk full.** Download errors log silently; placeholder persists. No
  crash path.

## 11. Implementation ordering

Suggested chunks (details for the implementation plan):

1. `components/poster/cache.go` — pure functions + unit tests.
2. `components/poster/pool.go` — workers, in-flight map, refresh
   channel + integration tests.
3. `components/card.go` — three-way switch with cache hit / enqueue.
4. `ui.go` / Browse tab — `PostersUpdatedMsg` routing + `pollPosterRefresh`
   Cmd at startup.
5. `grid.go` — scrollbar swap, delete 119–162.
6. Live smoke — open TUI, navigate Movies tab, confirm real posters
   appear after ~1s, confirm scrollbar track visible on short lists.
