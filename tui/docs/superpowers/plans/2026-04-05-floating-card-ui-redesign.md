# Floating-Card UI Redesign Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace STUI's half-border chrome with three independent floating rounded-border cards (topbar / main grid / statusbar) separated by 1-row gaps and inset 1 cell from every terminal edge, plus a scrollbar inside the main card.

**Architecture:** Three new/updated lipgloss styles (`TopBarStyle(focused)`, `StatusBarStyle()`, `MainCardStyle(focused)`) drive all visual changes. `RenderGrid` drops its own outer border so `viewMainCard()` in `ui.go` can own the single outer border for the grid. Width/height helpers centralise the chrome budget arithmetic.

**Tech Stack:** Go 1.25, `charm.land/bubbletea/v2`, `charm.land/lipgloss/v2`, `charm.land/bubbles/v2`

---

## File Map

| File | Change |
|---|---|
| `tui/pkg/theme/theme.go` | Update `TopBarStyle` signature, update `StatusBarStyle`, add `MainCardStyle` |
| `tui/pkg/theme/theme_test.go` | Tests for new/updated style signatures and border-color focus logic |
| `tui/internal/ui/screens/grid.go` | Remove `ResultsPanelStyle` wrapper, embed scrollbar inside content, accept explicit `availH` param |
| `tui/internal/ui/screens/grid_test.go` | New file — unit tests for scrollbar rendering and edge cases |
| `tui/internal/ui/ui.go` | `innerWidth()`, `viewTopBar(focused)`, `viewMainCard()`, `View()` gaps, spacer math, `hitTestTopBarWidgets` offset, `WindowSizeMsg` handler |

---

## Chunk 1: Theme Styles

### Task 1: Update `TopBarStyle` and `StatusBarStyle`, add `MainCardStyle`

**Files:**
- Modify: `tui/pkg/theme/theme.go` (functions `TopBarStyle`, `StatusBarStyle`, new `MainCardStyle`)
- Modify: `tui/pkg/theme/theme_test.go`

- [ ] **Step 1.1 — Write failing tests for the new style signatures**

Add to `tui/pkg/theme/theme_test.go`:

```go
func TestTopBarStyleFocusedBorderColor(t *testing.T) {
	// TopBarStyle(true) must use BorderFoc (accent) color.
	// We can't inspect lipgloss internals directly, so we verify
	// that focused/unfocused produce different rendered output on
	// a non-empty string (different border chars will differ in ANSI).
	focused := T.TopBarStyle(true).Width(20).Render("x")
	unfocused := T.TopBarStyle(false).Width(20).Render("x")
	if focused == unfocused {
		t.Error("TopBarStyle(true) and TopBarStyle(false) should produce different output")
	}
}

func TestTopBarStyleHasAllBorders(t *testing.T) {
	s := T.TopBarStyle(false)
	if !s.GetBorderTop() || !s.GetBorderBottom() || !s.GetBorderLeft() || !s.GetBorderRight() {
		t.Error("TopBarStyle must have all four border sides enabled")
	}
}

func TestStatusBarStyleHasAllBorders(t *testing.T) {
	s := T.StatusBarStyle()
	if !s.GetBorderTop() || !s.GetBorderBottom() || !s.GetBorderLeft() || !s.GetBorderRight() {
		t.Error("StatusBarStyle must have all four border sides enabled")
	}
}

func TestMainCardStyleFocusedBorderColor(t *testing.T) {
	focused := T.MainCardStyle(true).Width(20).Render("x")
	unfocused := T.MainCardStyle(false).Width(20).Render("x")
	if focused == unfocused {
		t.Error("MainCardStyle(true) and MainCardStyle(false) should produce different output")
	}
}

func TestMainCardStyleHasAllBorders(t *testing.T) {
	s := T.MainCardStyle(false)
	if !s.GetBorderTop() || !s.GetBorderBottom() || !s.GetBorderLeft() || !s.GetBorderRight() {
		t.Error("MainCardStyle must have all four border sides enabled")
	}
}
```

- [ ] **Step 1.2 — Run tests to confirm they fail**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/theme/... -run "TestTopBarStyle|TestStatusBar|TestMainCard" -v
```

Expected: compile error (`TopBarStyle` takes no args, `MainCardStyle` undefined).

- [ ] **Step 1.3 — Update `TopBarStyle`, `StatusBarStyle`, and add `MainCardStyle` in `theme.go`**

Replace the three style functions in `tui/pkg/theme/theme.go`:

```go
// TopBarStyle returns the chrome style for the top navigation bar.
// focused=true uses the accent border (when search input is active).
func (t *Theme) TopBarStyle(focused bool) lipgloss.Style {
	p := t.P()
	borderColor := p.Border
	if focused {
		borderColor = p.BorderFoc
	}
	return lipgloss.NewStyle().
		Background(p.Surface).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(borderColor).
		PaddingLeft(1).PaddingRight(1).
		MarginLeft(1).MarginRight(1).MarginTop(1)
}

// StatusBarStyle returns the chrome style for the bottom status bar.
// The statusbar is never focused — it always uses the dim border color.
func (t *Theme) StatusBarStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Surface).Foreground(p.TextMuted).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(p.Border).
		PaddingLeft(2).PaddingRight(2).
		MarginLeft(1).MarginRight(1).MarginBottom(1)
}

// MainCardStyle returns the chrome style for the main content area card.
// focused=true uses the accent border (when the grid/content has keyboard focus).
func (t *Theme) MainCardStyle(focused bool) lipgloss.Style {
	p := t.P()
	borderColor := p.Border
	if focused {
		borderColor = p.BorderFoc
	}
	return lipgloss.NewStyle().
		Background(p.Bg).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(borderColor).
		PaddingLeft(1).PaddingRight(1).
		MarginLeft(1).MarginRight(1)
}
```

- [ ] **Step 1.4 — Fix compile errors: update all callers of the old `TopBarStyle()` (no args) signature**

The only existing caller of `TopBarStyle()` is in `tui/internal/ui/ui.go` — the temporary call `theme.T.TopBarStyle()` will become `theme.T.TopBarStyle(false)` for now (focus wiring comes in Task 3). Search for all call sites:

```bash
grep -n "TopBarStyle\(\)" /home/ozogorgor/Projects/Stui_Project/stui/tui/internal/ui/ui.go
```

Replace each occurrence of `theme.T.TopBarStyle()` with `theme.T.TopBarStyle(false)` temporarily — Task 3 wires the real focus value.

- [ ] **Step 1.5 — Run tests to confirm they pass**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/theme/... -v
```

Expected: all pass including the 5 new tests.

- [ ] **Step 1.6 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors.

- [ ] **Step 1.7 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add pkg/theme/theme.go pkg/theme/theme_test.go internal/ui/ui.go && git commit -m "feat(theme): add MainCardStyle, make TopBarStyle focus-aware, full borders on all card styles"
```

---

## Chunk 2: Grid Scrollbar Refactor

### Task 2: Refactor `RenderGrid` — remove outer border, embed scrollbar inside content

**Files:**
- Modify: `tui/internal/ui/screens/grid.go`
- Create: `tui/internal/ui/screens/grid_test.go`

**Context:** `RenderGrid` currently:
1. Wraps all grid content in `ResultsPanelStyle` (a full rounded border)
2. Places the scrollbar *outside* that border, to its right

After this task:
1. `RenderGrid` returns raw content (no outer border — `viewMainCard` provides that)
2. The scrollbar column is embedded *inside* the content block, appended to the right with a 1-space gap
3. `availH` is accepted as an explicit parameter (caller pre-computes it)
4. Scrollbar uses `Accent` for thumb, `Border` for track

- [ ] **Step 2.1 — Create `tui/internal/ui/screens/grid_test.go` with failing tests**

```go
package screens

import (
	"strings"
	"testing"

	"github.com/stui/stui/internal/ipc"
	"charm.land/bubbles/v2/spinner"
)

func makeEntries(n int) []ipc.CatalogEntry {
	entries := make([]ipc.CatalogEntry, n)
	for i := range entries {
		entries[i] = ipc.CatalogEntry{ID: string(rune('a' + i)), Title: "Title"}
	}
	return entries
}

// RenderGrid must not wrap content in an outer rounded border.
// We detect an outer border by checking that the first line starts with '╭'
// (the top-left corner of a RoundedBorder). After the refactor it must NOT.
func TestRenderGridNoOuterBorder(t *testing.T) {
	entries := makeEntries(3)
	result := RenderGrid(entries, GridCursor{}, 120, 20, false, 0, "ready", []string{"test"}, nil)
	firstLine := strings.SplitN(result, "\n", 2)[0]
	if strings.HasPrefix(strings.TrimLeft(firstLine, " "), "╭") {
		t.Error("RenderGrid must not start with a rounded border corner — outer border is now provided by MainCardStyle")
	}
}

// When totalRows > visibleRows the returned string must contain a scrollbar
// character (█ or │) somewhere in the rightmost column.
func TestRenderGridScrollbarPresentWhenOverflow(t *testing.T) {
	// 30 entries at 120 cols will produce many rows; availH=8 forces overflow.
	entries := makeEntries(30)
	result := RenderGrid(entries, GridCursor{}, 120, 8, false, 0, "ready", []string{"test"}, nil)
	if !strings.Contains(result, "█") && !strings.Contains(result, "│") {
		t.Error("RenderGrid must render a scrollbar (█ or │) when content overflows")
	}
}

// When content fits entirely (1 row, large availH), no scrollbar chars appear.
func TestRenderGridNoScrollbarWhenNoOverflow(t *testing.T) {
	entries := makeEntries(3) // 1 row of posters
	result := RenderGrid(entries, GridCursor{}, 120, 40, false, 0, "ready", []string{"test"}, nil)
	// The scrollbar track char '│' may appear in card art, but '▐' and '▌' are
	// exclusive to the scrollbar thumb edges.
	if strings.Contains(result, "▐") || strings.Contains(result, "▌") {
		t.Error("RenderGrid must not render scrollbar thumb glyphs when no overflow")
	}
}

// Zero availH must return empty string without panicking.
func TestRenderGridZeroAvailH(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			t.Errorf("RenderGrid panicked with availH=0: %v", r)
		}
	}()
	entries := makeEntries(10)
	result := RenderGrid(entries, GridCursor{}, 120, 0, false, 0, "ready", []string{"test"}, nil)
	_ = result // may be empty string — just must not panic
}

// isLoading=true must return a centred loading message without panicking.
func TestRenderGridLoadingState(t *testing.T) {
	var s spinner.Model
	result := RenderGrid(nil, GridCursor{}, 80, 10, true, 0, "connecting", nil, &s)
	if result == "" {
		t.Error("RenderGrid with isLoading=true should return a non-empty loading message")
	}
}
```

- [ ] **Step 2.2 — Run tests to confirm they fail**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./internal/ui/screens/... -run "TestRenderGrid" -v
```

Expected: `TestRenderGridNoOuterBorder` and `TestRenderGridScrollbarPresentWhenOverflow` FAIL (outer border present, scrollbar outside it). Others may pass or fail.

- [ ] **Step 2.3 — Rewrite `RenderGrid` in `grid.go`**

Replace the body of `RenderGrid` with the following. The signature changes: `termHeight int` becomes `availH int` (caller provides the pre-computed available height).

**New signature:**
```go
func RenderGrid(
	entries []ipc.CatalogEntry,
	cursor GridCursor,
	termWidth, availH int,
	isLoading bool,
	loadingStart int64,
	runtimeStatus string,
	plugins []string,
	spinner *spinner.Model,
) string {
```

**New body** (replace everything from `availH := termHeight - 7` through `return gridBox`):

```go
	if availH <= 0 {
		return ""
	}

	// ── Loading / empty states ────────────────────────────────────────────
	if isLoading {
		if loadingStart > 0 {
			elapsed := time.Since(time.Unix(loadingStart, 0))
			if elapsed > loadingTimeout {
				hint := "Loading timed out"
				if runtimeStatus == "error" {
					hint = "Runtime unavailable"
				}
				return CenteredMsg(termWidth, availH,
					lipgloss.NewStyle().Foreground(theme.T.Yellow()).Render("⚠ "+hint),
				)
			}
		}
		spinnerView := "Loading…"
		if spinner != nil {
			spinnerView = spinner.View() + " Loading…"
		}
		return CenteredMsg(termWidth, availH,
			lipgloss.NewStyle().Foreground(theme.T.Neon()).Render(spinnerView),
		)
	}
	if len(entries) == 0 {
		hint := "Press / to search"
		if runtimeStatus == "connecting" {
			hint = "Connecting to runtime…"
		} else if runtimeStatus == "error" {
			hint = "Runtime unavailable — try: OMDB_API_KEY=... stui"
		} else if len(plugins) == 0 {
			hint = "No metadata sources — install provider plugins"
		}
		return CenteredMsg(termWidth, availH,
			lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render(hint),
		)
	}

	// Determine if scrollbar is needed before computing card width,
	// so we can reserve 2 columns (1 gap + 1 scrollbar) upfront.
	cols := components.CardColumns
	totalRows := (len(entries) + cols - 1) / cols
	rowH := components.CardPosterRows + 4 + 2 // card height + meta lines + border
	visibleRows := availH / rowH
	if visibleRows < 1 {
		visibleRows = 1
	}

	needsScrollbar := totalRows > visibleRows
	gridWidth := termWidth
	if needsScrollbar {
		gridWidth -= 2 // reserve 1 gap + 1 scrollbar column
	}

	cw := components.CardWidth(gridWidth)

	startRow := 0
	if cursor.row >= visibleRows {
		startRow = cursor.row - visibleRows + 1
	}
	endRow := min(startRow+visibleRows, totalRows)

	// Build scrollbar track (one char per visible row).
	var sbChars []string
	if needsScrollbar {
		thumbH := max(1, visibleRows*visibleRows/max(1, totalRows))
		thumbTop := 0
		if totalRows > visibleRows {
			thumbTop = startRow * (visibleRows - thumbH) / max(1, totalRows-visibleRows)
		}
		accentStr := lipgloss.NewStyle().Foreground(theme.T.Accent()).Render
		trackStr := lipgloss.NewStyle().Foreground(theme.T.Border()).Render
		for i := range visibleRows {
			inThumb := i >= thumbTop && i < thumbTop+thumbH
			if !inThumb {
				sbChars = append(sbChars, trackStr("│"))
				continue
			}
			if thumbH == 1 {
				sbChars = append(sbChars, accentStr("█"))
				continue
			}
			switch i {
			case thumbTop:
				sbChars = append(sbChars, accentStr("▐"))
			case thumbTop + thumbH - 1:
				sbChars = append(sbChars, accentStr("▌"))
			default:
				sbChars = append(sbChars, accentStr("█"))
			}
		}
	}

	// Render grid rows.
	var rowStrings []string
	for rowIdx := startRow; rowIdx < endRow; rowIdx++ {
		var cardStrings []string
		for colIdx := 0; colIdx < cols; colIdx++ {
			idx := rowIdx*cols + colIdx
			if idx >= len(entries) {
				filler := lipgloss.NewStyle().
					Width(cw + 4).
					Height(rowH).
					Render("")
				cardStrings = append(cardStrings, filler)
				continue
			}
			selected := (rowIdx == cursor.row && colIdx == cursor.col)
			cardStrings = append(cardStrings, components.RenderCard(entries[idx], cw, selected))
		}
		row := lipgloss.JoinHorizontal(lipgloss.Top, cardStrings...)
		rowStrings = append(rowStrings, row)
	}

	if !needsScrollbar {
		return strings.Join(rowStrings, "\n")
	}

	// Attach scrollbar: each grid row is rowH terminal lines tall.
	// Distribute the sbChars across rows — one char per visible row.
	sbIdx := 0
	var finalRows []string
	for _, rowStr := range rowStrings {
		lines := strings.Split(rowStr, "\n")
		for li, line := range lines {
			// Append the scrollbar char to the first line of each card row.
			if li == 0 && sbIdx < len(sbChars) {
				lines[li] = line + " " + sbChars[sbIdx]
				sbIdx++
			}
		}
		finalRows = append(finalRows, strings.Join(lines, "\n"))
	}
	return strings.Join(finalRows, "\n")
```

- [ ] **Step 2.4 — Fix the caller in `ui.go` to pass `availH` instead of `m.state.Height`**

In `tui/internal/ui/ui.go`, find the `RenderGrid` call (currently passes `m.state.Width, m.state.Height`). Change it to pass a local `availH`:

```go
availH := max(0, m.state.Height-12)
grid := screens.RenderGrid(
    m.currentGridEntries(),
    m.gridCursor,
    m.state.Width,
    availH,
    m.state.IsLoading,
    m.state.LoadingStart,
    m.state.RuntimeStatus.String(),
    m.state.Plugins,
    &m.loadingSpinner,
)
```

Note: `m.state.Width` is still the full terminal width here — `viewMainCard` (Task 3) will provide the outer border. `RenderGrid` now owns only the inner content.

- [ ] **Step 2.5 — Run the new grid tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./internal/ui/screens/... -run "TestRenderGrid" -v
```

Expected: all 5 pass.

- [ ] **Step 2.6 — Run full test suite**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./... 
```

Expected: all pass.

- [ ] **Step 2.7 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add internal/ui/screens/grid.go internal/ui/screens/grid_test.go internal/ui/ui.go && git commit -m "feat(grid): remove outer border from RenderGrid, embed scrollbar inside content"
```

---

## Chunk 3: Layout Wiring

### Task 3: Wire floating-card layout in `ui.go`

**Files:**
- Modify: `tui/internal/ui/ui.go`

This task wires together all the pieces: outer margin via style-level margins on each card, 1-row gap rows, focus-aware border colors, corrected spacer arithmetic, and updated mouse hit-test offsets.

**Key constants after the redesign:**
- `m.innerWidth()` = `max(0, m.state.Width - 6)` — content area inside `MainCardStyle` border+padding
- `availH` for grid = `max(0, m.state.Height - 12)` — already wired in Task 2
- `topBarPaddingLeft` = `3` — MarginLeft(1) + BorderLeft(1) + PaddingLeft(1)
- Topbar content width = `m.state.Width - 6` — used for spacer math
- StatusBar content width = `m.state.Width - 8` — used for gap math

- [ ] **Step 3.1 — Add `innerWidth()` helper and update `WindowSizeMsg` handler**

Add after the `Model` methods already in `ui.go` (near the helper functions section):

```go
// innerWidth returns the usable content width inside MainCardStyle
// (terminal width minus margins, border, and padding: 1+1+1+1+1+1 = 6).
// Floored at 0 to prevent negative dimensions on tiny terminals.
func (m Model) innerWidth() int {
	return max(0, m.state.Width-6)
}
```

In the `tea.WindowSizeMsg` case of `Update`, after setting `m.state.Width` and `m.state.Height`, update the music/collections screens to use the inner dimensions:

```go
case tea.WindowSizeMsg:
    m.state.Width = msg.Width
    m.state.Height = msg.Height
    m.search.SetWidth(max(20, m.innerWidth()/3))
    innerMsg := tea.WindowSizeMsg{Width: m.innerWidth(), Height: max(0, msg.Height - 12)}
    m.musicScreen, _ = m.musicScreen.Update(innerMsg)
    m.collectionsScreen = m.collectionsScreen.SetSize(m.innerWidth(), max(0, msg.Height-12))
```

- [ ] **Step 3.2 — Update `viewTopBar` to accept and use `focused bool`**

Change the signature and body in `ui.go`:

```go
func (m Model) viewTopBar(focused bool) string {
    w := m.state.Width
```

Update the `Width` call and spacer arithmetic. The topbar content width (inside border+padding but not margin) is `w - 6`:

```go
    contentW := w - 6
    spacerLeft := max(0, (contentW/2)-tabsW-(searchW/2))
    spacerRight := max(0, contentW-tabsW-searchW-gearW-spacerLeft)

    row := tabs + strings.Repeat(" ", spacerLeft) + searchBox + strings.Repeat(" ", spacerRight) + gear
    return theme.T.TopBarStyle(focused).Width(w - 2).Render(row)
```

(Recall: `.Width(w-2)` sets the box outer width to `w-2`; the style-level `MarginLeft(1)+MarginRight(1)` adds back 2 columns to reach terminal width `w`.)

- [ ] **Step 3.3 — Add `viewMainCard()` and update `View()`**

Add new method:

```go
func (m Model) viewMainCard() string {
    focused := m.state.Focus != state.FocusSearch
    inner := m.viewMain()
    return theme.T.MainCardStyle(focused).Width(m.state.Width - 2).Render(inner)
}
```

Update `View()` to use gaps and the new methods:

```go
func (m Model) View() tea.View {
    if m.state.Width == 0 {
        return tea.NewView("Loading…")
    }
    var content string
    if m.screen == screenDetail && m.detail != nil {
        overlay := screens.RenderDetailOverlay(
            m.detail,
            m.state.Width,
            m.state.Height,
            m.state.ActiveTab,
            m.state.RuntimeStatus.String(),
        )
        content = m.applyToast(overlay)
    } else {
        base := lipgloss.JoinVertical(lipgloss.Left,
            m.viewTopBar(m.state.Focus == state.FocusSearch),
            "",
            m.viewMainCard(),
            "",
            m.viewStatusBar(),
        )
        content = m.applyToast(base)
    }
    v := tea.NewView(content)
    v.AltScreen = true
    v.MouseMode = tea.MouseModeCellMotion
    return v
}
```

- [ ] **Step 3.4 — Update `viewStatusBar` spacer formula**

The statusbar content width is `w - 8` (margin 1+1, border 1+1, padding 2+2):

```go
func (m Model) viewStatusBar() string {
    w := m.state.Width
    // ... (pill, screenIndicator, statusMsg, right — unchanged) ...
    contentW := w - 8
    gap := max(0, contentW-lipgloss.Width(pill)-lipgloss.Width(screenIndicator)-lipgloss.Width(statusMsg)-lipgloss.Width(right))
    bar := pill + screenIndicator + statusMsg + strings.Repeat(" ", gap) + right
    return theme.T.StatusBarStyle().Width(w - 2).Render(bar)
}
```

- [ ] **Step 3.5 — Fix `hitTestTopBarWidgets` mouse offset**

Find the constant in `hitTestTopBarWidgets`:

```go
// TopBarStyle has MarginLeft(1) + BorderLeft(1) + PaddingLeft(1) = 3.
const topBarPaddingLeft = 3
```

Also update `hitTestTopTabBar` to use the same offset — the tabs now start at terminal column 3, not 1:

```go
func (m Model) hitTestTopTabBar(x int) (state.Tab, bool) {
    pos := 3 // MarginLeft(1) + BorderLeft(1) + PaddingLeft(1)
    for _, t := range state.Tabs() {
        // ... rest unchanged ...
        if x >= pos && x < pos+w {
            return t, true
        }
        pos += w
    }
    return 0, false
}
```

- [ ] **Step 3.6 — Update `RenderGrid` call in `viewMain()` to use `innerWidth()`**

In `viewMain()`, the `RenderGrid` call currently passes `m.state.Width` for `termWidth`. Change to `m.innerWidth()` so card width calculations use the content area:

```go
availH := max(0, m.state.Height-12)
grid := screens.RenderGrid(
    m.currentGridEntries(),
    m.gridCursor,
    m.innerWidth(),   // ← was m.state.Width
    availH,
    m.state.IsLoading,
    m.state.LoadingStart,
    m.state.RuntimeStatus.String(),
    m.state.Plugins,
    &m.loadingSpinner,
)
```

- [ ] **Step 3.7 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors.

- [ ] **Step 3.8 — Run full test suite**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./...
```

Expected: all pass.

- [ ] **Step 3.9 — Smoke test: launch the app and verify layout visually**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go run ./cmd/stui/main.go --no-runtime
```

Check:
- Three floating rounded-border cards visible (topbar / main / statusbar)
- 1-cell gap from all terminal edges
- 1 empty row between each card
- Main card border glows in accent purple (it has focus at startup)
- Spinner visible and animating in the main card
- Resize terminal: cards reflow cleanly

- [ ] **Step 3.10 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add internal/ui/ui.go && git commit -m "feat(layout): floating-card layout with outer margins, gap rows, focus-aware borders, corrected spacer math"
```
