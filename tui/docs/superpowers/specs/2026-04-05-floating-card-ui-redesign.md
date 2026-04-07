# STUI Floating-Card UI Redesign

**Date:** 2026-04-05  
**Status:** Approved  
**Inspired by:** rmpc (https://github.com/mierak/rmpc)

---

## Overview

Replace STUI's current half-border chrome (topbar with only a bottom line, statusbar with only a top line, bare main content area) with a **floating-card layout**: three independent rounded-border boxes separated by 1-row gaps and inset 1 cell from every terminal edge. The result is clean visual separation between chrome zones without the overhead of a full GUI.

---

## Layout Structure

```
                    ← 1-cell outer margin →
 ╭─────────────────── topbar card ───────────────────╮  ↑
 │ [Movies] [Series] [●Music] [Collections]  ⌕  ⚙  │  1-cell top margin
 ╰───────────────────────────────────────────────────╯
                    ← 1-row gap →
 ╭─────────────────── main card (focused) ───────────╮  ↑
 │  ╭────────╮  ╭────────╮  ╭────────╮            ▐  │  accent border
 │  │ POSTER │  │ POSTER │  │ POSTER │            █  │  when focused
 │  │ ██████ │  │ ██████ │  │ ██████ │            █  │
 │  ╰────────╯  ╰────────╯  ╰────────╯            ▌  │
 │  Title        Title        Title                │  │
 │  ╭────────╮  ╭────────╮  ╭────────╮            │  │
 │  │ POSTER │  │ POSTER │  │ POSTER │            │  │
 │  ╰────────╯  ╰────────╯  ╰────────╯            │  │
 │  Title        Title        Title                │  │
 ╰───────────────────────────────────────────────────╯
                    ← 1-row gap →
 ╭─────────────────── statusbar card ────────────────╮  ↑
 │ ■ stui  ◦ grid  Loading…          Movies 6 titles │  1-cell bottom margin
 ╰───────────────────────────────────────────────────╯
```

All three cards are rendered with `.Width(w - 2)`. Lipgloss v2 treats `Width()` as the total outer width **not including margins**. The style-level `MarginLeft(1).MarginRight(1)` adds the remaining 2 columns, so each card consumes exactly `w` terminal columns in total.

---

## Theme Changes (`pkg/theme/theme.go`)

### `TopBarStyle(focused bool)`
- **Before:** `BorderBottom(true)` only, no margin
- **After:** Full `RoundedBorder()` all sides, `MarginLeft(1).MarginRight(1).MarginTop(1)`
- Border color: `BorderFoc` when `focused == true`, `Border` otherwise

### `StatusBarStyle()`
- **Before:** `BorderTop(true)` only, no margin
- **After:** Full `RoundedBorder()` all sides, `MarginLeft(1).MarginRight(1).MarginBottom(1)`
- Border color: always `Border` (statusbar is never focused)

### New `MainCardStyle(focused bool)`
- Full `RoundedBorder()` all sides
- `MarginLeft(1).MarginRight(1)` (no top/bottom margin — gaps come from the empty rows in `View()`)
- Border color: `BorderFoc` when `focused == true`, `Border` otherwise

---

## Layout Rendering (`internal/ui/ui.go`)

### `View()`
Replace:
```go
base := lipgloss.JoinVertical(lipgloss.Left,
    m.viewTopBar(),
    m.viewMain(),
    m.viewStatusBar(),
)
```
With:
```go
base := lipgloss.JoinVertical(lipgloss.Left,
    m.viewTopBar(),
    "",                  // 1-row gap
    m.viewMainCard(),
    "",                  // 1-row gap
    m.viewStatusBar(),
)
```

### `viewTopBar(focused bool)`
Accepts a `focused` parameter, passes it to `TopBarStyle(focused)`. Called with `focused = (m.state.Focus == state.FocusSearch)`.

### `viewMainCard()`
New method. Calls `viewMain()` to get the inner content string, then wraps it in `MainCardStyle(focused)` where `focused = (m.state.Focus != state.FocusSearch)`.

### Available height for main content

Both `availH` and `innerWidth()` must be floored at 0 to prevent negative dimensions on extremely small terminals:

```go
func (m Model) innerWidth() int { return max(0, m.state.Width - 6) }
availH := max(0, m.state.Height - 12)
```

`RenderGrid` and `CenteredMsg` should early-return an empty string when either dimension is 0 — this matches the existing pattern in `RenderGrid` (`if len(entries) == 0 { return ... }`).

Chrome row count breakdown:

| Section | Rows |
|---|---|
| TopBar card: top margin + top border + content + bottom border | 4 |
| Gap row | 1 |
| Main card: top border + bottom border (content is `viewMain` output) | 2 |
| Gap row | 1 |
| StatusBar card: top border + content + bottom border + bottom margin | 4 |
| **Total chrome** | **12** |

`viewMain()` computes available inner height as:

```go
availH := m.state.Height - 12
```

This replaces the current `m.state.Height - 7`. The `availH` value is passed directly to `RenderGrid` — no secondary subtraction needed because `viewMainCard()` wraps the already-rendered string and lipgloss does not re-clip its height.

---

## Grid Scrollbar (`internal/ui/screens/grid.go`)

### Current state
No scrollbar. Row-based virtualization exists (`visibleRows`, `start`/`end` window) but no position indicator is rendered.

### New scrollbar
A vertical scrollbar column is appended to the right of the grid content block, inside the `MainCardStyle` border.

**Dimensions:** 1 column wide, `visibleRows` rows tall.

**Characters:**
- Thumb (filled): `█`
- Partial top edge of thumb: `▐`
- Partial bottom edge of thumb: `▌`
- Track (empty): `│`

**Logic (integer arithmetic):**
```
// Guard: only render when content overflows
if totalRows <= visibleRows { return gridContent }

thumbH   = max(1, visibleRows * visibleRows / max(1, totalRows))
thumbTop = scrollOffset * (visibleRows - thumbH) / max(1, totalRows - visibleRows)

for i in 0..visibleRows:
    if   i < thumbTop || i >= thumbTop+thumbH  → '│'   // track
    elif thumbH == 1                           → '█'   // single-cell thumb: full block
    elif i == thumbTop                         → '▐'   // top edge
    elif i == thumbTop + thumbH - 1            → '▌'   // bottom edge
    else                                       → '█'   // mid fill
```

When `thumbH == 1`, both the top-edge and bottom-edge conditions would overlap on the same row. The `thumbH == 1` check is evaluated first so the single-cell thumb always renders as the unambiguous `█`.

The `max(1, totalRows)` guard prevents divide-by-zero when called with an empty grid. The `thumbTop` denominator `max(1, totalRows - visibleRows)` is already guarded.

**Placement:** Each row of the grid content string gets the corresponding scrollbar character appended after a 1-space gap:

```
row[i] = gridRowString + " " + scrollbarChar[i]
```

Grid content width shrinks by 2 columns (1 gap + 1 scrollbar) when scrollbar is rendered. `RenderGrid` must subtract 2 from its `termWidth` argument when computing `cw` and `cols`.

**Color:** Thumb in `theme.T.Accent()`, track in `theme.T.Border()`.

---

## Width Budget

With terminal width `w`, all cards are rendered with `.Width(w - 2)`:

| Zone | `.Width()` arg | Margins | Border (L+R) | Padding (L+R) | Content width |
|---|---|---|---|---|---|
| Topbar card | `w - 2` | 1+1 (outside Width) | 1+1 | 1+1 | `w - 2 - 2 - 2 = w - 6` |
| Main card | `w - 2` | 1+1 (outside Width) | 1+1 | 1+1 | `w - 6` (minus 2 for scrollbar = `w - 8`) |
| Statusbar card | `w - 2` | 1+1 (outside Width) | 1+1 | 2+2 | `w - 2 - 2 - 4 = w - 8` |

**Note on lipgloss v2:** `Width(x)` sets the total rendered width of the box including borders and padding but *excluding* margins. The margin is added outside. So `.Width(w-2).MarginLeft(1).MarginRight(1)` produces a string that is `w` terminal columns wide.

### Spacer formulas

**`viewTopBar`** — content width is `w - 6`. Spacer right:
```go
spacerRight := max(0, (w-6) - tabsW - searchW - gearW - spacerLeft)
```

**`viewStatusBar`** — content width is `w - 8`. Gap between left elements and right label:
```go
gap := max(0, (w-8) - lipgloss.Width(pill) - lipgloss.Width(screenIndicator) - lipgloss.Width(statusMsg) - lipgloss.Width(right))
```

### `hitTestTopBarWidgets` mouse offset

`hitTestTopBarWidgets` in `ui.go` uses a hardcoded `topBarPaddingLeft = 1`. After the redesign the left frame offset is `MarginLeft(1) + BorderLeft(1) + PaddingLeft(1) = 3`. Update:

```go
const topBarPaddingLeft = 3
```

---

## Focus State

| `m.state.Focus` | Topbar border | Main card border |
|---|---|---|
| `FocusSearch` | `BorderFoc` (accent) | `Border` (dim) |
| anything else | `Border` (dim) | `BorderFoc` (accent) |

StatusBar border is always `Border` (dim).

---

## Music and Collections Width

`musicScreen.View()` and `collectionsScreen.View()` use `m.state.Width` internally. After the redesign the usable inner width is `w - 6` (card border + padding), but `m.state.Width` remains the terminal width `w`. Both screens must be passed the reduced inner width. Add a helper:

```go
func (m Model) innerWidth() int { return max(0, m.state.Width - 6) }
```

Pass `m.innerWidth()` wherever these screens currently receive `m.state.Width` for layout purposes. The `WindowSizeMsg` handler should also call `m.musicScreen.SetWidth(m.innerWidth())` (or equivalent) alongside `m.state.Width = msg.Width`.

---

## Files Changed

| File | Change |
|---|---|
| `pkg/theme/theme.go` | `TopBarStyle(focused bool)`, `StatusBarStyle()`, new `MainCardStyle(focused bool)` |
| `internal/ui/ui.go` | `View()` gap rows, `viewTopBar(focused)`, new `viewMainCard()`, `availH = Height-12`, spacer formulas updated, `hitTestTopBarWidgets` `topBarPaddingLeft` 1→3, music/collections `innerWidth()` helper, `WindowSizeMsg` handler |
| `internal/ui/screens/grid.go` | Vertical scrollbar in `RenderGrid`, grid width reduces by 2 when scrollbar visible |
| `internal/ui/screens/common.go` | `CenteredMsg` height arg updated if needed |
| `internal/ui/screens/dims.go` | Any shared dimension constants |

---

## Out of Scope

- Changing card colors/palette (existing palette unchanged)
- Changing tab pill styles
- Changing the search box inner border style
- List view (`screenList`) — scrollbar already exists there
