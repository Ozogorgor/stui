# Autoplay Next Episode Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing binge/autoplay behaviour persistent by adding two settings items ("Auto-play next episode" toggle and "Auto-play countdown" duration) that initialize the episode screen and control the countdown timer.

**Architecture:** Four small edits across four files — add `minVal`/`maxVal` clamping to `settingItem`, extend `state.Settings`, add two items to the Playback settings category plus their handler cases in `ui.go`, and add an `autoplayDefault` parameter to `NewEpisodeScreen`. No new files except a test file for the clamping logic.

**Tech Stack:** Go 1.22, Bubble Tea, `internal/ui/screens/settings.go`, `internal/state/app_state.go`, `internal/ui/ui.go`, `internal/ui/screens/episode.go`

**Spec:** `tui/docs/superpowers/specs/2026-03-17-autoplay-next-episode-design.md`

---

## Chunk 1: settingItem clamping + Settings state

### Task 1: Add minVal/maxVal to settingItem and clamp in adjust()

The `settingItem.adjust()` method currently does unclamped `intVal += delta`. We need optional bounds so the countdown setting can be restricted to 3–30 seconds.

**Files:**
- Modify: `tui/internal/ui/screens/settings.go:60-115`
- Create: `tui/internal/ui/screens/settings_test.go`

- [ ] **Step 1: Write the failing tests**

Create `tui/internal/ui/screens/settings_test.go`:

```go
package screens

import "testing"

func TestAdjustClampsAtMax(t *testing.T) {
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 3, maxVal: 10}
	item.adjust(100)
	if item.intVal != 10 {
		t.Errorf("expected max 10, got %d", item.intVal)
	}
}

func TestAdjustClampsAtMin(t *testing.T) {
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 3, maxVal: 10}
	item.adjust(-100)
	if item.intVal != 3 {
		t.Errorf("expected min 3, got %d", item.intVal)
	}
}

func TestAdjustNormalWithinBounds(t *testing.T) {
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 3, maxVal: 10}
	item.adjust(2)
	if item.intVal != 7 {
		t.Errorf("expected 7, got %d", item.intVal)
	}
}

func TestAdjustNoBoundsWhenZero(t *testing.T) {
	// Existing items have no minVal/maxVal (zero values) — must behave unchanged.
	item := &settingItem{kind: settingInt, intVal: 100}
	item.adjust(50)
	if item.intVal != 150 {
		t.Errorf("expected 150, got %d", item.intVal)
	}
	item.adjust(-200)
	if item.intVal != -50 {
		t.Errorf("expected -50, got %d", item.intVal)
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestAdjust" -v
```
Expected: FAIL — `settingItem` has no `minVal`/`maxVal` fields yet.

- [ ] **Step 3: Add minVal/maxVal fields to settingItem**

In `tui/internal/ui/screens/settings.go`, find the `settingItem` struct (line 60). Add two fields at the end, before the closing brace:

```go
type settingItem struct {
	label       string
	key         string      // dot-separated config key e.g. "player.default_volume"
	kind        settingKind
	boolVal     bool
	intVal      int
	floatVal    float64
	choiceVals  []string
	choiceIdx   int
	description string // shown in the footer when focused
	minVal      int    // lower bound for settingInt; 0 = no lower bound
	maxVal      int    // upper bound for settingInt; 0 = no upper bound
}
```

- [ ] **Step 4: Update adjust() to clamp**

In `tui/internal/ui/screens/settings.go`, find `func (s *settingItem) adjust(delta int)` (line 105). Replace the `case settingInt:` block:

```go
func (s *settingItem) adjust(delta int) {
	switch s.kind {
	case settingInt:
		s.intVal += delta
		if s.maxVal > 0 && s.intVal > s.maxVal {
			s.intVal = s.maxVal
		}
		if s.minVal > 0 && s.intVal < s.minVal {
			s.intVal = s.minVal
		}
	case settingFloat:
		s.floatVal += float64(delta) * 0.5
	case settingChoice:
		n := len(s.choiceVals)
		s.choiceIdx = (s.choiceIdx + delta + n) % n
	}
}
```

- [ ] **Step 5: Run tests — verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestAdjust" -v
```
Expected: all 4 tests PASS.

- [ ] **Step 6: Confirm full build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```
Expected: no errors.

---

### Task 2: Add AutoplayNext and AutoplayCountdown to state.Settings

**Files:**
- Modify: `tui/internal/state/app_state.go:67-81`

No test needed — this is a struct field addition; the existing `DefaultSettings()` function does not need updating because `AutoplayNext` defaults to `false` (Go zero = correct) and `AutoplayCountdown = 0` is handled by the fallback logic in Task 3.

- [ ] **Step 1: Add fields to Settings struct**

In `tui/internal/state/app_state.go`, find the `Settings` struct (line 67). Add two fields at the end of the struct, under a new comment:

```go
type Settings struct {
	// Playback
	AutoSkipIntro   bool
	AutoSkipCredits bool

	// Post-playback cleanup
	AutoDeleteVideo bool // default true
	AutoDeleteAudio bool // default false

	// Stream selection
	BenchmarkStreams bool // default false

	// Display
	ViewMode ViewMode

	// Autoplay
	AutoplayNext      bool // default false — initialises bingeEnabled on EpisodeScreen
	AutoplayCountdown int  // seconds; 0 treated as 5 in countdown logic
}
```

- [ ] **Step 2: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

---

## Chunk 2: Settings items + handler + countdown fix

### Task 3: Add settings items, wire handler, fix hardcoded countdown

**Files:**
- Modify: `tui/internal/ui/screens/settings.go:481-488` (Playback category)
- Modify: `tui/internal/ui/ui.go` (SettingsChangedMsg handler ~line 729, countdown ~line 585)

- [ ] **Step 1: Add two items to the Playback category**

In `tui/internal/ui/screens/settings.go`, find the Playback category's `items` slice. The last item is "Keep open" ending at approximately line 487 with `},`. Add two new items after it, before the closing `},` of the `items` slice:

```go
			{
				label:       "Auto-play next episode",
				key:         "playback.autoplay_next",
				kind:        settingBool,
				boolVal:     false,
				description: "Automatically play the next episode when one finishes",
			},
			{
				label:       "Auto-play countdown",
				key:         "playback.autoplay_countdown",
				kind:        settingInt,
				intVal:      5,
				minVal:      3,
				maxVal:      30,
				description: "Seconds to wait before auto-playing the next episode (3–30)",
			},
```

The Playback items slice should now look like:
```go
items: []*settingItem{
    { /* Volume */ },
    { /* Hardware decode */ },
    { /* Cache (secs) */ },
    { /* Keep open */ },
    { /* Auto-play next episode */ },  // new
    { /* Auto-play countdown */ },     // new
},
```

- [ ] **Step 2: Add SettingsChangedMsg handler cases**

In `tui/internal/ui/ui.go`, find the `switch msg.Key` block inside the `SettingsChangedMsg` handler (around line 729). Add two new cases after the existing `skipper.*` and `streaming.*` cases:

```go
	case "playback.autoplay_next":
		if v, ok := msg.Value.(bool); ok {
			m.state.Settings.AutoplayNext = v
		}
	case "playback.autoplay_countdown":
		if v, ok := msg.Value.(int); ok {
			m.state.Settings.AutoplayCountdown = v
		}
```

- [ ] **Step 3: Replace hardcoded countdown with settings value**

In `tui/internal/ui/ui.go`, find the binge EOF handler (around line 583–586). The current code is:

```go
if msg.Reason == "eof" && m.bingeCtx != nil && m.bingeCtx.BingeEnabled {
    if m.bingeCtx.CurrentIdx+1 < len(m.bingeCtx.Episodes) {
        m.bingeCountdown = 5
        return m, bingeTickCmd()
    }
```

Replace `m.bingeCountdown = 5` with:

```go
        countdown := m.state.Settings.AutoplayCountdown
        if countdown <= 0 {
            countdown = 5
        }
        m.bingeCountdown = countdown
```

The full block after the change:

```go
if msg.Reason == "eof" && m.bingeCtx != nil && m.bingeCtx.BingeEnabled {
    if m.bingeCtx.CurrentIdx+1 < len(m.bingeCtx.Episodes) {
        countdown := m.state.Settings.AutoplayCountdown
        if countdown <= 0 {
            countdown = 5
        }
        m.bingeCountdown = countdown
        return m, bingeTickCmd()
    }
    // Last episode of the season — clear context.
    m.bingeCtx = nil
}
```

- [ ] **Step 4: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

- [ ] **Step 5: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```
Expected: all pass.

---

## Chunk 3: Episode screen constructor

### Task 4: Add autoplayDefault to NewEpisodeScreen

The episode screen's `bingeEnabled` currently starts as `false` always. We need it to reflect the setting so users with autoplay enabled don't have to press `b` each time.

**Files:**
- Modify: `tui/internal/ui/screens/episode.go:48-56`
- Modify: `tui/internal/ui/ui.go` (all `NewEpisodeScreen(...)` call sites)

- [ ] **Step 1: Find all call sites of NewEpisodeScreen in ui.go**

```bash
grep -n "NewEpisodeScreen" "/home/ozogorgor/Projects/Stui Project/stui/tui/internal/ui/ui.go"
```

Note the line numbers — you will update each call site in Step 4.

- [ ] **Step 2: Update NewEpisodeScreen signature**

In `tui/internal/ui/screens/episode.go`, change `NewEpisodeScreen` to accept `autoplayDefault bool` and initialise `bingeEnabled` from it:

```go
func NewEpisodeScreen(client *ipc.Client, title, seriesID string, autoplayDefault bool) EpisodeScreen {
	return EpisodeScreen{
		client:       client,
		title:        title,
		seriesID:     seriesID,
		loading:      true,
		seasons:      []int{1, 2, 3, 4, 5}, // populated from metadata
		bingeEnabled: autoplayDefault,
	}
}
```

- [ ] **Step 3: Verify build fails at call sites**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./... 2>&1 | grep "NewEpisodeScreen"
```
Expected: compile errors listing each call site that needs updating.

- [ ] **Step 4: Update all call sites in ui.go**

For each call site found in Step 1, add `m.state.Settings.AutoplayNext` as the fourth argument. For example:

```go
// Before:
NewEpisodeScreen(m.client, title, seriesID)

// After:
NewEpisodeScreen(m.client, title, seriesID, m.state.Settings.AutoplayNext)
```

- [ ] **Step 5: Build — verify no errors**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```
Expected: no errors.

- [ ] **Step 6: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```
Expected: all pass.

- [ ] **Step 7: Smoke test**

```bash
go run ./cmd/stui
```

Verify:
1. Settings screen → Playback category shows "Auto-play next episode  off" and "Auto-play countdown  5"
2. `+`/`-` on countdown clamps at 3 and 30
3. Toggle "Auto-play next episode" on → navigate to a series → episode screen opens with "b  binge ON" already active
4. Toggle off → episode screen opens with "b  binge off" (default unchanged)
5. With autoplay on, play an episode to completion → countdown banner uses the configured seconds
