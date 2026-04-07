# Music Queue Sub-Tab Redesign — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign Music→Queue sub-tab with two-column layout (track list + now-playing panel), column headers, inline seek/volume controls, and inline visualizer.

**Architecture:** Three self-contained changes: (1) new fields + key bindings in `MusicQueueScreen`, (2) full `View()` rewrite with helpers extracted for testability, (3) visualizer wiring through `MusicScreen.SetVisualizer` and global suppression in `ui.go`.

**Tech Stack:** Go, charm.land/bubbletea/v2, charm.land/lipgloss/v2, internal ipc package, internal components.Visualizer

---

## Background: Codebase Orientation

**Module:** `github.com/stui/stui` (root: `tui/`)

**Key types:**
- `MusicQueueScreen` — value type in `internal/ui/screens/music_queue.go`. `Update(msg tea.Msg) (MusicQueueScreen, tea.Cmd)` returns updated copy. `View(w, h int) string` renders to string.
- `MusicScreen` — container in `music_screen.go`. Owns `queue MusicQueueScreen`. Its `Update()` fans out messages to all sub-screens via `default:` case.
- `components.Visualizer` — pointer type in `internal/ui/components/visualizer.go`. `IsRunning() bool`, `RenderBars(width int) string`, `Config() VisualizerConfig` (has `.Height int`).
- `ipc.MpdStatusMsg` — fields: `Elapsed float64`, `Duration float64`, `Volume uint32`, `SongID int32`, `SongPos int32`.

**Existing fields on `MusicQueueScreen`** (do not duplicate):
```go
nowTitle   string
nowArtist  string
nowSongID  int32
nowSongPos int32
```

**IPC command for volume** (confirmed in `ui.go:1602`):
```go
client.MpdCmd("mpd_set_volume", map[string]any{"volume": vol})
```

**Tests:** In `package screens` (white-box, same package as source). Run with:
```bash
cd tui && go test ./internal/ui/screens/... -v
```

---

## File Structure

| File | Role |
|------|------|
| `internal/ui/screens/music_queue.go` | Main change: new fields, extended Update(), full View() rewrite |
| `internal/ui/screens/music_queue_test.go` | New file: tests for helpers and key-binding logic |
| `internal/ui/screens/music_screen.go` | Add `SetVisualizer` method (~5 lines) |
| `internal/ui/ui.go` | Call SetVisualizer after init; wrap global viz render with suppression guard |

---

## Chunk 1: Fields, State, and Key Bindings

### Task 1: Add new fields to `MusicQueueScreen`

**Files:**
- Modify: `internal/ui/screens/music_queue.go:33-44` (the struct definition)

- [ ] **Step 1: Add the new fields to the struct**

In `music_queue.go`, find the struct definition starting at line 33. Add after the `spinner components.Spinner` field:

```go
// Now-playing state from MpdStatusMsg
nowElapsed  float64
nowDuration float64
nowVolume   uint32
prevVolume  uint32 // saved before local mute toggle
nowMuted    bool

// Visualizer reference — set by MusicScreen.SetVisualizer
visualizer *components.Visualizer
```

The import for `components` is already present (`"github.com/stui/stui/internal/ui/components"`).

- [ ] **Step 2: Build to confirm no compile errors**

```bash
cd tui && go build ./internal/ui/screens/...
```
Expected: no output (success).

- [ ] **Step 3: Commit**

```bash
cd tui && git add internal/ui/screens/music_queue.go
git commit -m "feat(queue): add now-playing state fields and visualizer pointer"
```

---

### Task 2: Extend `MpdStatusMsg` handler and add new key bindings

**Files:**
- Modify: `internal/ui/screens/music_queue.go` — Update() function
- Create: `internal/ui/screens/music_queue_test.go`

- [ ] **Step 1: Write failing tests**

Create `internal/ui/screens/music_queue_test.go`:

```go
package screens

import (
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/stui/stui/internal/ipc"
)

// helper: new queue screen with a known track loaded and playing
func queueWithTrack() MusicQueueScreen {
	s := NewMusicQueueScreen(nil)
	s.tracks = []ipc.MpdTrack{
		{ID: 5, Pos: 0, Title: "Cornish Acid", Artist: "Aphex Twin", Album: "RDJ Album", Duration: 214},
	}
	s.nowSongID = 5
	s.nowDuration = 214
	s.nowElapsed = 63
	s.nowVolume = 72
	s.prevVolume = 100
	return s
}

// MpdStatusMsg captures Elapsed, Duration, Volume
func TestQueueStatusMsgCapturesFields(t *testing.T) {
	s := NewMusicQueueScreen(nil)
	msg := ipc.MpdStatusMsg{
		SongTitle:  "Cornish Acid",
		SongArtist: "Aphex Twin",
		SongID:     5,
		Elapsed:    63.0,
		Duration:   214.0,
		Volume:     72,
	}
	s2, _ := s.Update(msg)
	if s2.nowElapsed != 63.0 {
		t.Errorf("nowElapsed = %v, want 63.0", s2.nowElapsed)
	}
	if s2.nowDuration != 214.0 {
		t.Errorf("nowDuration = %v, want 214.0", s2.nowDuration)
	}
	if s2.nowVolume != 72 {
		t.Errorf("nowVolume = %v, want 72", s2.nowVolume)
	}
}

// External volume-up clears nowMuted
func TestQueueStatusMsgClearsMuteOnVolumeUp(t *testing.T) {
	s := queueWithTrack()
	s.nowMuted = true
	s.nowVolume = 0
	msg := ipc.MpdStatusMsg{Volume: 50, SongID: 5}
	s2, _ := s.Update(msg)
	if s2.nowMuted {
		t.Error("nowMuted should be cleared when external volume > 0")
	}
}

// Key "0" mutes when not muted
func TestQueueMuteKeyMutes(t *testing.T) {
	s := queueWithTrack()
	s2, _ := s.Update(tea.KeyPressMsg{Text: "0"})
	if !s2.nowMuted {
		t.Error("pressing 0 should set nowMuted=true")
	}
	if s2.prevVolume != 72 {
		t.Errorf("prevVolume = %v, want 72", s2.prevVolume)
	}
}

// Key "0" unmutes when already muted
func TestQueueMuteKeyUnmutes(t *testing.T) {
	s := queueWithTrack()
	s.nowMuted = true
	s.nowVolume = 0
	s.prevVolume = 72
	s2, _ := s.Update(tea.KeyPressMsg{Text: "0"})
	if s2.nowMuted {
		t.Error("pressing 0 when muted should set nowMuted=false")
	}
}

// Muting when volume already 0 externally: treat as mute (save prevVolume=0)
func TestQueueMuteKeyWhenAlreadyZero(t *testing.T) {
	s := queueWithTrack()
	s.nowVolume = 0
	s.nowMuted = false
	s2, _ := s.Update(tea.KeyPressMsg{Text: "0"})
	if !s2.nowMuted {
		t.Error("pressing 0 when volume=0 and not muted should set nowMuted=true")
	}
	if s2.prevVolume != 0 {
		t.Errorf("prevVolume = %v, want 0", s2.prevVolume)
	}
}

// Key "<" does nothing when nowDuration == 0
func TestQueueSeekBackNoopWhenNoDuration(t *testing.T) {
	s := NewMusicQueueScreen(nil)
	s.nowDuration = 0
	s.nowElapsed = 0
	_, cmd := s.Update(tea.KeyPressMsg{Text: "<"})
	if cmd != nil {
		t.Error("seek < should be a no-op when nowDuration == 0")
	}
}

// Key ">" does nothing when nowDuration == 0
func TestQueueSeekFwdNoopWhenNoDuration(t *testing.T) {
	s := NewMusicQueueScreen(nil)
	s.nowDuration = 0
	_, cmd := s.Update(tea.KeyPressMsg{Text: ">"})
	if cmd != nil {
		t.Error("seek > should be a no-op when nowDuration == 0")
	}
}
```

- [ ] **Step 2: Run to confirm tests fail**

```bash
cd tui && go test ./internal/ui/screens/... -run TestQueue -v 2>&1 | head -40
```
Expected: compilation error or FAIL — fields not yet used in Update().

- [ ] **Step 3: Extend `MpdStatusMsg` case in `Update()`**

In `music_queue.go`, find the `case ipc.MpdStatusMsg:` block (around line 122). It currently sets `nowTitle`, `nowArtist`, `nowSongID`, `nowSongPos`. Add after those lines:

```go
s.nowElapsed  = m.Elapsed
s.nowDuration = m.Duration
s.nowVolume   = m.Volume
// External volume change clears local mute state
if s.nowMuted && m.Volume > 0 {
    s.nowMuted = false
}
```

- [ ] **Step 4: Add new key bindings in `Update()`**

In `music_queue.go`, inside the `case tea.KeyPressMsg:` block, add new cases after the existing `"G"` case and before the closing `}`. The queue screen has a `client *ipc.Client` field — use it for IPC calls:

```go
case "0":
    if s.nowMuted {
        // unmute: restore saved volume
        if s.client != nil {
            s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": int(s.prevVolume)})
        }
        s.nowMuted = false
    } else {
        // mute: save current volume (even if 0)
        s.prevVolume = s.nowVolume
        if s.client != nil {
            s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": 0})
        }
        s.nowMuted = true
    }

case "<":
    if s.nowDuration > 0 && s.client != nil {
        t := s.nowElapsed - 5
        if t < 0 {
            t = 0
        }
        s.client.MpdCmd("mpd_seek", map[string]any{"id": s.nowSongID, "time": t})
    }

case ">":
    if s.nowDuration > 0 && s.client != nil {
        t := s.nowElapsed + 5
        if t > s.nowDuration {
            t = s.nowDuration
        }
        s.client.MpdCmd("mpd_seek", map[string]any{"id": s.nowSongID, "time": t})
    }
```

- [ ] **Step 5: Run tests — expect pass**

```bash
cd tui && go test ./internal/ui/screens/... -run TestQueue -v
```
Expected: all 7 TestQueue* tests PASS.

- [ ] **Step 6: Build check**

```bash
cd tui && go build ./...
```
Expected: no output.

- [ ] **Step 7: Commit**

```bash
cd tui && git add internal/ui/screens/music_queue.go internal/ui/screens/music_queue_test.go
git commit -m "feat(queue): extend MpdStatusMsg handler, add mute/seek key bindings"
```

---

## Chunk 2: View Helpers and Wide Layout

### Task 3: Column width helper

**Files:**
- Modify: `internal/ui/screens/music_queue.go` — add `queueColWidths` helper
- Modify: `internal/ui/screens/music_queue_test.go` — add column width tests

The column logic is needed by both the header row and track rows. Extract it into a pure function.

- [ ] **Step 1: Write failing tests**

Add to `music_queue_test.go`:

```go
// queueColWidths(L) returns (titleW, artistW, albumW) where albumW==0 means no album column.
// Fixed overhead: prefix 3 + # 3 + space 1 + dur 6 = 13. Remaining R = L - 13.
// Wide (L>=120): title=R*40/100, artist=R*35/100, album=R*25/100, remainder to title.
// Narrow (L<120): title=R*55/100, artist=R*45/100, album=0, remainder to title.

func TestQueueColWidthsNarrow(t *testing.T) {
	// L=100, R=87: title=47 (87*55/100=47 rem 85), artist=39 (87*45/100=39 rem 15)
	// remainder = 87 - 47 - 39 = 1 goes to title → title=48
	tw, aw, alw := queueColWidths(100)
	if alw != 0 {
		t.Errorf("albumW = %d, want 0 for narrow layout", alw)
	}
	if tw+aw != 87 {
		t.Errorf("titleW(%d)+artistW(%d) = %d, want 87", tw, aw, tw+aw)
	}
	_ = tw
	_ = aw
}

func TestQueueColWidthsWide(t *testing.T) {
	// L=120, R=107: title=42, artist=37, album=26, rem=2 → title=44
	tw, aw, alw := queueColWidths(120)
	if alw == 0 {
		t.Error("albumW should be > 0 for L=120")
	}
	if tw+aw+alw != 107 {
		t.Errorf("column widths sum %d, want 107", tw+aw+alw)
	}
}

func TestQueueColWidthsExact143Terminal(t *testing.T) {
	// terminal width=143 → L=143-23=120, triggers wide layout
	L := 143 - 23
	_, _, alw := queueColWidths(L)
	if alw == 0 {
		t.Errorf("album column should appear at L=%d (terminal width 143)", L)
	}
}

func TestQueueColWidthsBelowThreshold(t *testing.T) {
	// L=119: narrow layout
	_, _, alw := queueColWidths(119)
	if alw != 0 {
		t.Errorf("album column should not appear at L=119, got albumW=%d", alw)
	}
}
```

- [ ] **Step 2: Run to confirm fail**

```bash
cd tui && go test ./internal/ui/screens/... -run TestQueueCol -v 2>&1 | head -20
```
Expected: compile error — `queueColWidths` undefined.

- [ ] **Step 3: Implement `queueColWidths`**

Add this function to `music_queue.go` (above or below `View()`):

```go
// queueColWidths returns (titleW, artistW, albumW) for the track list columns
// given left-panel width L. albumW == 0 means the Album column is hidden.
// Fixed overhead = 13ch (prefix 3 + # 3 + space 1 + duration 6).
func queueColWidths(L int) (titleW, artistW, albumW int) {
	R := L - 13
	if R < 1 {
		R = 1
	}
	if L >= 120 {
		titleW  = R * 40 / 100
		artistW = R * 35 / 100
		albumW  = R * 25 / 100
		// remainder goes to title
		titleW += R - titleW - artistW - albumW
	} else {
		titleW  = R * 55 / 100
		artistW = R * 45 / 100
		albumW  = 0
		titleW += R - titleW - artistW
	}
	return
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cd tui && go test ./internal/ui/screens/... -run TestQueueCol -v
```
Expected: all 4 TestQueueCol* PASS.

- [ ] **Step 5: Commit**

```bash
cd tui && git add internal/ui/screens/music_queue.go internal/ui/screens/music_queue_test.go
git commit -m "feat(queue): add queueColWidths helper with adaptive album column"
```

---

### Task 4: Right-panel component helpers

**Files:**
- Modify: `internal/ui/screens/music_queue.go` — add three pure helpers
- Modify: `internal/ui/screens/music_queue_test.go` — tests for each helper

- [ ] **Step 1: Write failing tests**

Add to `music_queue_test.go`:

```go
// ── Art placeholder ────────────────────────────────────────────────────

func TestQueueArtPlaceholderIs9Rows(t *testing.T) {
	lines := strings.Split(strings.TrimRight(queueArtPlaceholder(), "\n"), "\n")
	if len(lines) != 9 {
		t.Errorf("art placeholder has %d rows, want 9", len(lines))
	}
}

func TestQueueArtPlaceholderContainsMusicNote(t *testing.T) {
	out := queueArtPlaceholder()
	if !strings.Contains(out, "♪") {
		t.Error("art placeholder should contain ♪")
	}
}

// ── Seek bar ───────────────────────────────────────────────────────────

func TestQueueSeekBarZeroDuration(t *testing.T) {
	bar, times := queueSeekBar(0, 0)
	for _, ch := range bar {
		if ch != '─' {
			t.Errorf("seek bar with duration=0 should be all ─, got %q", bar)
			break
		}
	}
	if !strings.Contains(times, "0:00") {
		t.Errorf("seek bar times %q should contain 0:00", times)
	}
}

func TestQueueSeekBarLength20(t *testing.T) {
	bar, _ := queueSeekBar(63, 214)
	// strip ANSI — count runes that are bar chars
	count := 0
	for _, r := range bar {
		if r == '━' || r == '╸' || r == '─' {
			count++
		}
	}
	if count != 20 {
		t.Errorf("seek bar has %d bar chars, want 20", count)
	}
}

func TestQueueSeekBarCursorChar(t *testing.T) {
	bar, _ := queueSeekBar(63, 214)
	if !strings.ContainsRune(bar, '╸') {
		t.Errorf("seek bar %q should contain ╸ (U+2578)", bar)
	}
}

func TestQueueSeekBarFullProgress(t *testing.T) {
	// elapsed == duration: filled=19, cursor at pos 19
	bar, _ := queueSeekBar(214, 214)
	if !strings.ContainsRune(bar, '╸') {
		t.Errorf("full seek bar should still have ╸")
	}
}

// ── Volume bar ─────────────────────────────────────────────────────────

func TestQueueVolumeBar72(t *testing.T) {
	bar, hint := queueVolumeBar(72, false)
	if !strings.Contains(bar, "72%") {
		t.Errorf("volume bar %q should contain 72%%", bar)
	}
	if !strings.Contains(hint, "mute") {
		t.Errorf("hint %q should contain 'mute' when not muted", hint)
	}
}

func TestQueueVolumeBarMuted(t *testing.T) {
	_, hint := queueVolumeBar(0, true)
	if !strings.Contains(hint, "unmute") {
		t.Errorf("hint %q should contain 'unmute' when muted", hint)
	}
}

func TestQueueVolumeBar100(t *testing.T) {
	bar, _ := queueVolumeBar(100, false)
	// 10 filled blocks
	filled := strings.Count(bar, "▮")
	if filled != 10 {
		t.Errorf("volume=100 should have 10 filled blocks, got %d", filled)
	}
	empty := strings.Count(bar, "▯")
	if empty != 0 {
		t.Errorf("volume=100 should have 0 empty blocks, got %d", empty)
	}
}

func TestQueueVolumeBarZero(t *testing.T) {
	bar, _ := queueVolumeBar(0, false)
	filled := strings.Count(bar, "▮")
	if filled != 0 {
		t.Errorf("volume=0 should have 0 filled blocks, got %d", filled)
	}
}
```

Add `"strings"` to the imports at the top of the test file.

- [ ] **Step 2: Run to confirm fail**

```bash
cd tui && go test ./internal/ui/screens/... -run "TestQueueArt|TestQueueSeek|TestQueueVol" -v 2>&1 | head -20
```
Expected: compile error — helpers undefined.

- [ ] **Step 3: Implement the three helpers**

Add to `music_queue.go`:

```go
// queueArtPlaceholder returns a fixed 9-row art placeholder box (20ch wide).
func queueArtPlaceholder() string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.TextDim()).
		Width(18).
		Height(7).
		Align(lipgloss.Center, lipgloss.Center)
	return boxStyle.Render(dim.Render("♪")) + "\n"
}

// queueSeekBar returns (barRow, timeRow) for the progress display.
// barRow is 20 chars of ━/╸/─. timeRow shows elapsed and total, padded to 20ch.
func queueSeekBar(elapsed, duration float64) (barRow, timeRow string) {
	const w = 20
	// When duration == 0, return all dashes (no cursor tip)
	if duration <= 0 {
		barRow  = strings.Repeat("─", w)
		timeRow = "0:00" + strings.Repeat(" ", w-8) + "0:00"
		return
	}
	filled := int(elapsed / duration * w)
	if filled > w-1 {
		filled = w - 1
	}
	var b strings.Builder
	for i := 0; i < w; i++ {
		switch {
		case i < filled:
			b.WriteRune('━')
		case i == filled:
			b.WriteRune('╸')
		default:
			b.WriteRune('─')
		}
	}
	barRow = b.String()

	elStr := fmtMusicDuration(elapsed)
	totStr := fmtMusicDuration(duration)
	pad := w - len(elStr) - len(totStr)
	if pad < 1 {
		pad = 1
	}
	timeRow = elStr + strings.Repeat(" ", pad) + totStr
	return
}

// queueVolumeBar returns (barRow, hintRow) for the volume display.
func queueVolumeBar(volume uint32, muted bool) (barRow, hintRow string) {
	filled := int(volume / 10)
	empty  := 10 - filled
	bar := strings.Repeat("▮", filled) + strings.Repeat("▯", empty)
	barRow = fmt.Sprintf("%s  %d%%", bar, volume)
	if muted {
		hintRow = "+ vol  - vol  0 unmute"
	} else {
		hintRow = "+ vol  - vol  0 mute"
	}
	return
}
```

Ensure `"strings"` and `"fmt"` are already imported (both are). `fmtMusicDuration` is an existing function in the same file.

- [ ] **Step 4: Run tests — expect pass**

```bash
cd tui && go test ./internal/ui/screens/... -run "TestQueueArt|TestQueueSeek|TestQueueVol" -v
```
Expected: all PASS.

- [ ] **Step 5: Build check**

```bash
cd tui && go build ./...
```
Expected: no output.

- [ ] **Step 6: Commit**

```bash
cd tui && git add internal/ui/screens/music_queue.go internal/ui/screens/music_queue_test.go
git commit -m "feat(queue): add art placeholder, seek bar, and volume bar helpers"
```

---

### Task 5: Rewrite `View()` — wide two-column layout

**Files:**
- Modify: `internal/ui/screens/music_queue.go` — replace `View()` body
- Modify: `internal/ui/screens/music_queue_test.go` — layout smoke tests

- [ ] **Step 1: Write failing layout tests**

Add to `music_queue_test.go`:

```go
// ── View layout tests ──────────────────────────────────────────────────

func TestQueueViewNarrowNoRightPanel(t *testing.T) {
	s := queueWithTrack()
	out := s.View(80, 20)
	// narrow: no separator │ between track list and right panel
	if strings.Contains(out, "TITLE") {
		t.Error("narrow view (width=80) should not contain right panel TITLE label")
	}
}

func TestQueueViewWideHasRightPanel(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.Contains(out, "TITLE") {
		t.Error("wide view (width=120) should contain right panel TITLE label")
	}
	if !strings.Contains(out, "ARTIST") {
		t.Error("wide view should contain ARTIST label")
	}
}

func TestQueueViewWideHasColumnHeaders(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.Contains(out, "Title") {
		t.Error("wide view should contain Title column header")
	}
	if !strings.Contains(out, "Artist") {
		t.Error("wide view should contain Artist column header")
	}
}

func TestQueueViewWideHasSeekBar(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.ContainsRune(out, '╸') {
		t.Error("wide view should contain seek bar cursor ╸")
	}
}

func TestQueueViewWideHasVolumeBar(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.Contains(out, "▮") {
		t.Error("wide view should contain volume bar filled blocks ▮")
	}
}

func TestQueueViewAlbumColumnAtWidth143(t *testing.T) {
	s := queueWithTrack()
	out := s.View(143, 30)
	if !strings.Contains(out, "Album") {
		t.Error("view at width=143 should show Album column header")
	}
}

func TestQueueViewNoAlbumColumnAtWidth142(t *testing.T) {
	s := queueWithTrack()
	out := s.View(142, 30)
	// L = 142-23 = 119 < 120, no album column
	if strings.Contains(out, "Album") {
		t.Error("view at width=142 (L=119) should NOT show Album column header")
	}
}
```

- [ ] **Step 2: Run to confirm fail**

```bash
cd tui && go test ./internal/ui/screens/... -run "TestQueueView" -v 2>&1 | head -30
```
Expected: FAIL (current View() doesn't have column headers or right panel).

- [ ] **Step 3: Rewrite `View()` in `music_queue.go`**

Replace the entire `View(w, h int) string` function body with the following. The function signature does not change.

```go
func (s MusicQueueScreen) View(w, h int) string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle    := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle   := lipgloss.NewStyle().Foreground(theme.T.Text())
	cursorStyle := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Bold(true)

	footerLine := hintBar("enter play", "d remove", "c clear", "g top", "G bottom", "< seek-", "> seek+", "0 mute")

	// ── Narrow layout (≤80 cols): existing single-column behaviour ────────
	if w <= 80 {
		return s.viewNarrow(w, h, accentStyle, dimStyle, textStyle, cursorStyle, footerLine)
	}

	// ── Wide layout (>80 cols) ─────────────────────────────────────────────
	const rightPanelW = 22
	const sepW        = 1
	L := w - rightPanelW - sepW // left panel width

	// Visualizer height
	vizHeight := 0
	if s.visualizer != nil && s.visualizer.IsRunning() {
		vizHeight = s.visualizer.Config().Height
	}

	// Track list height: h minus header + colheader + footer rows, minus viz
	TH := h - 3 - vizHeight
	if TH < 1 {
		TH = 1
	}

	// ── Header line ───────────────────────────────────────────────────────
	headerText := fmt.Sprintf("Queue (%d tracks · %s)", len(s.tracks), fmtMusicDuration(s.totalDuration()))
	header := accentStyle.Render(headerText)

	// ── Column headers row ────────────────────────────────────────────────
	titleW, artistW, albumW := queueColWidths(L)
	var colHeader string
	if albumW > 0 {
		colHeader = fmt.Sprintf("   %-3s %-*s %-*s %-*s %6s",
			"#",
			titleW,  "Title",
			artistW, "Artist",
			albumW,  "Album",
			"Dur",
		)
	} else {
		colHeader = fmt.Sprintf("   %-3s %-*s %-*s %6s",
			"#",
			titleW,  "Title",
			artistW, "Artist",
			"Dur",
		)
	}
	colHeader = dimStyle.Render(colHeader)

	// ── Track list (virtualized) ──────────────────────────────────────────
	vl := components.NewVirtualizedList(
		len(s.tracks),
		s.cursor,
		TH,
		components.WithScrollMode(components.ScrollModeCenter),
	)
	start, end := vl.VisibleRange()
	scrollbar := vl.VerticalScrollbar(1, dimStyle)

	var trackLines []string
	for i := start; i < end; i++ {
		t := s.tracks[i]
		isCurrent := s.isCurrentTrack(t)
		isCursor  := i == s.cursor

		prefix := "   "
		if isCurrent {
			prefix = "▶  "
		}

		posStr  := fmt.Sprintf("%3d", t.Pos+1)
		durStr  := fmt.Sprintf("%6s", fmtMusicDuration(t.Duration))
		titleStr  := truncate(t.Title,  titleW)
		artistStr := truncate(t.Artist, artistW)

		var line string
		if albumW > 0 {
			albumStr := truncate(t.Album, albumW)
			line = fmt.Sprintf("%s%s %-*s %-*s %-*s %s",
				prefix, posStr,
				titleW,  titleStr,
				artistW, artistStr,
				albumW,  albumStr,
				durStr,
			)
		} else {
			line = fmt.Sprintf("%s%s %-*s %-*s %s",
				prefix, posStr,
				titleW,  titleStr,
				artistW, artistStr,
				durStr,
			)
		}

		var style lipgloss.Style
		switch {
		case isCurrent:
			style = accentStyle
		case isCursor:
			style = cursorStyle
		default:
			style = textStyle
		}
		trackLines = append(trackLines, style.Render(line))
	}
	// Pad to TH
	for len(trackLines) < TH {
		trackLines = append(trackLines, "")
	}

	// ── Right panel lines ─────────────────────────────────────────────────
	availPanelH := h - 3 - vizHeight
	rightLines  := s.buildRightPanel(availPanelH)
	// Pad right panel to TH rows (panel may be shorter if truncated)
	for len(rightLines) < TH {
		rightLines = append(rightLines, "")
	}
	if len(rightLines) > TH {
		rightLines = rightLines[:TH]
	}

	// ── Combine track list + separator + right panel ──────────────────────
	sep := dimStyle.Render("│")
	var sb strings.Builder
	sb.WriteString(header + "\n")
	sb.WriteString(colHeader + "\n")

	if scrollbar != "" && len(trackLines) > 0 {
		trackLines[0] = trackLines[0] + " " + scrollbar
	}

	for i, tl := range trackLines {
		rl := ""
		if i < len(rightLines) {
			rl = rightLines[i]
		}
		sb.WriteString(tl + sep + rl + "\n")
	}

	sb.WriteString(footerLine + "\n")

	// ── Visualizer strip ──────────────────────────────────────────────────
	if s.visualizer != nil && s.visualizer.IsRunning() {
		sb.WriteString(s.visualizer.RenderBars(w))
	}

	return sb.String()
}
```

Also add the `buildRightPanel` helper (place it just below `View()`):

```go
// buildRightPanel builds the right panel lines, truncating from the bottom
// if availH is less than the full 21 rows.
func (s MusicQueueScreen) buildRightPanel(availH int) []string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle    := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle   := lipgloss.NewStyle().Foreground(theme.T.Text())

	// Find current track
	var curTrack *ipc.MpdTrack
	for i := range s.tracks {
		if s.isCurrentTrack(s.tracks[i]) {
			curTrack = &s.tracks[i]
			break
		}
	}

	valStr := func(v string) string {
		if v == "" {
			return dimStyle.Render("—")
		}
		return textStyle.Render(truncate(v, 20))
	}

	// Build all 21 rows first, then truncate
	var lines []string

	// 1. Art placeholder (9 rows)
	artLines := strings.Split(strings.TrimRight(queueArtPlaceholder(), "\n"), "\n")
	lines = append(lines, artLines...)

	// 2. Metadata (8 rows: 4 × label+value)
	type metaField struct{ label, value string }
	var fields []metaField
	if curTrack != nil {
		fields = []metaField{
			{"TITLE",    curTrack.Title},
			{"ARTIST",   curTrack.Artist},
			{"ALBUM",    curTrack.Album},
			{"DURATION", fmtMusicDuration(curTrack.Duration)},
		}
	} else {
		fields = []metaField{
			{"TITLE", ""}, {"ARTIST", ""}, {"ALBUM", ""}, {"DURATION", ""},
		}
	}
	for _, f := range fields {
		lines = append(lines, dimStyle.Render(f.label))
		lines = append(lines, valStr(f.value))
	}

	// 3. Seek bar (2 rows)
	barRow, timeRow := queueSeekBar(s.nowElapsed, s.nowDuration)
	lines = append(lines, accentStyle.Render(barRow))
	lines = append(lines, dimStyle.Render(timeRow))

	// 4. Volume bar (2 rows)
	volBar, volHint := queueVolumeBar(s.nowVolume, s.nowMuted)
	lines = append(lines, accentStyle.Render(volBar))
	lines = append(lines, dimStyle.Render(volHint))

	// Truncate to availH from the bottom
	if availH < 0 {
		availH = 0
	}
	if len(lines) > availH {
		lines = lines[:availH]
	}
	return lines
}
```

Also extract the old single-column rendering into `viewNarrow`. Add this method to `music_queue.go`:

```go
// viewNarrow renders the original single-column queue layout for width ≤ 80.
func (s MusicQueueScreen) viewNarrow(w, h int,
	accentStyle, dimStyle, textStyle, cursorStyle lipgloss.Style,
	footerLine string,
) string {
	// Reserve 2 rows: 1 header + 1 footer
	listHeight := h - 2
	if listHeight < 1 {
		listHeight = 1
	}

	listW := w

	vl := components.NewVirtualizedList(
		len(s.tracks),
		s.cursor,
		listHeight,
		components.WithScrollMode(components.ScrollModeCenter),
	)

	headerText := fmt.Sprintf("Queue (%d tracks · %s)", len(s.tracks), fmtMusicDuration(s.totalDuration()))
	header := accentStyle.Render(headerText)

	var sb strings.Builder
	sb.WriteString(header + "\n")

	start, _ := vl.VisibleRange()
	scrollbar := vl.VerticalScrollbar(1, dimStyle)
	if start > 0 {
		sb.WriteString(dimStyle.Render("↑ more\n"))
	}

	if s.loading && len(s.tracks) == 0 {
		sb.WriteString("  " + s.spinner.View() + "\n")
		sb.WriteString(footerLine + "\n")
		return sb.String()
	}

	if !s.loading && len(s.tracks) == 0 {
		msg := "Queue is empty"
		pad := (listW - len(msg)) / 2
		if pad < 0 {
			pad = 0
		}
		emptyLine := strings.Repeat(" ", pad) + dimStyle.Render(msg)
		for i := 0; i < listHeight; i++ {
			if i == listHeight/2 {
				sb.WriteString(emptyLine + "\n")
			} else {
				sb.WriteString("\n")
			}
		}
		sb.WriteString(footerLine + "\n")
		return sb.String()
	}

	available := listW - 13
	if available < 10 {
		available = 10
	}
	titleW := available * 40 / 100
	if titleW < 8 {
		titleW = 8
	}
	artistW := available - titleW - 2
	if artistW < 4 {
		artistW = 4
	}

	_, end := vl.VisibleRange()
	var listLines []string
	for i := start; i < end; i++ {
		t := s.tracks[i]
		isCurrent := s.isCurrentTrack(t)
		isCursor  := i == s.cursor

		prefix := "   "
		if isCurrent {
			prefix = "▶  "
		}

		posStr   := fmt.Sprintf("%3d", t.Pos+1)
		titleStr  := fmt.Sprintf("%-*s", titleW, truncate(t.Title, titleW))
		durStr    := fmt.Sprintf("%5s", fmtMusicDuration(t.Duration))
		artistStr := truncate(t.Artist, artistW)
		line := prefix + posStr + " " + titleStr + " " + durStr + "  " + artistStr

		var style lipgloss.Style
		switch {
		case isCurrent:
			style = accentStyle
		case isCursor:
			style = cursorStyle
		default:
			style = textStyle
		}
		listLines = append(listLines, style.Render(line))
	}

	for len(listLines) < listHeight {
		listLines = append(listLines, "")
	}

	if scrollbar != "" {
		for i := range listLines {
			listLines[i] = listLines[i] + " " + scrollbar
		}
	}

	sb.WriteString(strings.Join(listLines, "\n"))
	sb.WriteString("\n")
	sb.WriteString(footerLine + "\n")
	return sb.String()
}
```

- [ ] **Step 4: Build check**

```bash
cd tui && go build ./...
```
Fix any compile errors before running tests.

- [ ] **Step 5: Run tests — expect pass**

```bash
cd tui && go test ./internal/ui/screens/... -run "TestQueue" -v
```
Expected: all TestQueue* PASS.

- [ ] **Step 6: Commit**

```bash
cd tui && git add internal/ui/screens/music_queue.go internal/ui/screens/music_queue_test.go
git commit -m "feat(queue): rewrite View() with two-column layout, column headers, right panel"
```

---

## Chunk 3: Visualizer Wiring

### Task 6: `SetVisualizer` on `MusicScreen` and global suppression in `ui.go`

**Files:**
- Modify: `internal/ui/screens/music_screen.go` — add `SetVisualizer` method
- Modify: `internal/ui/ui.go` — call SetVisualizer; wrap global viz render

There are no meaningful unit tests for these two wiring steps (they involve runtime state). A build check is sufficient.

- [ ] **Step 1: Add `SetVisualizer` to `MusicScreen`**

In `internal/ui/screens/music_screen.go`, add this method after `SetClient`:

```go
// SetVisualizer passes the visualizer reference to the queue sub-tab so it
// can render the visualizer strip inline.
func (s *MusicScreen) SetVisualizer(v *components.Visualizer) {
	s.queue.visualizer = v
}
```

`components` is already imported in `music_screen.go` (check imports; if not, add `"github.com/stui/stui/internal/ui/components"`).

- [ ] **Step 2: Build check**

```bash
cd tui && go build ./internal/ui/screens/...
```
Expected: no output.

- [ ] **Step 3: Call `SetVisualizer` from `ui.go`**

In `internal/ui/ui.go`, find where `m.musicScreen` is first assigned (search for `NewMusicScreen` or `musicScreen =`). After that assignment, add:

```go
m.musicScreen.SetVisualizer(m.visualizer)
```

- [ ] **Step 4: Suppress global visualizer render when queue is active**

In `ui.go`, find the global visualizer render block (around line 2441):
```go
if m.visualizer.IsRunning() {
    if viz := m.visualizer.RenderBars(m.state.Width); viz != "" {
```

Wrap it with:
```go
queueActive := m.state.ActiveTab == state.TabMusic &&
    m.musicScreen.ActiveSubTab() == screens.MusicQueue &&
    m.state.Width > 80
if !queueActive {
    if m.visualizer.IsRunning() {
        if viz := m.visualizer.RenderBars(m.state.Width); viz != "" {
            // existing body unchanged
        }
    }
}
```

`screens` is already imported in `ui.go` — confirm by searching for `screens.` usage in that file.

- [ ] **Step 5: Build check**

```bash
cd tui && go build ./...
```
Expected: no output.

- [ ] **Step 6: Run all tests**

```bash
cd tui && go test ./... 2>&1 | tail -20
```
Expected: all PASS, no new failures.

- [ ] **Step 7: Commit**

```bash
cd tui && git add internal/ui/screens/music_screen.go internal/ui/ui.go
git commit -m "feat(queue): wire visualizer into queue tab, suppress global render when queue active"
```

---

### Task 7: Build and deploy

- [ ] **Step 1: Final build**

```bash
cd tui && go build -o stui ./cmd/stui/
```
Expected: produces `tui/stui` binary.

- [ ] **Step 2: Deploy**

```bash
cp tui/stui /home/ozogorgor/.local/bin/stui.new && mv /home/ozogorgor/.local/bin/stui.new /home/ozogorgor/.local/bin/stui
```

- [ ] **Step 3: Final commit tag**

```bash
cd tui && git add -p  # ensure nothing uncommitted
git commit -m "chore: deploy queue redesign build" --allow-empty
```
