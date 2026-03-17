# UI Optimizations Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Standardise footer hint bar format across all TUI screens and add `Warn`/`Success` semantic colours to the theme.

**Architecture:** A new `hintBar()` helper in `screens/common.go` formats footer hint tokens with consistent 3-space separators and dim styling; each screen replaces its hand-written footer string with `hintBar()` calls. Two new `Palette` fields (`Warn`, `Success`) and accessor methods on `*Theme` replace hardcoded hex literals in screen files.

**Tech Stack:** Go 1.22, `github.com/charmbracelet/lipgloss`, `github.com/charmbracelet/bubbletea`

> **Note:** This is NOT a git repository. Omit all `git add` / `git commit` steps.

---

## Chunk 1: Foundation + primary screens

### Task 1: Theme — add Warn and Success colours

**Files:**
- Modify: `pkg/theme/theme.go`
- Create: `pkg/theme/theme_test.go`

- [ ] **Step 1: Create the test file**

```go
// pkg/theme/theme_test.go
package theme

import "testing"

func TestDefaultPaletteWarn(t *testing.T) {
	p := Default()
	if string(p.Warn) != "#e5c07b" {
		t.Errorf("Default Palette.Warn = %q, want #e5c07b", p.Warn)
	}
}

func TestDefaultPaletteSuccess(t *testing.T) {
	p := Default()
	if string(p.Success) != "#98c379" {
		t.Errorf("Default Palette.Success = %q, want #98c379", p.Success)
	}
}

func TestThemeWarnMethod(t *testing.T) {
	if got := T.Warn(); string(got) != "#e5c07b" {
		t.Errorf("T.Warn() = %q, want #e5c07b", got)
	}
}

func TestThemeSuccessMethod(t *testing.T) {
	if got := T.Success(); string(got) != "#98c379" {
		t.Errorf("T.Success() = %q, want #98c379", got)
	}
}
```

- [ ] **Step 2: Run tests — verify they fail**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./pkg/theme/...
```

Expected: FAIL (Warn/Success undefined)

- [ ] **Step 3: Add `Warn` and `Success` fields to the `Palette` struct**

In `pkg/theme/theme.go`, in the `Palette` struct, add two fields after `Yellow`:

```go
	Yellow    lipgloss.Color

	Warn    lipgloss.Color // amber — warning indicators
	Success lipgloss.Color // green — success indicators
```

- [ ] **Step 4: Populate in `Default()`**

In the `Default()` function, add after `Yellow`:

```go
		Yellow:    lipgloss.Color("#f59e0b"),

		Warn:    lipgloss.Color("#e5c07b"),
		Success: lipgloss.Color("#98c379"),
```

- [ ] **Step 5: Add accessor methods on `*Theme`**

After the existing `Yellow()` method (line 183), add:

```go
func (t *Theme) Warn() lipgloss.Color    { return t.P().Warn }
func (t *Theme) Success() lipgloss.Color { return t.P().Success }
```

**Note on `FromMatugen()`:** No changes to `FromMatugen()` are needed. That function starts with `p := Default()`, so `Warn` and `Success` are already populated with the correct default values. This follows the established pattern for `Green` and `Yellow` — the file comment at line 99 already documents "(no direct M3 green/yellow → kept from default or derived)".

- [ ] **Step 6: Run tests — verify they pass**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./pkg/theme/...
```

Expected: PASS (4 tests)

- [ ] **Step 7: Build check**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go build ./...
```

Expected: no output, exit 0

---

### Task 2: hintBar helper in `screens/common.go`

**Files:**
- Create: `internal/ui/screens/common.go`
- Create: `internal/ui/screens/common_test.go`

- [ ] **Step 1: Create the test file**

```go
// internal/ui/screens/common_test.go
package screens

import (
	"strings"
	"testing"
)

func TestHintBarLeadingIndent(t *testing.T) {
	result := hintBar("esc back")
	if !strings.HasPrefix(result, "  ") {
		t.Errorf("hintBar result %q should start with two spaces", result)
	}
}

func TestHintBarSingleHintPresent(t *testing.T) {
	result := hintBar("esc back")
	if !strings.Contains(result, "esc back") {
		t.Errorf("hintBar(%q) = %q, want it to contain the hint text", "esc back", result)
	}
}

func TestHintBarMultipleHintsPresent(t *testing.T) {
	result := hintBar("enter play", "esc back")
	if !strings.Contains(result, "enter play") {
		t.Errorf("hintBar result %q should contain 'enter play'", result)
	}
	if !strings.Contains(result, "esc back") {
		t.Errorf("hintBar result %q should contain 'esc back'", result)
	}
}

func TestHintBarNoHints(t *testing.T) {
	// Should not panic with zero arguments.
	_ = hintBar()
}
```

- [ ] **Step 2: Run tests — verify they fail**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... -run TestHintBar
```

Expected: FAIL (hintBar undefined)

- [ ] **Step 3: Create `common.go`**

```go
// internal/ui/screens/common.go
package screens

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/stui/stui/pkg/theme"
)

// hintBar renders a standardised footer hint line.
// Each argument is a pre-formatted "key action" token, e.g. "enter play", "esc back".
// Tokens are joined with 3-space separators and wrapped in dim styling.
// A fresh lipgloss.Style is created per call, consistent with the theme architecture
// (theme.go: "Styles are NOT stored as globals").
func hintBar(hints ...string) string {
	s := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return "  " + s.Render(strings.Join(hints, "   "))
}
```

- [ ] **Step 4: Run tests — verify they pass**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... -run TestHintBar
```

Expected: PASS (4 tests)

- [ ] **Step 5: Full test suite + build check**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./... && go build ./...
```

Expected: all pass, no build errors

---

### Task 3: stream_picker.go — replace hardcoded colours + migrate footer

**Files:**
- Modify: `internal/ui/screens/stream_picker.go`

Background: stream_picker.go uses `lipgloss.Color("#e5c07b")` (warn/amber) at lines 569 and 790, and `lipgloss.Color("#98c379")` (success/green) at lines 571, 697, and 789.

- [ ] **Step 1: Replace hardcoded colour literals**

Find all occurrences of `lipgloss.Color("#e5c07b")` in stream_picker.go. Replace each with `theme.T.Warn()`.

Find all occurrences of `lipgloss.Color("#98c379")` in stream_picker.go. Replace each with `theme.T.Success()`.

This affects lines 569, 571, 697, 789, 790. After replacement the pattern is:

```go
// was: warn := lipgloss.NewStyle().Foreground(lipgloss.Color("#e5c07b"))
warn := lipgloss.NewStyle().Foreground(theme.T.Warn())

// was: green := lipgloss.NewStyle().Foreground(lipgloss.Color("#98c379"))
green := lipgloss.NewStyle().Foreground(theme.T.Success())
```

- [ ] **Step 2: Build check after colour replacement**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go build ./...
```

Expected: no errors

- [ ] **Step 3: Migrate the footer hint line**

In `stream_picker.go`, find the footer block (around line 685):

```go
sb.WriteString("\n" + dim.Render("  \u2191\u2193 navigate   enter play   tab sort   r reverse   esc back") +
    "   " + autoHint + "   " + benchHint + downloadHint +
    "   " + dim.Render("1-4 quality") + "\n")
```

Replace with:

```go
sb.WriteString("\n" + hintBar("↑↓ navigate", "enter play", "tab sort", "r reverse", "esc back") +
    "   " + autoHint + "   " + benchHint + downloadHint +
    "   " + dim.Render("1-4 quality") + "\n")
```

- [ ] **Step 4: Run existing tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

---

### Task 4: settings.go + episode.go — footer migration

**Files:**
- Modify: `internal/ui/screens/settings.go`
- Modify: `internal/ui/screens/episode.go`

#### settings.go

- [ ] **Step 1: Replace the footer block in `settings.go`**

Find this block in `View()` (around lines 458 and 549–555):

```go
footerStyle := lipgloss.NewStyle().
    Foreground(theme.T.TextDim()).
    PaddingLeft(2)
```

and:

```go
var hint string
if m.editing {
    hint = "enter confirm   esc cancel"
} else {
    hint = "↑↓ navigate   enter select/toggle   +/- adjust   ← back   esc exit"
}
footer := footerStyle.Render(hint)
```

Replace both with (remove `footerStyle` entirely, change `footer` assignment):

```go
var footer string
if m.editing {
    footer = hintBar("enter confirm", "esc cancel")
} else {
    footer = hintBar("↑↓ navigate", "enter select/toggle", "+/- adjust", "← back", "esc exit")
}
```

- [ ] **Step 2: Run settings tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

#### episode.go

- [ ] **Step 3: Replace the footer hint in `episode.go`**

Find this line (around line 257):

```go
navHint := dim.Render("  \u2190\u2192\u2191\u2193 navigate   enter play   esc back")
```

Replace with:

```go
navHint := hintBar("←→↑↓ navigate", "enter play", "esc back")
```

- [ ] **Step 4: Run tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

---

### Task 5: downloads.go + audio_track_picker.go

**Files:**
- Modify: `internal/ui/screens/downloads.go`
- Modify: `internal/ui/screens/audio_track_picker.go`

#### downloads.go — colour replacement only (footer uses accent highlights, keep as-is)

- [ ] **Step 1: Replace hardcoded colour in `downloads.go`**

Find (around line 139):

```go
green := lipgloss.NewStyle().Foreground(lipgloss.Color("#98c379"))
```

Replace with:

```go
green := lipgloss.NewStyle().Foreground(theme.T.Success())
```

Note: `lipgloss.Color("#e06c75")` on line 140 is a custom red distinct from the theme's Red — leave it unchanged.

- [ ] **Step 2: Build check**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go build ./...
```

#### audio_track_picker.go — colour replacement + footer migration

- [ ] **Step 3: Replace hardcoded colour in `audio_track_picker.go`**

Find (around line 117):

```go
activeStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#98c379")) // green
```

Replace with:

```go
activeStyle := lipgloss.NewStyle().Foreground(theme.T.Success())
```

- [ ] **Step 4: Migrate the footer in `audio_track_picker.go`**

Find (around line 175):

```go
footer := dimStyle.Render("  ↑↓ navigate   enter select   esc back")
```

Replace with:

```go
footer := hintBar("↑↓ navigate", "enter select", "esc back")
```

- [ ] **Step 5: Run tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

---

## Chunk 2: Remaining screens

### Task 6: music screens — footer migration (dot-space → 3-space)

**Files:**
- Modify: `internal/ui/screens/music_browse.go`
- Modify: `internal/ui/screens/music_queue.go`

#### music_browse.go

- [ ] **Step 1: Migrate footer in `music_browse.go`**

Find (around lines 121–122):

```go
footerText := "  enter add to queue · / search · ↑↓ navigate"
footerLine := dimStyle.Render(footerText)
```

Replace with:

```go
footerLine := hintBar("enter add to queue", "/ search", "↑↓ navigate")
```

#### music_queue.go

- [ ] **Step 2: Migrate footer in `music_queue.go`**

Find (around lines 160–161):

```go
footerText := "  enter play · d remove · c clear · g top · G bottom"
footerLine := dimStyle.Render(footerText)
```

Replace with:

```go
footerLine := hintBar("enter play", "d remove", "c clear", "g top", "G bottom")
```

- [ ] **Step 3: Run tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

---

### Task 7: search + keybinds_editor + collections_screen

**Files:**
- Modify: `internal/ui/screens/search.go`
- Modify: `internal/ui/screens/keybinds_editor.go`
- Modify: `internal/ui/screens/collections_screen.go`

#### search.go

- [ ] **Step 1: Migrate footer in `search.go`**

Find (around line 310):

```go
sb.WriteString("\n" + dim.Render("  ↑↓ navigate   enter open   a toggle scope   esc close") + "\n")
```

Replace with:

```go
sb.WriteString("\n" + hintBar("↑↓ navigate", "enter open", "a toggle scope", "esc close") + "\n")
```

#### keybinds_editor.go

- [ ] **Step 2: Migrate footer in `keybinds_editor.go`**

Find (around lines 185–190):

```go
sb.WriteString("\n")
footer := "  ↑↓ navigate   enter rebind   r reset   R reset all   esc back"
if s.capture {
    footer = "  Press any key to bind  (esc to cancel)"
}
sb.WriteString(dim.Render(footer) + "\n")
```

Replace with:

```go
sb.WriteString("\n")
var footer string
if s.capture {
    footer = hintBar("Press any key to bind", "esc to cancel")
} else {
    footer = hintBar("↑↓ navigate", "enter rebind", "r reset", "R reset all", "esc back")
}
sb.WriteString(footer + "\n")
```

#### collections_screen.go

- [ ] **Step 3: Normalise hint separators in `renderFooter()`**

The collections footer uses a special style (Surface background + full Width) so `hintBar()` cannot be used directly. Instead, normalise the separator format to 3-space within `renderFooter()`.

Find the `renderFooter()` function (around line 581). The current hint strings are:

```go
hint = "  Type collection name  enter confirm  esc cancel"    // 2-space sep
hint = "  Type new name  enter confirm  esc cancel"           // 2-space sep
hint = "  j/k move   → entries   n new   d delete   r rename" // already 3-space
hint = "  j/k move   ← collections   enter open   x remove"  // already 3-space
```

Change the first two hints to use 3-space separators:

```go
hint = "  Type collection name   enter confirm   esc cancel"
hint = "  Type new name   enter confirm   esc cancel"
```

Leave the `lipgloss.NewStyle().Foreground(...).Background(...).Width(...).Render(hint)` wrapper unchanged.

- [ ] **Step 4: Run tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

---

### Task 8: offline_library + stream_radar + rating_weights + help

**Files:**
- Modify: `internal/ui/screens/offline_library.go`
- Modify: `internal/ui/screens/stream_radar.go`
- Modify: `internal/ui/screens/rating_weights.go`
- Modify: `internal/ui/screens/help.go`

#### offline_library.go

- [ ] **Step 1: Migrate footers in `offline_library.go`**

There are three occurrences. Find and replace each:

**Occurrence 1** (empty state, around line 178):
```go
footer := "\n\n  " + dim.Render("q close")
```
→
```go
footer := "\n\n" + hintBar("q close")
```

**Occurrence 2** (another empty-state path, around line 183):
```go
return "  " + header + "\n\n  " + dim.Render("q close")
```
→
```go
return "  " + header + "\n\n" + hintBar("q close")
```

**Occurrence 3** (normal state, around line 277):
```go
footer := "  " + dim.Render("↑↓ navigate   enter open   tab/←→ switch tab   q close")
```
→
```go
footer := hintBar("↑↓ navigate", "enter open", "tab/←→ switch tab", "q close")
```

#### stream_radar.go

- [ ] **Step 2: Migrate footers in `stream_radar.go`**

**Occurrence 1** (empty state, around line 166):
```go
footer := dim.Render("\n  q close")
```
→
```go
footer := "\n" + hintBar("q close")
```

**Occurrence 2** (normal state, around line 241):
```go
footer := "\n\n  " + dim.Render("q close")
```
→
```go
footer := "\n\n" + hintBar("q close")
```

#### rating_weights.go

- [ ] **Step 3: Migrate footer in `rating_weights.go`**

Find (around line 173):
```go
footer := "\n\n  " + dim.Render("q close")
```
→
```go
footer := "\n\n" + hintBar("q close")
```

#### help.go

- [ ] **Step 4: Migrate footer in `help.go`**

Find (around line 54):
```go
sb.WriteString(dim.Render("  esc close") + "\n")
```
→
```go
sb.WriteString(hintBar("esc close") + "\n")
```

- [ ] **Step 5: Run tests + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./internal/ui/screens/... && go build ./...
```

Expected: all pass

---

### Task 9: plugin screens — hardcoded colours + footer migration

**Files:**
- Modify: `internal/ui/screens/plugin_manager.go`
- Modify: `internal/ui/screens/plugin_settings.go`
- Modify: `internal/ui/screens/plugin_registry.go`
- Modify: `internal/ui/screens/plugin_repos.go`

#### plugin_manager.go — colour replacement + footer

- [ ] **Step 1: Replace hardcoded colours in `plugin_manager.go`**

Find all occurrences of `lipgloss.Color("#e5c07b")` (lines 290, 406, 463) and replace with `theme.T.Warn()`:

```go
// was:
warn := lipgloss.NewStyle().Foreground(lipgloss.Color("#e5c07b"))
// becomes:
warn := lipgloss.NewStyle().Foreground(theme.T.Warn())
```

Find all occurrences of `lipgloss.Color("#98c379")` (lines 351, 405) and replace with `theme.T.Success()`:

```go
// was:
green := lipgloss.NewStyle().Foreground(lipgloss.Color("#98c379"))
// becomes:
green := lipgloss.NewStyle().Foreground(theme.T.Success())
```

- [ ] **Step 2: Migrate the footer in `plugin_manager.go`**

Find (around line 343):
```go
sb.WriteString("\n  " + dim.Render("[tab/shift+tab] switch tab  [R] repos  [esc] close") + "\n")
```
→
```go
sb.WriteString("\n" + hintBar("tab/shift+tab switch tab", "R repos", "esc close") + "\n")
```

#### plugin_settings.go — footer only

- [ ] **Step 3: Migrate footer in `plugin_settings.go`**

Find (around lines 355–363):
```go
hint := "↑↓ navigate   tab switch panel   enter edit   esc back/cancel"
if m.editing {
    hint = "Type API key   enter save   esc cancel"
}
var footer string
if m.status != "" {
    footer = accentStyle.Render("  "+m.status) + "\n" + dimStyle.Render("  "+hint)
} else {
    footer = dimStyle.Render("  " + hint)
}
```
→
```go
var hintStr string
if m.editing {
    hintStr = hintBar("Type API key", "enter save", "esc cancel")
} else {
    hintStr = hintBar("↑↓ navigate", "tab switch panel", "enter edit", "esc back/cancel")
}
var footer string
if m.status != "" {
    footer = accentStyle.Render("  "+m.status) + "\n" + hintStr
} else {
    footer = hintStr
}
```

#### plugin_registry.go — footer only

- [ ] **Step 4: Migrate footer in `plugin_registry.go`**

Find (around lines 211–217):
```go
var hint string
if m.installing {
    hint = "  Installing, please wait…"
} else {
    hint = "  ↑↓ navigate   enter install   r refresh   esc back"
}
sb.WriteString(dimStyle.Render(hint) + "\n")
```
→
```go
var hint string
if m.installing {
    hint = dimStyle.Render("  Installing, please wait…")
} else {
    hint = hintBar("↑↓ navigate", "enter install", "r refresh", "esc back")
}
sb.WriteString(hint + "\n")
```

#### plugin_repos.go — footer only

- [ ] **Step 5: Migrate footer in `plugin_repos.go`**

Find (around lines 266–282):
```go
var hint string
switch {
case m.adding:
    hint = "Type URL   enter add   esc cancel"
case m.cursor == 0:
    hint = "↑↓ navigate   a add repo   esc back"
case m.cursor == len(m.repos)+1:
    hint = "↑↓ navigate   enter open registry   esc back"
default:
    hint = "↑↓ navigate   a add   d delete   esc back"
}
var footer string
if m.status != "" {
    footer = warnStyle.Render("  "+m.status) + "\n" + dimStyle.Render("  "+hint)
} else {
    footer = dimStyle.Render("  " + hint)
}
```
→
```go
var hintStr string
switch {
case m.adding:
    hintStr = hintBar("Type URL", "enter add", "esc cancel")
case m.cursor == 0:
    hintStr = hintBar("↑↓ navigate", "a add repo", "esc back")
case m.cursor == len(m.repos)+1:
    hintStr = hintBar("↑↓ navigate", "enter open registry", "esc back")
default:
    hintStr = hintBar("↑↓ navigate", "a add", "d delete", "esc back")
}
var footer string
if m.status != "" {
    footer = warnStyle.Render("  "+m.status) + "\n" + hintStr
} else {
    footer = hintStr
}
```

- [ ] **Step 6: Final full test suite + build**

```
cd "/home/ozogorgor/Projects/Stui Project/stui/tui" && go test ./... && go build ./...
```

Expected: all pass, no build errors
