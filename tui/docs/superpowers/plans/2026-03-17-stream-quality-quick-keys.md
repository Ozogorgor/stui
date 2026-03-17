# Stream Quality Quick Keys — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add keys `1`–`4` to instantly pick a quality-tier stream (480p/720p/1080p/4K) from both the detail overlay and the stream picker screen.

**Architecture:** Three changes across two files. A new exported `BestStreamForTier` function in `stream_picker.go` picks the highest-scoring stream at a given quality rank using the existing `qualityScore()` helper. The stream picker's `Update()` intercepts `1`–`4` before action dispatch to use it directly. `ui.go` adds `pendingQuality int` to `Model`, intercepts `1`–`4` in `handleKey` *before* the global action dispatch (so they shadow `ActionTab1`–`4` only when the detail overlay is open), and extends the `StreamsResolvedMsg` handler to auto-pick when `pendingQuality != 0`.

**Tech Stack:** Go 1.22, Bubble Tea, `internal/ui/screens/stream_picker.go`, `internal/ui/ui.go`

**Spec:** `tui/docs/superpowers/specs/2026-03-17-stream-quality-quick-keys-design.md`

---

## Chunk 1: BestStreamForTier

### Task 1: BestStreamForTier helper + tests

**Files:**
- Create: `tui/internal/ui/screens/stream_picker_test.go`
- Modify: `tui/internal/ui/screens/stream_picker.go` (after `qualityScore`, line ~95)

**Background:** `qualityScore(q string) int` is unexported, lives in `stream_picker.go` (package `screens`), uses `strings.HasPrefix` against `qualityRank` map. So `"1080p HDR"` → 5, `"720p 60fps"` → 4, `""` → 0. `BestStreamForTier` must call it for identical semantics. The `qualityRank` table:

```
"4k"/"2160p"/"uhd" → 7    "1440p"/"2k" → 6
"1080p"/"fhd"      → 5    "720p"/"hd"  → 4
"576p"             → 3    "480p"/"sd"  → 2    "360p" → 1
```

Key→rank table used throughout:

| Key | Tier  | rank |
|-----|-------|------|
| `1` | 480p  | 2    |
| `2` | 720p  | 4    |
| `3` | 1080p | 5    |
| `4` | 4K    | 7    |

- [ ] **Step 1: Write the failing tests**

Create `tui/internal/ui/screens/stream_picker_test.go`:

```go
package screens

import (
	"testing"

	"github.com/stui/stui/internal/ipc"
)

func TestBestStreamForTierExactMatch(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "1080p", Score: 80},
		{Quality: "720p", Score: 90},
	}
	got := BestStreamForTier(streams, 5) // rank 5 = 1080p
	if got == nil {
		t.Fatal("expected a match, got nil")
	}
	if got.Quality != "1080p" {
		t.Errorf("expected 1080p, got %s", got.Quality)
	}
}

func TestBestStreamForTierPicksHighestScore(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "1080p", Score: 60},
		{Quality: "1080p", Score: 90},
		{Quality: "1080p", Score: 75},
	}
	got := BestStreamForTier(streams, 5)
	if got == nil {
		t.Fatal("expected a match, got nil")
	}
	if got.Score != 90 {
		t.Errorf("expected score 90, got %d", got.Score)
	}
}

func TestBestStreamForTierNoMatch(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "720p", Score: 90},
		{Quality: "480p", Score: 70},
	}
	got := BestStreamForTier(streams, 5) // rank 5 = 1080p — not present
	if got != nil {
		t.Errorf("expected nil, got %+v", *got)
	}
}

func TestBestStreamForTierEmptyList(t *testing.T) {
	got := BestStreamForTier(nil, 5)
	if got != nil {
		t.Errorf("expected nil for empty list, got %+v", *got)
	}
}

func TestBestStreamForTierEmptyQualityNotMatched(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "", Score: 999},
		{Quality: "1080p", Score: 50},
	}
	got := BestStreamForTier(streams, 5)
	if got == nil {
		t.Fatal("expected a match, got nil")
	}
	if got.Quality != "1080p" {
		t.Errorf("expected 1080p, got %q", got.Quality)
	}
}

func TestBestStreamForTierHasPrefixSemantics(t *testing.T) {
	// "1080p HDR" has prefix "1080p" → qualityScore returns 5.
	streams := []ipc.StreamInfo{
		{Quality: "1080p HDR", Score: 85},
	}
	got := BestStreamForTier(streams, 5)
	if got == nil {
		t.Fatal("expected a match for '1080p HDR' at rank 5, got nil")
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestBestStreamForTier" -v
```

Expected: FAIL — `BestStreamForTier` undefined.

- [ ] **Step 3: Implement BestStreamForTier**

In `tui/internal/ui/screens/stream_picker.go`, add immediately after the closing `}` of `qualityScore` (after line 95):

```go
// BestStreamForTier returns the stream with the highest Score
// (ipc.StreamInfo.Score, the provider-reported integer) whose quality label
// resolves to the given qualityRank value, or nil if none match.
//
// Uses qualityScore() for label→rank lookup so "1080p HDR" matches rank 5
// just like "1080p".
func BestStreamForTier(streams []ipc.StreamInfo, rank int) *ipc.StreamInfo {
	var best *ipc.StreamInfo
	for i := range streams {
		s := &streams[i]
		if qualityScore(s.Quality) != rank {
			continue
		}
		if best == nil || s.Score > best.Score {
			best = s
		}
	}
	return best
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestBestStreamForTier" -v
```

Expected: all 6 tests PASS.

- [ ] **Step 5: Verify full build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

Expected: no errors.

---

## Chunk 2: Stream picker key handling

### Task 2: Quality keys inside StreamPickerScreen

**Files:**
- Modify: `tui/internal/ui/screens/stream_picker.go:432–440` (before `actions.FromKey` call)

**Background:** The picker's `Update()` handles `tea.KeyMsg` starting at line 373. There is an `actions.FromKey(key)` call at line 441. The `1`–`4` keys must be intercepted **before** that call. When no match is found, return an `ipc.StatusMsg` cmd so the root model's status bar shows the error (same mechanism as `case ipc.StatusMsg: m.state.StatusMsg = msg.Text` at root ui.go:297). The `SwitchStream` + `screen.PopMsg{}` pattern (lines 381, 453) plays the stream and dismisses the picker.

- [ ] **Step 1: Add quality key block to stream picker Update()**

In `tui/internal/ui/screens/stream_picker.go`, find the `// 'd' — pre-download` block that ends around line 439 with `}`. Add the quality key block immediately after it and before the `if action, ok := actions.FromKey(key); ok {` line:

```go
		// Quality quick keys: 1=480p, 2=720p, 3=1080p, 4=4K
		// Checked before actions.FromKey to prevent confusion with any global bindings.
		if !s.loading && len(s.streams) > 0 {
			qualKeyRank := map[string]int{"1": 2, "2": 4, "3": 5, "4": 7}
			qualLabel := map[string]string{"1": "480p", "2": "720p", "3": "1080p", "4": "4K"}
			if rank, ok := qualKeyRank[key]; ok {
				if best := BestStreamForTier(s.streams, rank); best != nil && s.client != nil {
					s.client.SwitchStream(best.URL)
					return s, func() tea.Msg { return screen.PopMsg{} }
				}
				label := qualLabel[key]
				return s, func() tea.Msg {
					return ipc.StatusMsg{Text: "No " + label + " streams available"}
				}
			}
		}
```

- [ ] **Step 2: Verify build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

Expected: no errors.

- [ ] **Step 3: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```

Expected: all pass.

---

## Chunk 3: Model state + detail overlay + StreamsResolvedMsg

### Task 3: pendingQuality on Model + detail overlay intercept + auto-pick handler

**Files:**
- Modify: `tui/internal/ui/ui.go` — Model struct (~line 169), `handleKey` (~line 1082), `StreamsResolvedMsg` handler (~line 399)

**Background:**

*Model struct:* Add `pendingQuality int` after `streamStats` (line ~169). Zero means no pending quality.

*handleKey intercept:* `handleKey` (line 1084) processes keys in this order:
1. Binge countdown intercept (lines 1087–1099)
2. Global action dispatch — handles `ActionTab1`–`4` at lines 1123–1137
3. Player controls, MPD controls, screen delegation
4. Detail overlay: `if m.screen == screenDetail && m.detail != nil { return m.handleDetailKey(key) }` (line 1321)

`ActionTab1`–`4` are triggered at step 2, **before** the detail overlay handler at step 4. So quality keys must be intercepted between steps 1 and 2 (after binge countdown, before global action dispatch).

*StreamsResolvedMsg:* Currently at line 399, just accumulates stats and sends a notification. Extend it to auto-pick when `m.detail != nil && msg.EntryID == m.detail.Entry.ID && m.pendingQuality != 0`.

*Resolve call:* Use `m.client.Resolve(m.detail.Entry.ID, "")` — same empty-provider pattern the stream picker itself uses (stream_picker.go:323).

*Feedback:* Use `components.ShowToast` + `m.activeToast` (same pattern as line 1217–1218) for "Resolving streams…" and "No Xp streams available" toasts. `components.ShowToast` is available in package `ui`.

- [ ] **Step 1: Add pendingQuality to Model struct**

In `tui/internal/ui/ui.go`, find the `// Stream Radar` comment (~line 168). Add `pendingQuality` immediately before `streamStats`:

```go
	// Stream quality quick keys — rank of tier user pressed (0 = none pending).
	// 2=480p  4=720p  5=1080p  7=4K  (qualityRank values from stream_picker.go)
	pendingQuality int

	// Stream Radar — accumulated stream stats for the current session.
	streamStats screens.StreamRadarStats
```

- [ ] **Step 2: Add quality key intercept to handleKey**

In `tui/internal/ui/ui.go`, find `handleKey` at line 1084. After the binge countdown block (which ends around line 1099 with `}`), add the quality key intercept block **before** the `// ── Action-based dispatch` comment:

```go
	// ── Quality quick keys — detail overlay only ──────────────────────────
	// Must intercept here, before ActionTab1–4 in the global action dispatch.
	if m.screen == screenDetail && m.detail != nil && !m.detail.CollectionPickerOpen {
		qualKeyRank := map[string]int{"1": 2, "2": 4, "3": 5, "4": 7}
		if rank, ok := qualKeyRank[key]; ok {
			m.pendingQuality = rank
			if m.client != nil {
				m.client.Resolve(m.detail.Entry.ID, "")
			}
			t, cmd := components.ShowToast("Resolving streams\u2026", false)
			m.activeToast = &t
			return m, cmd
		}
	}
```

- [ ] **Step 3: Extend StreamsResolvedMsg handler**

In `tui/internal/ui/ui.go`, find the `case ipc.StreamsResolvedMsg:` block (line ~399). Replace it with:

```go
	case ipc.StreamsResolvedMsg:
		// Accumulate into session-wide radar stats.
		m.streamStats.AddBatch(msg.Streams)
		if m.notifyCfg.OnStreams && len(msg.Streams) > 0 {
			body := fmt.Sprintf("%d stream(s) found", len(msg.Streams))
			notify.Send(m.notifyCfg, "✓ Streams Resolved", body, notify.UrgencyLow)
		}
		// Quality quick key auto-pick: fire when a pending tier matches this entry.
		if m.detail != nil && msg.EntryID == m.detail.Entry.ID && m.pendingQuality != 0 {
			rank := m.pendingQuality
			m.pendingQuality = 0
			qualLabel := map[int]string{2: "480p", 4: "720p", 5: "1080p", 7: "4K"}
			if best := screens.BestStreamForTier(msg.Streams, rank); best != nil && m.client != nil {
				m.client.SwitchStream(best.URL)
			} else {
				t, cmd := components.ShowToast("No "+qualLabel[rank]+" streams available", false)
				m.activeToast = &t
				return m, cmd
			}
		}
		return m, nil
```

- [ ] **Step 4: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

Expected: no errors.

- [ ] **Step 5: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```

Expected: all pass.
