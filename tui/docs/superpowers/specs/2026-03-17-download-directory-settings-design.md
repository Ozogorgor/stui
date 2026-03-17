# Download Directory Settings — Design Spec

**Date:** 2026-03-17
**Status:** Approved

---

## Overview

Add two user-configurable download directory settings: one for video/movie/series downloads and one for music/audio downloads. Both are editable inline in the Settings screen via a new `settingPath` kind that activates a text input on Enter.

| Setting | Config key | Default |
|---------|-----------|---------|
| Video download dir | `downloads.video_dir` | `~/Videos` |
| Music download dir | `downloads.music_dir` | `~/Music` |

---

## Data Model

### state.Settings

Two new fields added to `Settings` in `internal/state/app_state.go`:

```go
// Downloads
VideoDownloadDir string // default ~/Videos
MusicDownloadDir string // default ~/Music
```

`DefaultSettings()` is extended to populate these from `os.UserHomeDir()`. Because `os.UserHomeDir()` can fail (e.g. missing `$HOME`), an empty result falls back to `"."`:

```go
home, err := os.UserHomeDir()
if err != nil || home == "" {
    home = "."
}
return Settings{
    AutoDeleteVideo:  true,
    VideoDownloadDir: filepath.Join(home, "Videos"),
    MusicDownloadDir: filepath.Join(home, "Music"),
}
```

`app_state.go` gains imports `"os"` and `"path/filepath"`.

**Known limitation:** The TUI populates default values from `os.UserHomeDir()` at startup. If the runtime has a saved config value in `stui.toml` that differs, the settings screen will show the `os.UserHomeDir()`-derived defaults until the user edits them. There is no IPC mechanism for the runtime to push its saved config to the TUI on startup; this is an accepted limitation shared by all current settings.

---

## Settings Screen

### New imports in settings.go

Add to the import block:

```go
"os"
"path/filepath"

"github.com/charmbracelet/bubbles/textinput"
```

### Package-level home directory

Add a package-level variable resolved once at program start:

```go
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

This avoids calling `os.UserHomeDir()` on every render frame. Tests that need a specific home path can set `settingsHomeDir` directly before calling the function under test.

### New settingPath kind

A new `settingPath settingKind` constant is appended to the `settingKind` enum.

**`settingItem` additions:**
- `strVal string` — holds the raw absolute path value

**`displayValue()`** for `settingPath`:

```go
case settingPath:
    if settingsHomeDir == "." {
        return s.strVal // fallback: no home dir, show raw path
    }
    rel, err := filepath.Rel(settingsHomeDir, s.strVal)
    if err == nil && !strings.HasPrefix(rel, "..") {
        return "~/" + rel
    }
    return s.strVal
```

`filepath.Rel` handles the separator correctly and covers the edge case where `strVal` equals `settingsHomeDir` exactly (which `strings.TrimPrefix` with an appended separator would mis-render as `"~//home/user"`). Non-home paths and the `"."` fallback are returned unchanged.

**`toggle()` and `adjust()`** have no case for `settingPath` — they silently no-op for path items. Do NOT add cases; this is correct behaviour (path items are not toggled or incremented).

**`settingChangedCmd()`** adds a `settingPath` case:

```go
case settingPath:
    v = item.strVal
```

This closes the existing nil-value fall-through for unhandled kinds.

### Editing state on SettingsModel

Two new fields on `SettingsModel`:

```go
editing   bool
editInput textinput.Model
```

`editing` is true while a `settingPath` item's text input is active.

### Interaction

**Starting an edit:** The existing `case "enter", "right", "l":` handler must be split into two cases: `case "enter":` and `case "right", "l":`. Both new cases must preserve the existing `!m.inCategory` branch (entering a category from the left panel — `m.inCategory = true; m.itemCursor = 0`). Only the `"enter"` case gets the additional path-editing branch.

Inside the `"enter"` case only, when `m.inCategory` and the focused item has `kind == settingPath`:

```go
if item.kind == settingPath {
    ti := textinput.New()
    ti.SetValue(item.strVal)
    ti.CursorEnd()
    ti.Width = 48       // fits inside the right panel on typical terminals
    ti.CharLimit = 512
    cmd := ti.Focus()   // Focus() returns a cmd for cursor blink — must not be dropped
    m.editInput = ti
    m.editing = true
    return m, cmd
}
// existing toggle + settingChangedCmd for all other kinds
item.toggle()
return m, settingChangedCmd(item)
```

**While editing (`m.editing == true`):** At the top of `Update()`, before any navigation logic, intercept all input:

```go
if m.editing {
    switch msg := msg.(type) {
    case tea.KeyMsg:
        switch msg.String() {
        case "enter":
            // confirm
            cat := m.categories[m.catCursor]
            item := cat.items[m.itemCursor]
            item.strVal = m.editInput.Value()
            m.editing = false
            return m, settingChangedCmd(item)
        case "esc":
            // cancel — no change to strVal
            m.editing = false
            return m, nil
        default:
            // forward to textinput
            newInput, cmd := m.editInput.Update(msg)
            m.editInput = newInput
            return m, cmd
        }
    case tea.MouseMsg:
        // suppress all mouse events (left click, wheel, etc.) during editing
        // to prevent itemCursor from drifting while the input is open
        return m, nil
    }
    return m, nil
}
```

This pattern ensures:
- `"esc"` during editing cancels the edit and does NOT fire the existing `inCategory = false` back-navigation handler below it.
- Mouse scroll/click events do NOT change `catCursor`/`itemCursor` while editing.
- `textinput.Update()` return value (both model and cmd) is always captured.

**Pointer safety:** `cat.items[m.itemCursor]` is a `*settingItem` (pointer), so `item.strVal = m.editInput.Value()` correctly mutates the stored item. The `settingChangedCmd(item)` closure captures the pointer at call time — safe even with Bubble Tea's async cmd dispatch.

**Known limitation — re-mount:** `Init()` currently returns `nil`. If the settings screen is re-mounted while `m.editing == true`, the cursor blink cmd is lost. This is an accepted edge case; path editing is not expected to survive a screen re-mount.

### Display

In the item row renderer, when `m.editing` is true and the row being rendered is the currently focused path item, replace the value column with `m.editInput.View()` directly — do NOT wrap it in `valStyle.Render()`, as `textinput.View()` carries its own ANSI styling which would be corrupted by re-wrapping.

**Footer hint:** While `m.editing` is true the footer hint is replaced with:

```
enter confirm   esc cancel
```

This avoids showing the misleading global `esc exit` hint during path editing.

**Path validation:** No validation is performed in the TUI. The raw string is forwarded to the runtime via `SetConfig`. If the path does not exist or is not writable, the runtime (aria2) handles the error. This keeps the TUI simple and avoids duplicating directory-existence logic.

### New "Downloads" category

`defaultCategories()` uses the package-level `settingsHomeDir` (already resolved by `init()`) for the initial `strVal` values — no signature change needed.

The new category is inserted between "Streaming" and "Subtitles":

```go
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
```

---

## Root Model Wiring

Two new cases inside the `switch msg.Key` block in the `SettingsChangedMsg` handler in `internal/ui/ui.go` (alongside the existing mirror cases after `playback.autoplay_countdown`):

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

The existing `m.client.SetConfig(msg.Key, msg.Value)` call (which runs for all non-visualizer keys before the mirror switch) automatically forwards both keys to the runtime — no additional IPC work needed.

**Persistence:** The runtime owns persistence of config values to `stui.toml`. The TUI forwards the new values via `SetConfig` on each change; the runtime is responsible for writing them to disk. This is the same pattern as all other settings.

---

## Files Changed

| File | Change |
|------|--------|
| `internal/state/app_state.go` | Add `VideoDownloadDir`, `MusicDownloadDir` to `Settings`; populate in `DefaultSettings()` with `os.UserHomeDir()` fallback; add `"os"` and `"path/filepath"` imports |
| `internal/ui/screens/settings.go` | Add `"os"`, `"path/filepath"`, `textinput` imports; add `settingsHomeDir` package-level var + `init()`; add `settingPath` kind; add `strVal` to `settingItem`; update `displayValue()`, `settingChangedCmd()`; add `editing`/`editInput` to `SettingsModel`; split `"enter"` from `"right"/"l"` case; add editing intercept block; suppress mouse during editing; update footer hint during editing; add Downloads category |
| `internal/ui/ui.go` | Add two `SettingsChangedMsg` handler cases |

---

## Files Unchanged

| File | Reason |
|------|--------|
| `internal/ipc/ipc.go` | `SetConfig` already handles arbitrary key/value — no new IPC messages |
| `internal/ui/screens/downloads.go` | Download initiation is handled at the call site (`DownloadStream`), not by this screen; the directory path is passed to aria2 by the runtime from its own config |

---

## Testing

- `displayValue()` for `settingPath`: replaces home dir prefix with `~/`; leaves non-home paths unchanged
- `displayValue()` when `settingsHomeDir == "."`: returns raw `strVal` with no `~/` prefix
- `settingChangedCmd()` for `settingPath`: emits `SettingsChangedMsg` with `strVal` as a `string` (not nil)
- Editing: Enter on path item activates input and returns blink cmd (not nil); Enter again confirms and emits `SettingsChangedMsg` with new value; Esc cancels without changing `strVal`; `"right"`/`"l"` do NOT open the editor
- Mouse events (click and scroll) suppressed while editing
- `DefaultSettings()` populates `VideoDownloadDir` and `MusicDownloadDir` from `os.UserHomeDir()` with `"."` fallback on error
