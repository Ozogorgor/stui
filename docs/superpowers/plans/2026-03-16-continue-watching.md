# Continue Watching Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "Continue Watching" row at the top of the Movies and Series grids showing in-progress titles with one-keypress resume.

**Architecture:** Three layers — (1) extend the `watchhistory.Entry` data model with season/episode fields; (2) a new `continue_watching.go` file in the `ui` package that owns all CW rendering and query helpers; (3) wire `cwCursor`/`cwFocused` state into the existing `Model` in `ui.go` for navigation and key handling. A small `IsAtTopRow()` accessor is added to `GridCursor` so `ui.go` can check grid position without accessing unexported fields.

**Tech Stack:** Go 1.22, Bubble Tea, Lip Gloss, `pkg/watchhistory`, `internal/ipc`, `internal/ui/components`

**Spec:** `docs/superpowers/specs/2026-03-16-continue-watching-design.md`

---

## Chunk 1: Data model — watchhistory.Entry + episode helpers

### Task 1: Add Season/Episode fields to watchhistory.Entry

**Files:**
- Modify: `tui/pkg/watchhistory/history.go`
- Create: `tui/pkg/watchhistory/history_test.go`

- [ ] **Step 1: Write the failing tests**

Create `tui/pkg/watchhistory/history_test.go`:

```go
package watchhistory_test

import (
	"testing"

	"github.com/stui/stui/pkg/watchhistory"
)

func TestEntryHasSeasonEpisodeFields(t *testing.T) {
	e := watchhistory.Entry{
		ID:      "tt1234",
		Title:   "Breaking Bad",
		Season:  3,
		Episode: 5,
	}
	if e.Season != 3 {
		t.Errorf("Season: want 3, got %d", e.Season)
	}
	if e.Episode != 5 {
		t.Errorf("Episode: want 5, got %d", e.Episode)
	}
}

func TestParseEpisodeInfo(t *testing.T) {
	cases := []struct {
		title   string
		season  int
		episode int
	}{
		{"Breaking Bad S03E05", 3, 5},
		{"The Bear s2e1", 2, 1},
		{"Some Movie", 0, 0},
		{"Show S1E10 Finale", 1, 10},
		{"No pattern here", 0, 0},
	}
	for _, tc := range cases {
		s, e := watchhistory.ParseEpisodeInfo(tc.title)
		if s != tc.season || e != tc.episode {
			t.Errorf("ParseEpisodeInfo(%q): want (%d,%d), got (%d,%d)",
				tc.title, tc.season, tc.episode, s, e)
		}
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./pkg/watchhistory/...
```
Expected: compile error — `Season`/`Episode` fields and `ParseEpisodeInfo` don't exist yet.

- [ ] **Step 3: Add Season/Episode to Entry struct**

In `tui/pkg/watchhistory/history.go`, find the `Entry` struct and add two fields after `LastWatched`:

```go
Season  int `json:"season,omitempty"`   // 0 = unknown
Episode int `json:"episode,omitempty"`  // 0 = unknown
```

- [ ] **Step 4: Add ParseEpisodeInfo — merge imports and add function**

`history.go` already imports `encoding/json`, `os`, `path/filepath`, `sort`, `time`. Add `"fmt"` and `"regexp"` to that existing import block (do not add a second `import` declaration). Then add `ParseEpisodeInfo` at the bottom of the file:

```go
// episodeRe matches patterns like S03E05, s2e1, S1E10.
var episodeRe = regexp.MustCompile(`(?i)[Ss](\d+)[Ee](\d+)`)

// ParseEpisodeInfo extracts season and episode numbers from a title string.
// Returns (0, 0) if no SnnEnn pattern is found.
func ParseEpisodeInfo(title string) (season, episode int) {
	m := episodeRe.FindStringSubmatch(title)
	if m == nil {
		return 0, 0
	}
	fmt.Sscanf(m[1], "%d", &season)
	fmt.Sscanf(m[2], "%d", &episode)
	return season, episode
}
```

- [ ] **Step 5: Run tests — verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./pkg/watchhistory/... -v
```
Expected: `PASS` — both `TestEntryHasSeasonEpisodeFields` and `TestParseEpisodeInfo` pass.

- [ ] **Step 6: Confirm build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```
Expected: no errors.

---

### Task 2: Add IsAtTopRow() accessor to GridCursor

`GridCursor` has unexported `row`/`col` fields. The `ui` package cannot read them directly. Add a single accessor so `ui.go` can check whether the cursor is in the first grid row.

**Files:**
- Modify: `tui/internal/ui/screens/grid.go`

- [ ] **Step 1: Add IsAtTopRow to GridCursor**

Find the `GridCursor` struct in `tui/internal/ui/screens/grid.go`. Add the method immediately after the existing `Index` method:

```go
// IsAtTopRow returns true when the cursor is on the first row of the grid.
func (c GridCursor) IsAtTopRow() bool {
	return c.row == 0
}
```

- [ ] **Step 2: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

---

## Chunk 2: CW rendering — continue_watching.go

### Task 3: Create continue_watching.go with helpers and card renderer

**Files:**
- Create: `tui/internal/ui/continue_watching.go`
- Create: `tui/internal/ui/continue_watching_test.go`

- [ ] **Step 1: Write the failing tests**

Create `tui/internal/ui/continue_watching_test.go`:

```go
package ui

import (
	"fmt"
	"strings"
	"testing"

	"github.com/stui/stui/pkg/watchhistory"
)

// ── cwTimeLeft ────────────────────────────────────────────────────────────────

func TestCwTimeLeft(t *testing.T) {
	cases := []struct {
		pos, dur float64
		want     string
	}{
		{3600, 7200, "1h 00m left"},
		{0, 5400, "1h 30m left"},
		{5100, 5400, "5m left"},
		{5400, 5400, "0m left"},
		{3600, 0, ""},
	}
	for _, tc := range cases {
		got := cwTimeLeft(tc.pos, tc.dur)
		if got != tc.want {
			t.Errorf("cwTimeLeft(%v,%v): want %q, got %q", tc.pos, tc.dur, tc.want, got)
		}
	}
}

// ── cwSubtitle ────────────────────────────────────────────────────────────────

func TestCwSubtitle(t *testing.T) {
	cases := []struct {
		entry watchhistory.Entry
		want  string
	}{
		{
			watchhistory.Entry{Tab: "series", Season: 3, Episode: 5, Position: 300, Duration: 3900},
			"S3E5 · 1h 00m left",
		},
		{
			watchhistory.Entry{Tab: "series", Season: 0, Episode: 0, Position: 300, Duration: 3900},
			"Series · 1h 00m left",
		},
		{
			watchhistory.Entry{Tab: "movies", Position: 1800, Duration: 7200},
			"Movie · 1h 30m left",
		},
	}
	for _, tc := range cases {
		got := cwSubtitle(tc.entry)
		if got != tc.want {
			t.Errorf("cwSubtitle(%+v): want %q, got %q", tc.entry, tc.want, got)
		}
	}
}

// ── cwProgressBar ─────────────────────────────────────────────────────────────

func TestCwProgressBarLength(t *testing.T) {
	bar := cwProgressBar(0.5, 1.0, 10)
	filled := strings.Count(bar, "█")
	empty := strings.Count(bar, "░")
	if filled+empty != 10 {
		t.Errorf("expected 10 bar chars, got filled=%d empty=%d bar=%q", filled, empty, bar)
	}
	if filled != 5 {
		t.Errorf("expected 5 filled chars at 50%%, got %d", filled)
	}
}

func TestCwProgressBarFullEmpty(t *testing.T) {
	full := cwProgressBar(1.0, 1.0, 8)
	if strings.Count(full, "░") != 0 {
		t.Errorf("100%% bar should have no empty chars")
	}
	empty := cwProgressBar(0, 1.0, 8)
	if strings.Count(empty, "█") != 0 {
		t.Errorf("0%% bar should have no filled chars")
	}
}

// ── cwItems ───────────────────────────────────────────────────────────────────

func TestCwItems(t *testing.T) {
	// Use Load (not NewStore — Load is the constructor in pkg/watchhistory)
	store := watchhistory.Load("/tmp/test-history-cw.json")
	store.Upsert(watchhistory.Entry{ID: "m1", Tab: "movies", Position: 10, Duration: 100, LastWatched: 200})
	store.Upsert(watchhistory.Entry{ID: "m2", Tab: "movies", Position: 20, Duration: 100, LastWatched: 300})
	store.Upsert(watchhistory.Entry{ID: "s1", Tab: "series", Position: 30, Duration: 100, LastWatched: 100})
	store.Upsert(watchhistory.Entry{ID: "m3", Tab: "movies", Position: 0, Duration: 100})

	got := cwItems(store, "movies")
	if len(got) != 2 {
		t.Fatalf("expected 2 movie items, got %d", len(got))
	}
	if got[0].ID != "m2" {
		t.Errorf("expected m2 first (LastWatched=300), got %s", got[0].ID)
	}
	if got[1].ID != "m1" {
		t.Errorf("expected m1 second (LastWatched=200), got %s", got[1].ID)
	}

	gotSeries := cwItems(store, "series")
	if len(gotSeries) != 1 || gotSeries[0].ID != "s1" {
		t.Errorf("expected 1 series item, got %v", gotSeries)
	}
}

func TestCwItemsCappedAt5(t *testing.T) {
	store := watchhistory.Load("/tmp/test-history-cw-cap.json")
	for i := 0; i < 7; i++ {
		store.Upsert(watchhistory.Entry{
			ID:          fmt.Sprintf("m%d", i),
			Tab:         "movies",
			Position:    10,
			Duration:    100,
			LastWatched: int64(i),
		})
	}
	got := cwItems(store, "movies")
	if len(got) != 5 {
		t.Errorf("expected cap of 5, got %d", len(got))
	}
}

// ── historyEntryToCatalogEntry ────────────────────────────────────────────────

func TestHistoryEntryToCatalogEntry(t *testing.T) {
	e := watchhistory.Entry{
		ID:       "tt1234",
		Title:    "Breaking Bad",
		Year:     "2008",
		Provider: "torrentio",
		ImdbID:   "tt0903747",
		Tab:      "series",
	}
	cat := historyEntryToCatalogEntry(e)
	if cat.ID != "tt1234" {
		t.Errorf("ID mismatch")
	}
	if cat.Title != "Breaking Bad" {
		t.Errorf("Title mismatch")
	}
	if cat.Year == nil || *cat.Year != "2008" {
		t.Errorf("Year mismatch: got %v", cat.Year)
	}
	if cat.ImdbID == nil || *cat.ImdbID != "tt0903747" {
		t.Errorf("ImdbID mismatch: got %v", cat.ImdbID)
	}
	if cat.Provider != "torrentio" {
		t.Errorf("Provider mismatch")
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/... 2>&1 | head -30
```
Expected: compile errors — functions not defined yet.

- [ ] **Step 3: Implement continue_watching.go**

Create `tui/internal/ui/continue_watching.go`:

```go
package ui

// continue_watching.go — Continue Watching row: query helpers, card renderer,
// and row renderer for the in-progress section at the top of Movies/Series grids.

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

const cwMaxItems = 5

// cwItems returns in-progress entries for the given tab, capped at cwMaxItems,
// most-recently-watched first. tabID should be string(ipc.TabMovies) or
// string(ipc.TabSeries).
func cwItems(store *watchhistory.Store, tabID string) []watchhistory.Entry {
	all := store.InProgress()
	var filtered []watchhistory.Entry
	for _, e := range all {
		if e.Tab == tabID {
			filtered = append(filtered, e)
		}
		if len(filtered) == cwMaxItems {
			break
		}
	}
	return filtered
}

// historyEntryToCatalogEntry converts a watchhistory.Entry to ipc.CatalogEntry
// for passing to openDetail. Metadata not stored in history (genre, rating,
// poster) is left nil — the detail screen handles zero values gracefully.
func historyEntryToCatalogEntry(e watchhistory.Entry) ipc.CatalogEntry {
	return ipc.CatalogEntry{
		ID:       e.ID,
		Title:    e.Title,
		Year:     &e.Year,
		Provider: e.Provider,
		ImdbID:   &e.ImdbID,
		Tab:      e.Tab,
	}
}

// cwTimeLeft returns a human-readable "Xh Ym left" or "Xm left" string.
// Returns "" if duration is 0 (unknown).
func cwTimeLeft(position, duration float64) string {
	if duration <= 0 {
		return ""
	}
	remaining := duration - position
	if remaining < 0 {
		remaining = 0
	}
	totalSecs := int(remaining)
	h := totalSecs / 3600
	m := (totalSecs % 3600) / 60
	if h > 0 {
		return fmt.Sprintf("%dh %02dm left", h, m)
	}
	return fmt.Sprintf("%dm left", m)
}

// cwSubtitle returns the second line of a CW card: "S3E5 · 1h left" for series
// with known episode info, or "Series · 1h left" / "Movie · 1h left" fallback.
func cwSubtitle(e watchhistory.Entry) string {
	timeStr := cwTimeLeft(e.Position, e.Duration)
	var typeLabel string
	if e.Tab == string(ipc.TabSeries) {
		if e.Season > 0 && e.Episode > 0 {
			typeLabel = fmt.Sprintf("S%dE%d", e.Season, e.Episode)
		} else {
			typeLabel = "Series"
		}
	} else {
		typeLabel = "Movie"
	}
	if timeStr == "" {
		return typeLabel
	}
	return typeLabel + " · " + timeStr
}

// cwProgressBar returns a fixed-width progress bar string using block characters.
// w is the desired total character width.
func cwProgressBar(position, duration float64, w int) string {
	if w <= 0 {
		return ""
	}
	fraction := 0.0
	if duration > 0 {
		fraction = position / duration
		if fraction > 1 {
			fraction = 1
		}
		if fraction < 0 {
			fraction = 0
		}
	}
	filled := int(float64(w) * fraction)
	empty := w - filled
	bar := strings.Repeat("█", filled) + strings.Repeat("░", empty)
	return lipgloss.NewStyle().Foreground(theme.T.Accent()).Render(bar)
}

// renderContinueWatchingCard renders a single CW card. It reuses the same
// poster placeholder renderer as the main grid but replaces the bottom lines
// with subtitle + progress bar.
func renderContinueWatchingCard(e watchhistory.Entry, w int, selected bool) string {
	poster := components.RenderPosterPlaceholder(e.Title, "", w, components.CardPosterRows)

	title := components.Truncate(e.Title, w-2)
	titleLine := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Bold(true).
		Width(w).
		Render(title)

	sub := components.Truncate(cwSubtitle(e), w-2)
	subLine := lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		Width(w).
		Render(sub)

	barLine := cwProgressBar(e.Position, e.Duration, w)

	content := lipgloss.JoinVertical(lipgloss.Left, poster, titleLine, subLine, barLine)

	borderColor := theme.T.Border()
	if selected {
		borderColor = theme.T.Accent()
	}
	return lipgloss.NewStyle().
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(borderColor).
		Padding(0, 1).
		Width(w).
		Render(content)
}

// renderContinueWatchingRow renders the full Continue Watching section:
// a header line and a row of up to cwMaxItems cards.
// cursor is the index of the selected card (0-based).
// focused = true draws the selected card with the accent border.
func renderContinueWatchingRow(entries []watchhistory.Entry, cursor int, focused bool, termWidth int) string {
	if len(entries) == 0 {
		return ""
	}
	fill := max(0, termWidth-24)
	header := lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		Render(" ─── Continue Watching " + strings.Repeat("─", fill))

	cw := components.CardWidth(termWidth)
	var cards []string
	for i, e := range entries {
		cards = append(cards, renderContinueWatchingCard(e, cw, focused && i == cursor))
	}
	return lipgloss.JoinVertical(lipgloss.Left, header, lipgloss.JoinHorizontal(lipgloss.Top, cards...))
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/... -run "TestCw|TestHistory" -v
```
Expected: all pass.

- [ ] **Step 5: Confirm full build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```
Expected: no errors.

---

## Chunk 3: Model integration — ui.go wiring

### Task 4: Add CW state to Model, update switchTab, populate Season/Episode

**Files:**
- Modify: `tui/internal/ui/ui.go`

- [ ] **Step 1: Add cwCursor and cwFocused to the Model struct**

Find the `// Watch history` section in the Model struct (around line 121). After `nowPlayingEntry watchhistory.Entry`, add:

```go
// Continue Watching
cwCursor  int  // index of selected card in the CW row
cwFocused bool // true when cursor is in the CW row (not the main grid)
```

- [ ] **Step 2: Update switchTab to reset CW state**

Find `func (m *Model) switchTab(t state.Tab)` (around line 1862). After `m.gridCursor = screens.GridCursor{}`, add:

```go
m.cwCursor = 0
// Set cwFocused if the new tab has in-progress items
if m.historyStore != nil && cwTabActive(t) &&
	len(cwItems(m.historyStore, t.MediaTabID())) > 0 {
	m.cwFocused = true
} else {
	m.cwFocused = false
}
```

- [ ] **Step 3: Populate Season/Episode in nowPlayingEntry**

Find the detail play path where `m.nowPlayingEntry` is constructed (around line 1645). Replace the struct literal with:

```go
season, episode := watchhistory.ParseEpisodeInfo(ds.Entry.Title)
m.nowPlayingEntry = watchhistory.Entry{
	ID:       ds.Entry.ID,
	Title:    ds.Entry.Title,
	Year:     ds.Entry.Year,
	Tab:      ds.Entry.Tab,
	Provider: provider,
	ImdbID:   ds.Entry.ImdbID,
	Season:   season,
	Episode:  episode,
}
```

Find the binge play path (around line 1041). Replace that struct literal with:

```go
season, episode := watchhistory.ParseEpisodeInfo(title)
m.nowPlayingEntry = watchhistory.Entry{
	ID:       ep.EntryID,
	Title:    title,
	Tab:      string(m.bingeCtx.Tab),
	Provider: ep.Provider,
	Season:   season,
	Episode:  episode,
}
```

- [ ] **Step 4: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

---

### Task 5: Render the CW row in viewMain()

**Files:**
- Modify: `tui/internal/ui/ui.go`

- [ ] **Step 1: Add cwTabActive helper**

Add to `continue_watching.go` (already in the `ui` package):

```go
// cwTabActive returns true for tabs that should show the Continue Watching row.
func cwTabActive(tab state.Tab) bool {
	return tab == state.TabMovies || tab == state.TabSeries
}
```

Add the `"github.com/stui/stui/internal/state"` import to `continue_watching.go` if not already present.

- [ ] **Step 2: Add a cwCurrentItems method to Model**

Add to `continue_watching.go`:

```go
// cwCurrentItems returns CW items for the active tab, or nil if not applicable.
func (m *Model) cwCurrentItems() []watchhistory.Entry {
	if m.historyStore == nil || !cwTabActive(m.state.ActiveTab) {
		return nil
	}
	return cwItems(m.historyStore, m.state.ActiveTab.MediaTabID())
}
```

- [ ] **Step 3: Modify viewMain() to prepend the CW row**

Find `viewMain()` in `ui.go`. Locate the `screens.RenderGrid(...)` call. Just before it, add the CW section block and join it:

```go
// Continue Watching row (Movies and Series tabs only)
var cwSection string
if m.historyStore != nil && cwTabActive(m.state.ActiveTab) {
	items := cwItems(m.historyStore, m.state.ActiveTab.MediaTabID())
	if len(items) > 0 {
		cwSection = renderContinueWatchingRow(items, m.cwCursor, m.cwFocused, m.state.Width)
	}
}

grid := screens.RenderGrid(
	m.currentGridEntries(),
	m.gridCursor,
	m.state.Width,
	m.state.Height,
	m.state.IsLoading,
	m.state.RuntimeStatus.String(),
)

if cwSection != "" {
	return lipgloss.JoinVertical(lipgloss.Left, cwSection, grid)
}
return grid
```

- [ ] **Step 4: Build and visually verify**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./... && go run ./cmd/stui
```

Navigate to Movies or Series tab. If `~/.config/stui/history.json` has in-progress entries, the CW row should appear at the top. To add a test entry manually:

```json
{"entries":[{"id":"test1","title":"Breaking Bad","tab":"series","provider":"torrentio","position":1800,"duration":3600,"last_watched":1710000000,"season":3,"episode":5}]}
```

---

### Task 6: Wire keyboard handlers

**Files:**
- Modify: `tui/internal/ui/ui.go`

- [ ] **Step 1: Handle ↑ — move from top of main grid into CW row**

In the grid key handler (around line 1349, the `k`/up block), add a CW check at the top before `MoveCursorUp`:

```go
case m.keys.Up.Contains(msg):
	if !m.cwFocused {
		items := m.cwCurrentItems()
		if len(items) > 0 && m.gridCursor.IsAtTopRow() {
			m.cwFocused = true
			if m.cwCursor >= len(items) {
				m.cwCursor = len(items) - 1
			}
			return m, nil
		}
		m.gridCursor = screens.MoveCursorUp(m.gridCursor)
	}
```

Note: `MoveCursorUp` takes only the cursor — no `total` or `cols` argument.

- [ ] **Step 2: Handle ↓ from CW row — drop into main grid**

In the `j`/down block (around line 1345), add a CW check at the top:

```go
case m.keys.Down.Contains(msg):
	if m.cwFocused {
		m.cwFocused = false
		m.gridCursor = screens.GridCursor{}
		return m, nil
	}
	m.gridCursor = screens.MoveCursorDown(m.gridCursor, len(m.currentGridEntries()))
```

Note: `MoveCursorDown` takes `(cursor, total)` — no `cols` argument.

- [ ] **Step 3: Handle ← and → within CW row**

In the `h`/left and `l`/right blocks, add CW branches:

```go
case m.keys.Left.Contains(msg):
	if m.cwFocused {
		if m.cwCursor > 0 {
			m.cwCursor--
		}
		return m, nil
	}
	m.gridCursor = screens.MoveCursorLeft(m.gridCursor)

case m.keys.Right.Contains(msg):
	if m.cwFocused {
		items := m.cwCurrentItems()
		if m.cwCursor < len(items)-1 {
			m.cwCursor++
		}
		return m, nil
	}
	m.gridCursor = screens.MoveCursorRight(m.gridCursor, len(m.currentGridEntries()))
```

Note: `MoveCursorLeft` takes only the cursor; `MoveCursorRight` takes `(cursor, total)`.

- [ ] **Step 4: Handle Enter — resume playback**

In the enter block (around line 1353), add a CW branch before the existing grid Enter logic:

```go
case m.keys.Enter.Contains(msg):
	if m.cwFocused {
		items := m.cwCurrentItems()
		if len(items) == 0 || m.cwCursor >= len(items) {
			return m, nil
		}
		entry := items[m.cwCursor]
		if entry.Provider == "" {
			return m, m.openDetail(historyEntryToCatalogEntry(entry))
		}
		tab := ipc.MediaTab(m.state.ActiveTab.MediaTabID())
		m.nowPlayingEntryID = entry.ID
		season, episode := watchhistory.ParseEpisodeInfo(entry.Title)
		m.nowPlayingEntry = watchhistory.Entry{
			ID:       entry.ID,
			Title:    entry.Title,
			Year:     entry.Year,
			Tab:      entry.Tab,
			Provider: entry.Provider,
			ImdbID:   entry.ImdbID,
			Season:   season,
			Episode:  episode,
		}
		m.historyStore.Upsert(m.nowPlayingEntry)
		m.state.StatusMsg = fmt.Sprintf("Resuming %s from %s…",
			entry.Title, formatDurationHMS(entry.Position))
		m.client.PlayFrom(entry.ID, entry.Provider, entry.ImdbID, tab, entry.Position)
		return m, nil
	}
	// ... existing grid Enter logic continues ...
```

- [ ] **Step 5: Handle `i` — open detail from CW row**

Find the `i` key handler in the grid section. Add a CW branch at the top:

```go
case msg.String() == "i":
	if m.cwFocused {
		items := m.cwCurrentItems()
		if len(items) == 0 || m.cwCursor >= len(items) {
			return m, nil
		}
		return m, m.openDetail(historyEntryToCatalogEntry(items[m.cwCursor]))
	}
	// ... existing detail open logic ...
```

- [ ] **Step 6: Handle `d` — remove from Continue Watching**

Find the `d` key handler (or add a new case in the grid key section):

```go
case msg.String() == "d":
	if m.cwFocused && m.historyStore != nil {
		items := m.cwCurrentItems()
		if len(items) == 0 || m.cwCursor >= len(items) {
			return m, nil
		}
		m.historyStore.Remove(items[m.cwCursor].ID)
		go func() { _ = m.historyStore.Save() }()
		newItems := m.cwCurrentItems()
		if len(newItems) == 0 {
			m.cwFocused = false
		} else if m.cwCursor >= len(newItems) {
			m.cwCursor = len(newItems) - 1
		}
		return m, nil
	}
	// ... existing d handler (if any) continues ...
```

- [ ] **Step 7: Handle Esc — clear CW focus**

Find the Esc handler. Add at the very top of its block:

```go
case key.Matches(msg, m.keys.Escape):
	if m.cwFocused {
		m.cwFocused = false
		return m, nil
	}
	// ... existing Esc logic ...
```

- [ ] **Step 8: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

- [ ] **Step 9: Full smoke test**

```bash
go run ./cmd/stui
```

Verify in order:
1. Movies/Series tab shows CW row at top when history has in-progress entries
2. `←`/`→` navigate within the CW row (accent border moves)
3. `↓` from CW row moves cursor to top of main grid
4. `↑` from top of main grid moves cursor back into CW row
5. `Enter` on a CW item shows "Resuming…" in status bar and starts playback
6. `i` on a CW item opens the detail overlay
7. `d` removes the item; row disappears when all items removed
8. Switch tabs — CW row reloads for new tab; cursor in CW row if items exist

- [ ] **Step 10: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```
Expected: all pass.
