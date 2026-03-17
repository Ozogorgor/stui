# Download Directory Settings — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add user-configurable `~/Videos` and `~/Music` download directory settings, editable inline from the Settings screen via a new `settingPath` kind with an embedded text input.

**Architecture:** Three files changed. `app_state.go` gains two new `Settings` fields populated from `os.UserHomeDir()`. `settings.go` gains a new `settingPath` kind with a package-level home-dir variable, inline text editing state on `SettingsModel`, and a new Downloads category. `ui.go` adds two mirror cases for the new config keys.

**Tech Stack:** Go 1.22, Bubble Tea, `charmbracelet/bubbles/textinput`, `internal/state/app_state.go`, `internal/ui/screens/settings.go`, `internal/ui/ui.go`

**Spec:** `tui/docs/superpowers/specs/2026-03-17-download-directory-settings-design.md`

---

## Chunk 1: Data model + settingPath kind

### Task 1: state.Settings data model

**Files:**
- Modify: `tui/internal/state/app_state.go`
- Create: `tui/internal/state/app_state_test.go`

**Background:** `Settings` in `app_state.go` currently has fields for Playback, Post-playback, Stream selection, Display, and Autoplay. `DefaultSettings()` only sets `AutoDeleteVideo: true`. Both `"os"` and `"path/filepath"` are NOT currently imported in this file.

- [ ] **Step 1: Write the failing test**

Create `tui/internal/state/app_state_test.go`:

```go
package state

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDefaultSettingsVideoDir(t *testing.T) {
	s := DefaultSettings()
	home, _ := os.UserHomeDir()
	want := filepath.Join(home, "Videos")
	if s.VideoDownloadDir != want {
		t.Errorf("VideoDownloadDir = %q, want %q", s.VideoDownloadDir, want)
	}
}

func TestDefaultSettingsMusicDir(t *testing.T) {
	s := DefaultSettings()
	home, _ := os.UserHomeDir()
	want := filepath.Join(home, "Music")
	if s.MusicDownloadDir != want {
		t.Errorf("MusicDownloadDir = %q, want %q", s.MusicDownloadDir, want)
	}
}

func TestDefaultSettingsAutoDeleteVideoStillTrue(t *testing.T) {
	// Regression: existing default must not be broken.
	s := DefaultSettings()
	if !s.AutoDeleteVideo {
		t.Error("AutoDeleteVideo should still default to true")
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/state/... -run "TestDefaultSettings" -v
```

Expected: FAIL — `VideoDownloadDir` and `MusicDownloadDir` undefined (compile error).

- [ ] **Step 3: Add fields to Settings and update DefaultSettings()**

In `tui/internal/state/app_state.go`, add `"os"` and `"path/filepath"` to the import block (file currently has no imports — add a new import block at the top after the `package state` line):

```go
import (
	"os"
	"path/filepath"
)
```

In the `Settings` struct, add after the `// Autoplay` block:

```go
	// Downloads — directory paths for aria2 downloads.
	VideoDownloadDir string // default ~/Videos
	MusicDownloadDir string // default ~/Music
```

Replace `DefaultSettings()` entirely:

```go
func DefaultSettings() Settings {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		home = "."
	}
	return Settings{
		AutoDeleteVideo:  true,
		VideoDownloadDir: filepath.Join(home, "Videos"),
		MusicDownloadDir: filepath.Join(home, "Music"),
	}
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/state/... -run "TestDefaultSettings" -v
```

Expected: all 3 tests PASS.

- [ ] **Step 5: Verify full build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

Expected: no errors.

---

### Task 2: settingPath kind — core infrastructure

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`
- Modify: `tui/internal/ui/screens/settings_test.go`

**Background:** `settingKind` is an `iota` enum at line 51. `settingItem` struct is at line 60. `displayValue()` is at line 74 — a switch on `s.kind`. `settingChangedCmd()` is at line 316 — a switch that currently has no `settingPath` case, causing unknown kinds to emit `Value: nil`. Current imports in `settings.go`: `"fmt"`, `"strings"`, bubbletea, lipgloss, screen, theme. The file needs `"os"`, `"path/filepath"`, and `textinput` added (textinput will be used in Task 3; add the import here so the file compiles cleanly for this task).

- [ ] **Step 1: Write failing tests**

Add to `tui/internal/ui/screens/settings_test.go`:

```go
func TestSettingPathDisplayValueTildePrefix(t *testing.T) {
	// Save and restore the package-level var so this test is hermetic.
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "/home/testuser"
	item := &settingItem{kind: settingPath, strVal: "/home/testuser/Videos"}
	got := item.displayValue()
	if got != "~/Videos" {
		t.Errorf("displayValue() = %q, want %q", got, "~/Videos")
	}
}

func TestSettingPathDisplayValueNonHomePath(t *testing.T) {
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "/home/testuser"
	item := &settingItem{kind: settingPath, strVal: "/mnt/data/videos"}
	got := item.displayValue()
	if got != "/mnt/data/videos" {
		t.Errorf("displayValue() = %q, want non-home path unchanged", got)
	}
}

func TestSettingPathDisplayValueExactHomeDir(t *testing.T) {
	// strVal == settingsHomeDir exactly (no subdir) — must not produce "~/"
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "/home/testuser"
	item := &settingItem{kind: settingPath, strVal: "/home/testuser"}
	got := item.displayValue()
	// filepath.Rel("/home/testuser", "/home/testuser") == "." — rendered as "~/."
	// This is acceptable edge-case behaviour; the important thing is no panic.
	if got == "" {
		t.Errorf("displayValue() returned empty string for exact home dir")
	}
}

func TestSettingPathDisplayValueFallbackDot(t *testing.T) {
	// When settingsHomeDir is ".", no ~/prefix should be added.
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "."
	item := &settingItem{kind: settingPath, strVal: "/some/path"}
	got := item.displayValue()
	if got != "/some/path" {
		t.Errorf("displayValue() with homeDir='.': got %q, want raw path", got)
	}
}

func TestSettingChangedCmdPathEmitsString(t *testing.T) {
	item := &settingItem{kind: settingPath, key: "downloads.video_dir", strVal: "/home/user/Videos"}
	cmd := settingChangedCmd(item)
	if cmd == nil {
		t.Fatal("settingChangedCmd returned nil for settingPath item")
	}
	msg := cmd()
	scm, ok := msg.(SettingsChangedMsg)
	if !ok {
		t.Fatalf("expected SettingsChangedMsg, got %T", msg)
	}
	v, ok := scm.Value.(string)
	if !ok {
		t.Fatalf("expected Value to be string, got %T (nil means missing case)", scm.Value)
	}
	if v != "/home/user/Videos" {
		t.Errorf("Value = %q, want %q", v, "/home/user/Videos")
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestSettingPath|TestSettingChangedCmdPath" -v
```

Expected: FAIL — `settingPath` undefined and `settingsHomeDir` undefined.

- [ ] **Step 3: Add imports to settings.go**

In `tui/internal/ui/screens/settings.go`, replace the existing import block. Do NOT add `textinput` yet — it will be added in Task 3 when it is actually referenced (Go rejects unused imports):

```go
import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)
```

- [ ] **Step 4: Add settingsHomeDir package var + init()**

In `tui/internal/ui/screens/settings.go`, immediately after the import block, before the `type settingKind int` line, add:

```go
// settingsHomeDir is resolved once at program start and used by displayValue()
// and defaultCategories() to avoid calling os.UserHomeDir() on every render.
// Tests can set this variable directly to control the home path.
var settingsHomeDir string

func init() {
	h, err := os.UserHomeDir()
	if err != nil || h == "" {
		settingsHomeDir = "."
	} else {
		settingsHomeDir = h
	}
}
```

- [ ] **Step 5: Add settingPath to the settingKind enum**

In `tui/internal/ui/screens/settings.go`, find the `const (` block starting at line 50. Append `settingPath` after `settingAction`:

```go
const (
	settingBool   settingKind = iota // on/off toggle
	settingInt                       // integer with +/- adjustment
	settingFloat                     // float with +/- adjustment
	settingChoice                    // cycle through a fixed list
	settingInfo                      // read-only informational row
	settingAction                    // press Enter → emits a message (no value change)
	settingPath                      // editable filesystem path; Enter opens inline textinput
)
```

- [ ] **Step 6: Add strVal field to settingItem**

In the `settingItem` struct (line ~60), add `strVal` after `choiceIdx`:

```go
	choiceIdx   int
	strVal      string      // current path value for settingPath items
	description string // shown in the footer when focused
```

- [ ] **Step 7: Add settingPath case to displayValue()**

In `displayValue()`, add after the `case settingAction:` case and before the closing `}`:

```go
	case settingPath:
		if settingsHomeDir == "." {
			return s.strVal
		}
		rel, err := filepath.Rel(settingsHomeDir, s.strVal)
		if err == nil && !strings.HasPrefix(rel, "..") {
			return "~/" + rel
		}
		return s.strVal
```

- [ ] **Step 8: Add settingPath case to settingChangedCmd()**

In `settingChangedCmd()`, add after the `case settingChoice:` case:

```go
	case settingPath:
		v = item.strVal
```

- [ ] **Step 9: Run tests — verify they pass**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestSettingPath|TestSettingChangedCmdPath" -v
```

Expected: all 5 new tests PASS.

- [ ] **Step 10: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```

Expected: all pass (including pre-existing settingInt and BestStreamForTier tests).

- [ ] **Step 11: Verify build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

Expected: no errors.

---

## Chunk 2: Editing interaction + Downloads category + wiring

> **Prerequisites:** Chunk 1 must be fully complete before starting this chunk. `settingPath`, `strVal`, `settingsHomeDir`, and their associated helpers are all defined in Tasks 1–2.

### Task 3: Editing state + interaction + View changes

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`
- Modify: `tui/internal/ui/screens/settings_test.go`

**Background:** `SettingsModel` struct is at line 145. The `Update()` `case "enter", "right", "l":` is a single combined case at line 254. The `View()` renders items in a loop at line 411; the value column uses `valStyle.Render(item.displayValue())` at line 430. The footer hint is a hardcoded string at line 453. `settingCategory.items` is `[]*settingItem` — pointer slice, so `items[i].field = x` mutates the stored item directly.

- [ ] **Step 1: Add textinput import and editing fields to SettingsModel**

In `tui/internal/ui/screens/settings.go`, add `textinput` to the import block:

```go
import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/bubbles/textinput"
	"github.com/charmbracelet/lipgloss"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)
```

In the `SettingsModel` struct, add after `height int`:

```go
	// Path editing state — active when the user is editing a settingPath item.
	editing   bool
	editInput textinput.Model
```

- [ ] **Step 2: Write tests for editing interaction**

Add to `tui/internal/ui/screens/settings_test.go`:

```go
func TestSettingPathToggleIsNoOp(t *testing.T) {
	// toggle() must not panic and must not change strVal for settingPath items.
	item := &settingItem{kind: settingPath, strVal: "/home/user/Videos"}
	item.toggle()
	if item.strVal != "/home/user/Videos" {
		t.Errorf("toggle() changed strVal: got %q", item.strVal)
	}
}

func TestSettingPathAdjustIsNoOp(t *testing.T) {
	// adjust() must not panic and must not change strVal for settingPath items.
	item := &settingItem{kind: settingPath, strVal: "/home/user/Videos"}
	item.adjust(1)
	item.adjust(-1)
	if item.strVal != "/home/user/Videos" {
		t.Errorf("adjust() changed strVal: got %q", item.strVal)
	}
}
```

- [ ] **Step 3: Run tests — verify they pass immediately (both are regression guards)**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestSettingPathToggle|TestSettingPathAdjust" -v
```

Expected: both PASS (toggle/adjust already no-op for unknown kinds via switch fall-through).

- [ ] **Step 4: Split "enter" from "right"/"l" case and add path editing branch**

In `tui/internal/ui/screens/settings.go`, find:

```go
	case "enter", "right", "l":
		if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
			m.inCategory = true
			m.itemCursor = 0
		} else if m.inCategory {
			cat := m.categories[m.catCursor]
			if m.itemCursor < len(cat.items) {
				item := cat.items[m.itemCursor]
				// Action items navigate to a sub-screen
				if item.kind == settingAction {
```

Replace the entire `case "enter", "right", "l":` block with two separate cases. The `"right", "l"` case keeps only the category-entry branch. The `"enter"` case adds the path-editing branch before the existing toggle logic:

```go
		case "right", "l":
			if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
				m.inCategory = true
				m.itemCursor = 0
			}

		case "enter":
			if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
				m.inCategory = true
				m.itemCursor = 0
			} else if m.inCategory {
				cat := m.categories[m.catCursor]
				if m.itemCursor < len(cat.items) {
					item := cat.items[m.itemCursor]
					// settingPath — start inline editing instead of toggle
					if item.kind == settingPath {
						ti := textinput.New()
						ti.SetValue(item.strVal)
						ti.CursorEnd()
						ti.Width = 48
						ti.CharLimit = 512
						cmd := ti.Focus() // returns blink cmd — must be returned
						m.editInput = ti
						m.editing = true
						return m, cmd
					}
					// Action items navigate to a sub-screen
					if item.kind == settingAction {
						switch item.key {
						case "plugins.manager":
							return m, func() tea.Msg { return OpenPluginManagerMsg{} }
						case "plugins.manage_repos":
							return m, func() tea.Msg { return OpenPluginReposMsg{} }
						case "keybinds.edit":
							return m, func() tea.Msg { return OpenKeybindsEditorMsg{} }
						case "stats.stream_radar":
							return m, func() tea.Msg { return OpenStreamRadarMsg{} }
						case "stats.rating_weights":
							return m, func() tea.Msg { return OpenRatingWeightsMsg{} }
						case "stats.offline_library":
							return m, func() tea.Msg { return OpenOfflineLibraryMsg{} }
						case "stats.clear_cache":
							return m, func() tea.Msg { return ClearMediaCacheMsg{} }
						default:
							return m, func() tea.Msg { return OpenPluginSettingsMsg{} }
						}
					}
					item.toggle()
					return m, settingChangedCmd(item)
				}
			}
```

- [ ] **Step 5: Add editing intercept block at the top of Update()**

In `tui/internal/ui/screens/settings.go`, `Update()` starts with `switch msg := msg.(type) {`. Add the editing intercept at the very top of `Update()`, before the `switch`:

```go
func (m SettingsModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	// ── Editing intercept — settingPath inline text input ─────────────────
	// While editing, all input is consumed here. Navigation is suppressed.
	if m.editing {
		switch msg := msg.(type) {
		case tea.KeyMsg:
			switch msg.String() {
			case "enter":
				// Confirm: write the typed value back to the item.
				cat := m.categories[m.catCursor]
				item := cat.items[m.itemCursor]
				item.strVal = m.editInput.Value()
				m.editing = false
				return m, settingChangedCmd(item)
			case "esc":
				// Cancel: discard typed text, no change.
				m.editing = false
				return m, nil
			default:
				// Forward to textinput; capture both return values.
				newInput, cmd := m.editInput.Update(msg)
				m.editInput = newInput
				return m, cmd
			}
		case tea.MouseMsg:
			// Suppress mouse events during editing to prevent cursor drift.
			return m, nil
		}
		return m, nil
	}

	switch msg := msg.(type) {
```

- [ ] **Step 6: Update View() to render textinput when editing**

In `tui/internal/ui/screens/settings.go`, in `View()`, find the item rendering loop. Replace:

```go
		val := valStyle.Render(item.displayValue())
		line := style.Render(prefix+labelPad) + val
```

with:

```go
		var val string
		if m.editing && m.inCategory && i == m.itemCursor && item.kind == settingPath {
			// Render the live textinput instead of the plain value.
			val = m.editInput.View()
		} else {
			val = valStyle.Render(item.displayValue())
		}
		line := style.Render(prefix+labelPad) + val
```

- [ ] **Step 7: Update footer hint during editing**

In `tui/internal/ui/screens/settings.go`, in `View()`, replace:

```go
	hint := "↑↓ navigate   enter select/toggle   +/- adjust   ← back   esc exit"
	footer := footerStyle.Render(hint)
```

with:

```go
	var hint string
	if m.editing {
		hint = "enter confirm   esc cancel"
	} else {
		hint = "↑↓ navigate   enter select/toggle   +/- adjust   ← back   esc exit"
	}
	footer := footerStyle.Render(hint)
```

- [ ] **Step 8: Verify build is clean**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go build ./...
```

Expected: no errors.

- [ ] **Step 9: Run all tests**

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./...
```

Expected: all pass.

---

### Task 4: Downloads category + ui.go wiring

**Files:**
- Modify: `tui/internal/ui/screens/settings.go` (defaultCategories)
- Modify: `tui/internal/ui/ui.go`

**Background:** `defaultCategories()` is at line 461. The Streaming category ends at line 561, followed immediately by the Subtitles category at line 562. The new Downloads category is inserted between them. In `ui.go`, the `SettingsChangedMsg` handler switch block containing mirrors for local state lives at around line 750; `playback.autoplay_countdown` case ends around line 781.

- [ ] **Step 1: Add Downloads category to defaultCategories()**

In `tui/internal/ui/screens/settings.go`, find the closing `},` of the Streaming category (the `},` that closes the `items: []*settingItem{...}` for Streaming — right before `{name: "Subtitles"`). Insert the new Downloads category immediately after it:

```go
		},
		{
			name: "Downloads",
			icon: "⬇",
			items: []*settingItem{
				{
					label:       "Video download dir",
					key:         "downloads.video_dir",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos"),
					description: "Directory for movie and series downloads (enter to edit)",
				},
				{
					label:       "Music download dir",
					key:         "downloads.music_dir",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Music"),
					description: "Directory for music and audio downloads (enter to edit)",
				},
			},
		},
		{
			name: "Subtitles",
```

- [ ] **Step 2: Write and run test verifying the Downloads category exists**

Add to `tui/internal/ui/screens/settings_test.go`:

```go
func TestDefaultCategoriesHasDownloads(t *testing.T) {
	cats := defaultCategories()
	for _, cat := range cats {
		if cat.name == "Downloads" {
			keys := make([]string, len(cat.items))
			for i, item := range cat.items {
				keys[i] = item.key
			}
			wantKeys := []string{"downloads.video_dir", "downloads.music_dir"}
			for _, wk := range wantKeys {
				found := false
				for _, k := range keys {
					if k == wk {
						found = true
						break
					}
				}
				if !found {
					t.Errorf("Downloads category missing item with key %q", wk)
				}
			}
			// Verify both items are settingPath kind
			for _, item := range cat.items {
				if item.kind != settingPath {
					t.Errorf("item %q: kind = %v, want settingPath", item.key, item.kind)
				}
			}
			return
		}
	}
	t.Error("no Downloads category found in defaultCategories()")
}
```

Run:

```bash
cd "/home/ozogorgor/Projects/Stui Project/stui/tui"
go test ./internal/ui/screens/... -run "TestDefaultCategoriesHasDownloads" -v
```

Expected: PASS.

- [ ] **Step 3: Add SettingsChangedMsg handler cases in ui.go**

In `tui/internal/ui/ui.go`, find the `switch msg.Key {` block inside the `case screens.SettingsChangedMsg:` handler. Find the `case "playback.autoplay_countdown":` block and add the two new cases immediately after it:

```go
		case "downloads.video_dir":
			if v, ok := msg.Value.(string); ok {
				m.state.Settings.VideoDownloadDir = v
			}
		case "downloads.music_dir":
			if v, ok := msg.Value.(string); ok {
				m.state.Settings.MusicDownloadDir = v
			}
```

- [ ] **Step 4: Verify build is clean**

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
