# Browse polish — posters + always-visible scrollbar — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `chafa`-rendered posters into Movies/Series/Music Browse grids and replace the hand-built scrollbar in `grid.go` with the already-existing `components.ScrollbarChars` API.

**Architecture:** A new `tui/internal/ui/components/poster/` package owns poster caching (URL → `<stui-cache>/posters/<hash>.ext`) and a global 4-worker downloader pool. `RenderCard` gains a three-way switch: runtime pre-rendered `PosterArt` wins (future-compat), else cache hit → render via `ImageView`, else enqueue + show placeholder. A long-lived `tea.Cmd` listens on the pool's debounced refresh channel and emits `PostersUpdatedMsg` to trigger a re-render. The hand-built scrollbar in `grid.go:119–162` is replaced with a call to `components.ScrollbarChars(...)`.

**Tech Stack:** Go 1.22+, Bubbletea v2, Lipgloss v2, `chafa` external binary, `net/http` standard library, existing `components/imageview.go` + `components/scrollbar.go`.

**Spec:** `docs/superpowers/specs/2026-04-22-browse-posters-scrollbar-design.md`

---

## File Structure

### New files

| Path | Responsibility |
|------|---------------|
| `tui/internal/ui/components/poster/cache.go` | `CacheKey(url)` + `CachedPath(url)` + cache-dir resolution. Pure, no network, no globals. |
| `tui/internal/ui/components/poster/cache_test.go` | Unit tests for `CacheKey`/`CachedPath`. |
| `tui/internal/ui/components/poster/msg.go` | `PostersUpdatedMsg` type (one-file shim so importers don't need the whole pool). |
| `tui/internal/ui/components/poster/pool.go` | Global worker pool: `Global()`, `Enqueue(url)`, `RefreshChan()`. 4 workers, unbounded queue, debounced refresh. |
| `tui/internal/ui/components/poster/pool_test.go` | Integration tests with `httptest.Server` covering concurrency cap, in-flight dedup, refresh coalescing. |

### Modified files

| Path | Change |
|------|--------|
| `tui/internal/ui/components/card.go:40–55` | `Render` gains a three-way switch: `PosterArt` → pre-rendered blocks, else `PosterURL` (cache lookup / enqueue), else existing placeholder. |
| `tui/internal/ui/screens/grid.go:119–162` | Replace hand-built scrollbar char construction with `components.ScrollbarChars(startRow, visibleRows, totalRows, ...)`. |
| `tui/internal/ui/ui.go` | Add `poster.PostersUpdatedMsg` case in `Model.Update` that re-arms `pollPosterRefresh()`. Add `pollPosterRefresh()` to `Model.Init`'s `tea.Batch`. |

### Import alias convention

Pool + cache live in `.../components/poster`. Card rendering has a local variable `poster` (the rendered string). To avoid collision the card file imports it as:

```go
import posterpkg "stui/internal/ui/components/poster"
```

Replace `stui` above with the repo's actual Go module path — grep `go.mod` in `tui/` if unsure. Use `posterpkg` throughout. Other files (`ui.go`) don't have a local `poster` variable and can import plainly as `poster`.

---

## Chunk 1: Poster fetch layer

Self-contained — no TUI changes. Tests exercise the full queue → download → cache path behind an `httptest.Server`.

### Task 1: Poster cache helper

**Files:**
- Create: `tui/internal/ui/components/poster/cache.go`
- Create: `tui/internal/ui/components/poster/cache_test.go`

- [ ] **Step 1.1: Discover the Go module path**

Run: `grep '^module ' tui/go.mod | awk '{print $2}'`
Expected: something like `stui/tui` — note the value; every import `stui/internal/...` later is actually `<module>/internal/...`. In the code below, replace `stui` with the value you just read.

- [ ] **Step 1.2: Write the failing tests**

```go
// tui/internal/ui/components/poster/cache_test.go
package poster

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestCacheKey_Stable(t *testing.T) {
	a := CacheKey("https://image.tmdb.org/t/p/w342/foo.jpg")
	b := CacheKey("https://image.tmdb.org/t/p/w342/foo.jpg")
	if a != b {
		t.Fatalf("CacheKey should be stable, got %q vs %q", a, b)
	}
}

func TestCacheKey_DifferentURLsDiffer(t *testing.T) {
	a := CacheKey("https://a/x.jpg")
	b := CacheKey("https://a/y.jpg")
	if a == b {
		t.Fatalf("different URLs should hash differently: %q == %q", a, b)
	}
}

func TestCacheKey_PreservesWhitelistedExtensions(t *testing.T) {
	tests := []struct {
		url, wantExt string
	}{
		{"https://a/poster.jpg", ".jpg"},
		{"https://a/poster.jpeg", ".jpeg"},
		{"https://a/poster.png", ".png"},
		{"https://a/poster.webp", ".webp"},
		{"https://a/poster.gif", ".gif"},
		{"https://a/poster.BMP", ".jpg"}, // non-whitelisted → fallback
		{"https://a/poster", ".jpg"},      // no extension → fallback
	}
	for _, tc := range tests {
		got := filepath.Ext(CacheKey(tc.url))
		if !strings.EqualFold(got, tc.wantExt) {
			t.Errorf("CacheKey(%q) ext = %q, want %q", tc.url, got, tc.wantExt)
		}
	}
}

func TestCacheKey_StripsQueryAndFragment(t *testing.T) {
	// `?v=…` cache-busters must not fool the extension whitelist.
	got := filepath.Ext(CacheKey("https://a/poster.jpg?v=123&x=y#section"))
	if got != ".jpg" {
		t.Fatalf("query/fragment should be stripped before ext detection; got %q", got)
	}
}

func TestCachedPath_ReportsHitMiss(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	url := "https://a/test.jpg"
	path, hit := CachedPath(url)
	if hit {
		t.Fatalf("should be miss on empty cache dir, got hit=%v path=%q", hit, path)
	}
	// Populate the cached file and re-check.
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("fake"), 0o644); err != nil {
		t.Fatal(err)
	}
	path2, hit2 := CachedPath(url)
	if !hit2 || path2 != path {
		t.Fatalf("should be hit now; got hit=%v path=%q", hit2, path2)
	}
}
```

- [ ] **Step 1.3: Run test to verify it fails**

Run: `cd tui && go test ./internal/ui/components/poster/... -run TestCache -v`
Expected: FAIL — `undefined: CacheKey`, `undefined: CachedPath`.

- [ ] **Step 1.4: Write the minimal implementation**

```go
// tui/internal/ui/components/poster/cache.go
// Package poster owns the TUI-side poster cache + download pool.
//
// Broader asset caching is out of scope for this package; size-based
// eviction lives in the runtime-side caching pass (see
// docs/superpowers/specs/2026-04-22-browse-posters-scrollbar-design.md §8).
package poster

import (
	"crypto/sha256"
	"encoding/hex"
	"net/url"
	"os"
	"path/filepath"
	"strings"
)

// Whitelisted image extensions. Anything else falls back to ".jpg".
var allowedExts = map[string]bool{
	".jpg":  true,
	".jpeg": true,
	".png":  true,
	".webp": true,
	".gif":  true,
}

// CacheKey returns a stable filename for a poster URL:
// `<sha256-hex>.<extension>` where extension is preserved from the URL's
// path (query + fragment stripped first); unknown extensions fall back to
// `.jpg`.
func CacheKey(u string) string {
	sum := sha256.Sum256([]byte(u))
	hash := hex.EncodeToString(sum[:])

	ext := extFromURL(u)
	if !allowedExts[ext] {
		ext = ".jpg"
	}
	return hash + ext
}

// extFromURL extracts the extension from the URL path (not query or fragment).
// Lowercased; returns "" if the path has no extension.
func extFromURL(raw string) string {
	u, err := url.Parse(raw)
	if err != nil {
		return ""
	}
	return strings.ToLower(filepath.Ext(u.Path))
}

// CachedPath returns the absolute on-disk path for a URL's poster, plus
// whether a file currently exists at that path.
func CachedPath(u string) (string, bool) {
	path := filepath.Join(cacheDir(), CacheKey(u))
	if _, err := os.Stat(path); err == nil {
		return path, true
	}
	return path, false
}

// cacheDir resolves to <xdg-cache-home>/stui/posters/, falling back to
// ~/.stui/cache/posters/. The directory is NOT created here — callers
// (pool.go) create it lazily on first successful download.
func cacheDir() string {
	if x := os.Getenv("XDG_CACHE_HOME"); x != "" {
		return filepath.Join(x, "stui", "posters")
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return filepath.Join(os.TempDir(), "stui-posters")
	}
	return filepath.Join(home, ".stui", "cache", "posters")
}
```

- [ ] **Step 1.5: Run test to verify it passes**

Run: `cd tui && go test ./internal/ui/components/poster/... -run TestCache -v`
Expected: PASS — 5 tests.

- [ ] **Step 1.6: Commit**

```
git add tui/internal/ui/components/poster/cache.go tui/internal/ui/components/poster/cache_test.go
git commit -m "feat(tui/poster): URL→disk cache helper with XDG-aware dir"
```

---

### Task 2: Download worker pool

**Files:**
- Create: `tui/internal/ui/components/poster/msg.go`
- Create: `tui/internal/ui/components/poster/pool.go`
- Create: `tui/internal/ui/components/poster/pool_test.go`

- [ ] **Step 2.1: Write the msg type**

```go
// tui/internal/ui/components/poster/msg.go
package poster

// PostersUpdatedMsg is dispatched by the long-lived `PollRefresh` Cmd each
// time the pool reports at least one newly-cached poster since the last
// receive. It's a pure "re-render please" signal; no payload.
//
// Browse-tab-owning models should recognise it and re-arm the poll Cmd:
//
//	case poster.PostersUpdatedMsg:
//	    return m, poster.PollRefresh()
type PostersUpdatedMsg struct{}
```

- [ ] **Step 2.2: Write the failing integration test**

```go
// tui/internal/ui/components/poster/pool_test.go
package poster

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// helper: spawn a server that counts concurrent in-flight requests + total
// requests, so we can assert both caps.
func fixtureServer(t *testing.T) (*httptest.Server, *int32, *int32) {
	t.Helper()
	var inFlight, peak, total int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		cur := atomic.AddInt32(&inFlight, 1)
		for {
			p := atomic.LoadInt32(&peak)
			if cur <= p || atomic.CompareAndSwapInt32(&peak, p, cur) {
				break
			}
		}
		atomic.AddInt32(&total, 1)
		// Hold briefly so concurrency is observable.
		time.Sleep(50 * time.Millisecond)
		w.Header().Set("Content-Type", "image/png")
		_, _ = w.Write([]byte(fmt.Sprintf("fake-png-%s", r.URL.Path)))
		atomic.AddInt32(&inFlight, -1)
	}))
	t.Cleanup(srv.Close)
	return srv, &peak, &total
}

func TestPool_FetchesAndCaches(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, _, _ := fixtureServer(t)

	url := srv.URL + "/inception.png"
	Global().Enqueue(url)

	// Wait up to 2s for the file to appear.
	path := filepath.Join(os.Getenv("XDG_CACHE_HOME"), "stui", "posters", CacheKey(url))
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("expected cached file at %s, got %v", path, err)
	}
}

func TestPool_InFlightCapIsFour(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, peak, total := fixtureServer(t)

	// 10 distinct URLs enqueued concurrently.
	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			Global().Enqueue(fmt.Sprintf("%s/poster-%d.png", srv.URL, n))
		}(i)
	}
	wg.Wait()

	// Wait for drain.
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && atomic.LoadInt32(total) < 10 {
		time.Sleep(20 * time.Millisecond)
	}
	if got := atomic.LoadInt32(total); got != 10 {
		t.Fatalf("expected 10 total requests, got %d", got)
	}
	if got := atomic.LoadInt32(peak); got > 4 {
		t.Fatalf("concurrency cap breached: peak %d > 4", got)
	}
}

func TestPool_DedupesDuplicateEnqueues(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, _, total := fixtureServer(t)

	url := srv.URL + "/dupe.png"
	for i := 0; i < 5; i++ {
		Global().Enqueue(url)
	}

	// Wait for drain (single request expected).
	time.Sleep(300 * time.Millisecond)
	if got := atomic.LoadInt32(total); got != 1 {
		t.Fatalf("duplicate enqueues should dedup; got %d requests", got)
	}
}

func TestPool_RefreshFiresAtLeastOnce(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, _, _ := fixtureServer(t)

	ch := Global().RefreshChan()
	Global().Enqueue(srv.URL + "/notify.png")

	select {
	case <-ch:
		// pass
	case <-time.After(2 * time.Second):
		t.Fatal("RefreshChan never fired after a successful download")
	}
}
```

- [ ] **Step 2.3: Run tests to verify they fail**

Run: `cd tui && go test ./internal/ui/components/poster/... -run TestPool -v`
Expected: FAIL — `undefined: Global`, `undefined: resetPoolForTest`.

- [ ] **Step 2.4: Write the pool implementation**

```go
// tui/internal/ui/components/poster/pool.go
package poster

import (
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const (
	poolWorkers     = 4
	refreshDebounce = 150 * time.Millisecond
	httpTimeout     = 15 * time.Second
)

// Pool downloads poster URLs concurrently into the on-disk cache and
// emits a debounced refresh signal when any download completes.
type Pool struct {
	queue    chan string
	refresh  chan struct{}
	inFlight sync.Map // url -> struct{}
	client   *http.Client
}

var (
	globalOnce sync.Once
	global     *Pool
)

// Global returns the process-wide Pool, lazily spawning 4 workers on first
// call. Safe for concurrent use.
func Global() *Pool {
	globalOnce.Do(func() {
		global = newPool()
		global.start()
	})
	return global
}

// resetPoolForTest clears the global pool so each test starts fresh.
// Only compiled into tests via the _test.go wiring below.
func resetPoolForTest() {
	globalOnce = sync.Once{}
	global = nil
}

func newPool() *Pool {
	return &Pool{
		queue:   make(chan string, 256),
		refresh: make(chan struct{}, 1), // buffered(1): non-blocking send coalesces bursts
		client:  &http.Client{Timeout: httpTimeout},
	}
}

func (p *Pool) start() {
	// Spawn a debouncer: every refreshDebounce window, if any completion
	// landed, emit one struct{} to the observable refresh channel.
	raw := make(chan struct{}, 256)
	go func() {
		var pending bool
		tick := time.NewTicker(refreshDebounce)
		defer tick.Stop()
		for {
			select {
			case <-raw:
				pending = true
			case <-tick.C:
				if pending {
					pending = false
					select {
					case p.refresh <- struct{}{}:
					default: // already pending; coalesce
					}
				}
			}
		}
	}()

	for i := 0; i < poolWorkers; i++ {
		go p.worker(raw)
	}
}

func (p *Pool) worker(doneSignal chan<- struct{}) {
	for url := range p.queue {
		// Check cache again under worker — another worker might have raced.
		if _, hit := CachedPath(url); hit {
			p.inFlight.Delete(url)
			continue
		}
		if err := p.download(url); err == nil {
			// Notify debouncer without blocking.
			select {
			case doneSignal <- struct{}{}:
			default:
			}
		}
		p.inFlight.Delete(url)
	}
}

// Enqueue posts a URL for background download. Idempotent — duplicate
// URLs currently queued or in-flight are dropped. Already-cached URLs are
// dropped too (avoids pointless queue pressure).
func (p *Pool) Enqueue(url string) {
	if url == "" {
		return
	}
	if _, hit := CachedPath(url); hit {
		return
	}
	if _, loaded := p.inFlight.LoadOrStore(url, struct{}{}); loaded {
		return
	}
	// Non-blocking send; dropping rather than stalling the UI thread is
	// acceptable for a best-effort poster cache.
	select {
	case p.queue <- url:
	default:
		p.inFlight.Delete(url) // let a later enqueue retry
	}
}

// RefreshChan returns a buffered(1) channel that fires at least once per
// debounced window when any download completes. See spec §4.2.
func (p *Pool) RefreshChan() <-chan struct{} {
	return p.refresh
}

// download fetches URL and writes the body to the cache path.
func (p *Pool) download(url string) error {
	path, hit := CachedPath(url)
	if hit {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return &httpErr{status: resp.StatusCode}
	}
	// Write atomically via a tmp file + rename so partial reads never happen.
	tmp := path + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if _, err := io.Copy(f, resp.Body); err != nil {
		_ = f.Close()
		_ = os.Remove(tmp)
		return err
	}
	if err := f.Close(); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return os.Rename(tmp, path)
}

type httpErr struct{ status int }

func (e *httpErr) Error() string { return http.StatusText(e.status) }
```

- [ ] **Step 2.5: Add a `PollRefresh` Cmd helper**

Append to `pool.go`:

```go
// PollRefresh returns a tea.Cmd that blocks on the next refresh signal and
// emits a PostersUpdatedMsg. The caller's Update handler is responsible
// for re-arming the Cmd after each receive to keep the subscription alive.
//
// The `any` return type avoids a cross-package dependency from the poster
// package on Bubbletea — callers adapt via `tea.Cmd(poster.PollRefresh())`.
func PollRefresh() func() any {
	return func() any {
		<-Global().RefreshChan()
		return PostersUpdatedMsg{}
	}
}
```

Note: the signature `func() any` is intentionally Bubbletea-compatible
(`tea.Cmd` is `func() tea.Msg`, and `tea.Msg` is an alias for `any`).
Callers in `ui.go` use it as:

```go
var pollPosterRefresh tea.Cmd = tea.Cmd(poster.PollRefresh())
```

- [ ] **Step 2.6: Run tests to verify they pass**

Run: `cd tui && go test ./internal/ui/components/poster/... -v`
Expected: PASS — 5 cache tests + 4 pool tests = 9 tests.

- [ ] **Step 2.7: Commit**

```
git add tui/internal/ui/components/poster/pool.go \
        tui/internal/ui/components/poster/pool_test.go \
        tui/internal/ui/components/poster/msg.go
git commit -m "feat(tui/poster): 4-worker download pool with debounced refresh"
```

---

### Chunk 1 Review Checkpoint

Before proceeding to Chunk 2:

- [ ] `go test ./internal/ui/components/poster/... -race` passes clean (race detector catches concurrent map misuse).
- [ ] `go vet ./internal/ui/components/poster/...` clean.
- [ ] The package compiles with zero cross-package deps on `bubbletea` — the only imports should be stdlib.

---

## Chunk 2: TUI integration

### Task 3: Card render — three-way switch

**Files:**
- Modify: `tui/internal/ui/components/card.go` (Render function, ~lines 40–55)

- [ ] **Step 3.1: Open card.go to find the existing switch**

Read: `tui/internal/ui/components/card.go` lines 40–75. Confirm the current structure:

```go
if entry.PosterArt != nil && *entry.PosterArt != "" {
    poster = *entry.PosterArt
} else {
    poster = renderPlaceholderPoster(entry, w, posterH)
}
```

- [ ] **Step 3.2: Add the import**

At the top of `card.go`, add (replace `stui/tui` with the actual module path from Step 1.1):

```go
import (
    // ...existing imports...
    posterpkg "<module-path>/internal/ui/components/poster"
)
```

- [ ] **Step 3.3: Rewrite the branch as a three-way switch**

Replace the existing if/else with:

```go
// Precedence:
//  1. `PosterArt` — runtime pre-rendered block art (future caching path).
//  2. `PosterURL` + on-disk cache hit — render through ImageView (chafa).
//  3. `PosterURL` + cache miss — enqueue for background download, show
//     existing placeholder so the user sees SOMETHING immediately.
//  4. Neither — existing placeholder.
var poster string
switch {
case entry.PosterArt != nil && *entry.PosterArt != "":
    poster = *entry.PosterArt
case entry.PosterURL != nil && *entry.PosterURL != "":
    if cached, hit := posterpkg.CachedPath(*entry.PosterURL); hit {
        iv := NewImageView(w, posterH)
        iv.SetImage(cached)
        poster = iv.View()
    } else {
        posterpkg.Global().Enqueue(*entry.PosterURL)
        poster = renderPlaceholderPoster(entry, w, posterH)
    }
default:
    poster = renderPlaceholderPoster(entry, w, posterH)
}
```

- [ ] **Step 3.4: Build to catch any compile errors**

Run: `cd tui && go build ./internal/ui/components/...`
Expected: clean build.

- [ ] **Step 3.5: Run existing card-package tests (if any)**

Run: `cd tui && go test ./internal/ui/components/...`
Expected: PASS. The poster package tests from Chunk 1 should continue to pass; card tests (if present) should too.

- [ ] **Step 3.6: Commit**

```
git add tui/internal/ui/components/card.go
git commit -m "feat(tui/card): three-way switch — PosterArt | cached URL | placeholder"
```

---

### Task 4: Refresh plumbing in the root Model

**Files:**
- Modify: `tui/internal/ui/ui.go` — add import, Init Cmd, Update handler.

- [ ] **Step 4.1: Open ui.go, find Model.Init and Model.Update**

Run: `grep -n "func (m Model) Init\|func (m Model) Update" tui/internal/ui/ui.go`
Expected: two matches. Note the line numbers.

- [ ] **Step 4.2: Add the import**

At the top of `ui.go` with the other `tui/internal/ui/components/...` imports (replace `<module-path>` with the value from Chunk 1 Step 1.1):

```go
import (
    // ...existing...
    "<module-path>/internal/ui/components/poster"
)
```

- [ ] **Step 4.3: Register the poll Cmd in Init**

In `Model.Init`, the function returns a `tea.Cmd` — frequently a `tea.Batch(...)` or a single Cmd. Add the poster poll to the batch:

```go
func (m Model) Init() tea.Cmd {
    // existing cmds...
    return tea.Batch(
        // ...existing...
        tea.Cmd(poster.PollRefresh()),
    )
}
```

If `Init` currently returns a single Cmd (not a batch), wrap:

```go
return tea.Batch(<existing-cmd>, tea.Cmd(poster.PollRefresh()))
```

- [ ] **Step 4.4: Handle PostersUpdatedMsg in Update**

Inside `Model.Update`, after the existing `case` branches (e.g. after `case ipc.PluginToastMsg:`), insert:

```go
case poster.PostersUpdatedMsg:
    // Re-arm the poll so we keep listening. No model-state change — the
    // next View() pass picks up newly-cached posters directly.
    return m, tea.Cmd(poster.PollRefresh())
```

- [ ] **Step 4.5: Build**

Run: `cd tui && go build ./...`
Expected: clean.

- [ ] **Step 4.6: Sanity-run the existing TUI test suite**

Run: `cd tui && go test ./internal/ui/...`
Expected: existing tests still pass; no poster-specific test regressions.

- [ ] **Step 4.7: Commit**

```
git add tui/internal/ui/ui.go
git commit -m "feat(tui): route PostersUpdatedMsg + arm poster.PollRefresh on init"
```

---

### Task 5: Scrollbar swap in grid.go

**Files:**
- Modify: `tui/internal/ui/screens/grid.go:119–162` (hand-built scrollbar block).

- [ ] **Step 5.1: Read the current scrollbar block**

Open `tui/internal/ui/screens/grid.go` lines 119–162. The existing code computes `thumbH`, `thumbTop`, and builds `sbChars []string` (one styled char per visible row) with `│` for track and `▐ █ ▌` for thumb top/body/bottom.

- [ ] **Step 5.2: Confirm the component signature**

`components.ScrollbarChars(scroll, viewH, totalItems int, style lipgloss.Style) []string` — returns one styled char per row using `█` for thumb and `░` for track. It always draws the track even when everything fits (spec requirement).

**Note:** spec §7 originally named `ScrollbarStyle` (which returns a single concatenated string). We intentionally use `ScrollbarChars` here instead — the per-row `[]string` return is load-bearing because `grid.go` interleaves one scrollbar char per grid row during its per-row render loop. Semantically equivalent; just the right shape for this call site.

- [ ] **Step 5.3: Replace the block**

Delete lines 133–162 (the `var sbChars []string` through the end of the trailing `}` on the `if needsScrollbar` block). Replace with:

```go
// Build scrollbar (one styled char per visible row). The component
// always renders the track — we only allocate the column when
// totalRows > visibleRows; otherwise gridWidth keeps its full span.
var sbChars []string
if needsScrollbar {
    sbStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())
    sbChars = components.ScrollbarChars(startRow, visibleRows, totalRows, sbStyle)
}
```

Keep lines 119–132 as-is — they compute `needsScrollbar`, `gridWidth`, `cw`, `startRow`, `endRow` which the rest of the function still needs.

- [ ] **Step 5.4: Build**

Run: `cd tui && go build ./internal/ui/screens/...`
Expected: clean.

- [ ] **Step 5.5: Run grid tests if present**

Run: `cd tui && go test ./internal/ui/screens/...`
Expected: no regressions. If there's a snapshot test that hardcodes the old `│ ▐ █ ▌` characters, update the snapshot; the new bar uses `░` and `█` which visually reserve space the same way.

- [ ] **Step 5.6: Commit**

```
git add tui/internal/ui/screens/grid.go
git commit -m "refactor(tui/grid): replace hand-built scrollbar with components.ScrollbarChars"
```

---

### Task 6: Live smoke

Not a committable step — just a hand-test before declaring done.

- [ ] **Step 6.1: Build the TUI binary**

```
cd tui && go build -o /tmp/stui-browse-smoke ./cmd/stui
```

Expected: clean build, binary at `/tmp/stui-browse-smoke`.

- [ ] **Step 6.2: Ensure runtime is running**

In a separate terminal (or background):
```
(cd /home/ozogorgor/Projects/Stui_Project/stui && \
  set -a; . ~/.stui/secrets.env; set +a && \
  /home/ozogorgor/.cargo/target/debug/stui-runtime daemon)
```

- [ ] **Step 6.3: Launch the TUI against it**

```
/tmp/stui-browse-smoke
```

- [ ] **Step 6.4: Navigate through all three Browse tabs**

- Press `1` (Movies): confirm a grid appears, posters fill in within ~1–2s (background downloader completes TMDB/OMDb hits), scrollbar column is visible on the right whenever the row count > viewport.
- Press `2` (Series): same expectations.
- Press `3` (Music): same expectations (MusicBrainz + Discogs + Last.fm plugins should populate).
- Cards WITHOUT a poster URL should still render the placeholder — no crash, no empty block.

- [ ] **Step 6.5: Confirm cache on disk**

```
ls ~/.stui/cache/posters/ | head
```

Expected: a handful of `<hex>.jpg` / `.png` files corresponding to the posters you saw.

- [ ] **Step 6.6: Stop the daemon, wrap up**

If everything behaves, the implementation is complete.

---

## Final review

- [ ] All tests pass: `cd tui && go test ./... -race`
- [ ] `cd tui && go vet ./...` clean.
- [ ] Spec §2 non-goals preserved: no size-based eviction, no runtime-side rendering, no kitty backend swap, no plugin-manager changes.
- [ ] Ready to hand off to tomorrow's caching work: `PosterArt` still wins first in card render; swap to runtime-side is drop-in.

**Tip:** invoke superpowers:finishing-a-development-branch to merge / PR once the smoke passes.
