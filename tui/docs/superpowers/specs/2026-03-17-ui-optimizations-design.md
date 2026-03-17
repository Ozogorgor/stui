# UI Optimizations — Design Spec

**Date:** 2026-03-17
**Status:** Approved

---

## Overview

Two categories of improvement applied project-wide to the TUI screens:

- **Visual polish:** Standardise footer hint bar format across all screens via a shared `hintBar()` helper. Add missing semantic colours (`Warn`, `Success`) to the theme.
- **Code quality / maintainability:** The `hintBar()` helper eliminates duplicated string-building logic across 19 screen files. No style caching is introduced — the theme architecture explicitly requires `lipgloss.NewStyle()` to be called fresh per render (see theme.go lines 3–14: "Styles are NOT stored as globals... lipgloss.NewStyle() is a tiny stack allocation; this is intentional").

---

## Audit Findings

An audit of all 24 non-test screen files identified the following inconsistencies.

### Footer separator format (5 distinct patterns)

| Pattern | Screens |
|---------|---------|
| 3-space `"   "` | settings, stream_picker, episode, search, audio_track_picker, keybinds_editor, plugin_settings, plugin_registry — most common |
| Dot-space `" · "` | music_browse, music_queue, offline_library |
| No separator / spaces only | collections_screen |
| Single-action minimal | help, stream_radar, rating_weights |
| Bracket notation in footer | plugin_manager (`"[tab/shift+tab]..."`) |
| Embedded in header | detail (out of scope) |

**Chosen standard:** 3-space delimiter — used by the majority of screens.

### Hardcoded colours not in theme

| Hex | Semantic meaning | Files using it |
|-----|-----------------|----------------|
| `#e5c07b` | warning / amber | stream_picker, plugin_manager (×3) |
| `#98c379` | success / green | stream_picker, downloads, plugin_manager (×2), audio_track_picker |

---

## Data Model

No data model changes. All changes are in the presentation layer.

---

## Changes

### 1. Theme — new colour methods

**File:** `pkg/theme/theme.go`

Two new fields added to the `Palette` struct (the existing `Yellow` field is `#f59e0b` and `Green` is `#10b981` — the hardcoded screen values differ, so new dedicated fields are required):

```go
// In Palette struct:
Warn    lipgloss.Color // amber — #e5c07b
Success lipgloss.Color // green — #98c379
```

Two new accessor methods on `*Theme`, following the existing pattern:

```go
func (t *Theme) Warn() lipgloss.Color    { return t.palette().Warn }
func (t *Theme) Success() lipgloss.Color { return t.palette().Success }
```

`DefaultPalette()` sets both fields to the above hex values. `FromMatugen()` maps them from the closest matugen palette slot (same as `Yellow` and `Green` respectively, unless a more appropriate slot is available).

All hardcoded `lipgloss.Color("#e5c07b")` and `lipgloss.Color("#98c379")` literals in screen files are replaced with `theme.T.Warn()` and `theme.T.Success()`.

---

### 2. Shared footer helper — `screens/common.go`

**File:** `internal/ui/screens/common.go` *(new)*

```go
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

The leading `"  "` (two spaces) matches the left indent used by the majority of existing footer lines.

**No package-level style vars are introduced.** The theme architecture explicitly disallows global style storage to support live palette swapping via `T.Apply()`.

---

### 3. Footer migration

Each screen listed in the migration scope replaces its hand-written footer string with `hintBar()` calls. Examples:

**Before (stream_picker.go):**
```go
sb.WriteString("\n" + dim.Render("  ↑↓ navigate   enter play   tab sort   r reverse   esc back") + ...)
```

**After:**
```go
sb.WriteString("\n" + hintBar("↑↓ navigate", "enter play", "tab sort", "r reverse", "esc back") + ...)
```

Screens that previously used dot-space (`" · "`) separators (music_browse, music_queue) are updated to 3-space as part of standardisation.

Screens with mode-dependent footers (keybinds_editor capture mode, plugin_settings edit mode, plugin_repos input mode) retain their conditional logic — only the separator and style call are unified.

---

## Files Changed

| File | Change |
|------|--------|
| `pkg/theme/theme.go` | Add `Warn` and `Success` fields to `Palette`; add `Warn()` and `Success()` accessor methods; populate in `DefaultPalette()` and `FromMatugen()` |
| `internal/ui/screens/common.go` | **New:** `hintBar()` helper |
| `internal/ui/screens/settings.go` | Migrate footer to `hintBar()` |
| `internal/ui/screens/stream_picker.go` | Replace hardcoded `#e5c07b`/`#98c379` with theme methods; migrate footer |
| `internal/ui/screens/episode.go` | Migrate footer |
| `internal/ui/screens/downloads.go` | Replace hardcoded `#98c379` with `theme.T.Success()` |
| `internal/ui/screens/music_browse.go` | Migrate footer (dot-space → 3-space) |
| `internal/ui/screens/music_queue.go` | Migrate footer (dot-space → 3-space) |
| `internal/ui/screens/collections_screen.go` | Migrate footer |
| `internal/ui/screens/offline_library.go` | Migrate footer (dot-space → 3-space) |
| `internal/ui/screens/help.go` | Migrate footer |
| `internal/ui/screens/stream_radar.go` | Migrate footer |
| `internal/ui/screens/rating_weights.go` | Migrate footer |
| `internal/ui/screens/search.go` | Migrate footer |
| `internal/ui/screens/audio_track_picker.go` | Replace hardcoded `#98c379` with `theme.T.Success()`; migrate footer |
| `internal/ui/screens/keybinds_editor.go` | Migrate footer (both normal and capture-mode variants) |
| `internal/ui/screens/plugin_settings.go` | Migrate footer (both normal and edit-mode variants) |
| `internal/ui/screens/plugin_registry.go` | Migrate footer |
| `internal/ui/screens/plugin_repos.go` | Migrate footer (all context-dependent variants) |
| `internal/ui/screens/plugin_manager.go` | Replace hardcoded `#e5c07b`/`#98c379` with theme methods; migrate footer |

---

## Files Unchanged

| File | Reason |
|------|--------|
| `internal/ui/screens/detail.go` | Footer is embedded in the header bar — structural change outside scope |
| `internal/ui/screens/music_screen.go` | Container screen; delegates all rendering to sub-screens |
| `internal/ui/screens/grid.go` | Utility renderer; no own footer or header |
| `internal/ui/screens/music_library.go` | Footer not present in available code; no hardcoded colours found |
| `internal/ui/screens/music_playlists.go` | Footer not present in available code; no hardcoded colours found |
| `internal/ui/screens/detail_state.go` | State-only file; no `View()` function |
| `internal/ui/screens/settings_test.go` | Test file |
| `internal/ui/screens/stream_picker_test.go` | Test file |

---

## Testing

- Existing tests (`settings_test.go`, `stream_picker_test.go`) must continue to pass unchanged.
- `go build ./...` must compile clean after all changes.
- No new test files required — `hintBar()` is a pure string transformation with no logic branches to unit-test independently.
- Visual verification: all migrated screens show consistent `"  key action   key action"` footer format.

---

## Out of Scope

- Footer refactor for `detail.go` (embedded header format)
- Empty state / loading state standardisation
- Layout constant centralisation
- Runtime theme switching compatibility (existing `T.Apply()` path already works correctly; no regressions introduced since no style globals are added)
