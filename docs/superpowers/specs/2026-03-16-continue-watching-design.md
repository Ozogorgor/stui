# Continue Watching — Design Spec

**Date:** 2026-03-16
**Status:** Approved

---

## Overview

Add a "Continue Watching" row at the top of the Movies and Series tabs. It surfaces in-progress titles (started but not yet completed) directly in the main grid view, allowing one-keypress resume without navigating through a detail screen.

---

## Entry Point

The Continue Watching row appears inline at the top of the **Movies** and **Series** tabs only. It does not appear on Music, Library, or Collections tabs.

The row is hidden entirely when there are no in-progress items for the current tab. When hidden, the main grid occupies the full view as normal.

---

## Card Layout

Cards match the existing poster grid exactly — same dimensions, same 5-column layout. Each card adds three lines below the poster art:

```
 ┌─────────────┐
 │  poster art │
 │             │
 └─────────────┘
  Breaking Bad
  S3E5 · 1h left
  █████░░░░░░░░
```

- **Line 1:** Title
- **Line 2:** For series with season/episode info: `S{n}E{n} · {time} left`. For movies or series entries without episode info: `Movie · {time} left` / `Series · {time} left`
- **Line 3:** Progress bar (`█` filled, `░` empty), proportional to `Position / Duration`

Items are capped at 5, sorted most-recently-watched first, filtered to match the current tab's type (`ipc.TabMovies` / `ipc.TabSeries`).

---

## Navigation

The view has two sections: the Continue Watching row (when present) and the main grid below it.

- `←` / `→` — move within the currently focused section
- `↓` from the CW row — moves cursor into the first row of the main grid
- `↑` from the top row of the main grid — moves cursor back up to the CW row (if present)
- Tab switching resets `cwCursor` to 0 and `cwFocused` to `true` if CW items exist, otherwise `cwFocused = false` and the grid cursor starts at 0,0. **`switchTab` must reset both `cwCursor` and `cwFocused`.**

---

## Focus State

Rather than adding a new `FocusArea` enum value, CW focus is tracked by a `cwFocused bool` on the Model. To prevent existing key-handler paths (which set `state.Focus = state.FocusResults`) from clobbering CW focus, `cwFocused` is only set/cleared by:

- Arrow key handlers that move between sections
- Tab switch (`switchTab`)
- `Esc` (clears CW focus, returns to grid)

All other `state.Focus` assignments leave `cwFocused` untouched — they operate on a different axis (tab/search/settings focus) and do not conflict.

---

## Interactions

| Key | Action |
|-----|--------|
| `Enter` | Resume immediately from saved position using the last-used provider (`client.PlayFrom`). If `entry.Provider` is empty, fall back to opening the detail screen instead. |
| `i` | Open the detail screen for this title via `openDetail(historyEntryToCatalogEntry(entry))` |
| `d` | Remove via `historyStore.Remove(entry.ID)` + async save. If the row becomes empty, `cwFocused` is set to `false` and focus moves to the grid. |
| `↑ ↓ ← →` | Navigate within and between the CW row and main grid |

---

## Data

**Source:** `historyStore.InProgress()` — returns entries sorted by `LastWatched` descending, excluding completed items (>90% watched threshold).

**Tab filtering:** `entry.Tab == string(ipc.TabMovies)` or `string(ipc.TabSeries)` — use typed constants, not bare string literals.

**On Enter (resume):**
```go
tab := ipc.MediaTab(m.state.ActiveTab.MediaTabID())
client.PlayFrom(entry.ID, entry.Provider, entry.ImdbID, tab, entry.Position)
```

**On `d` (remove):**
```go
historyStore.Remove(entry.ID)   // method already exists
go func() { _ = historyStore.Save() }()
```

---

## Data Model Changes

### `watchhistory.Entry` — add season/episode fields

To display `S3E5` in the card subtitle for series, extend the struct:

```go
type Entry struct {
    // existing fields unchanged …
    Season  int    // 0 = unknown
    Episode int    // 0 = unknown
}
```

Populate at upsert time from the `ipc.CatalogEntry` or `DetailState` that triggers playback. The `nowPlayingEntry` in `ui.go` already captures this context — `Season` and `Episode` can be extracted from the entry's title or from a dedicated field if the catalog returns it.

Display logic: if `entry.Season > 0 && entry.Episode > 0`, show `S{n}E{n} · {time} left`; otherwise show `{type} · {time} left`.

### `historyEntryToCatalogEntry` adapter

The `i` key opens the detail screen which requires an `ipc.CatalogEntry`. A conversion adapter is needed:

```go
func historyEntryToCatalogEntry(e watchhistory.Entry) ipc.CatalogEntry {
    return ipc.CatalogEntry{
        ID:       e.ID,
        Title:    e.Title,
        Year:     &e.Year,    // CatalogEntry.Year is *string
        Provider: e.Provider,
        ImdbID:   &e.ImdbID,  // CatalogEntry.ImdbID is *string
        Tab:      e.Tab,
        // Genre, Rating, PosterArt, Description left empty —
        // detail screen handles zero values gracefully
    }
}
```

This results in a detail screen with reduced metadata (no poster, no rating, no genre). This is acceptable — the user navigated from a history entry, not a full catalog result.

---

## Implementation Scope

### Files to modify

| File | Change |
|------|--------|
| `tui/pkg/watchhistory/history.go` | Add `Season int`, `Episode int` to `Entry` struct |
| `tui/internal/ui/ui.go` | Add `cwCursor int`, `cwFocused bool` to Model; render CW row in `View()`; handle `Enter`/`i`/`d`/arrows for CW; update `switchTab` to reset CW state; add `historyEntryToCatalogEntry` helper |
| `tui/internal/ui/screens/grid.go` | Expose or extract poster card renderer so CW cards reuse the same function |

### Files unchanged

| File | Reason |
|------|--------|
| `tui/pkg/watchhistory/history.go` methods | `InProgress()`, `Remove()`, `Save()`, `Upsert()` all already exist |
| `tui/internal/ipc/ipc.go` | `PlayFrom` already accepts `startPos float64` |
| `tui/internal/state/state.go` | No new `FocusArea` value needed |
| `tui/internal/ui/screens/detail.go` | Unchanged; `openDetail()` used as-is |

---

## Edge Cases

- **No provider recorded:** If `entry.Provider == ""`, open detail screen instead of attempting `PlayFrom`.
- **Entry playback error:** Surfaced via the existing status bar error path — no special handling needed.
- **Completed items:** `InProgress()` already excludes entries at >90% — no extra filtering.
- **Fewer than 5 items:** Row shows only as many cards as exist; no empty placeholder cards.
- **`Season`/`Episode` unknown (older history entries):** Display `Series · {time} left` fallback.
- **Tab switch with CW focused:** `switchTab` resets `cwCursor = 0`; if new tab has CW items `cwFocused = true`, otherwise `cwFocused = false`.
