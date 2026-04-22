package screens

// grid.go — renders the Netflix-style poster grid.
//
// Layout (5 columns, dynamic card width):
//
//   ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐
//   │  DU  │ │  OP  │ │  PT  │ │  ZI  │ │  PL  │
//   │      │ │      │ │      │ │      │ │      │
//   │      │ │      │ │      │ │      │ │      │
//   └──────┘ └──────┘ └──────┘ └──────┘ └──────┘
//   Dune     Oppenhe… Poor…    Zone…    Past…
//   2024  ★8.8       2023  ★8.0
//   ◆ Sci-Fi          ◆ Fantasy
//
// Keyboard:
//   h/l or ←/→ — move left/right within a row
//   j/k or ↓/↑ — move down/up a row
//   enter       — select item

import (
	"strings"
	"time"

	"charm.land/bubbles/v2/spinner"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

const (
	loadingTimeout = 30 * time.Second
)

// GridCursor tracks position in the 2D grid.
type GridCursor struct {
	row int
	col int
}

func (g GridCursor) Index(cols int) int {
	return g.row*cols + g.col
}

// IsAtTopRow returns true when the cursor is on the first row of the grid.
func (c GridCursor) IsAtTopRow() bool {
	return c.row == 0
}

// RenderGrid renders the full poster grid for the given entries.
// cursor is the currently focused card index.
// termWidth is the full terminal width; availH is the pre-computed available height.
// loadingStart is the timestamp when loading started (0 if not loading).
// plugins is the list of loaded metadata provider plugins.
// spinner is an optional spinner model for loading animation (can be nil).
// No outer border is applied — the caller (viewMainCard) provides that wrapper.
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
	// so we can reserve space upfront.
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
		gridWidth -= 1 // reserve 1 column for the scrollbar (no extra padding — the card's internal right margin separates it visually)
	}

	cw := components.CardWidth(gridWidth)

	startRow := 0
	if cursor.row >= visibleRows {
		startRow = cursor.row - visibleRows + 1
	}
	endRow := min(startRow+visibleRows, totalRows)

	// Build scrollbar (one styled char per visible row). The component
	// always renders the track — we only allocate the column when
	// totalRows > visibleRows; otherwise gridWidth keeps its full span.
	//
	// Note: we intentionally use components.ScrollbarChars (returns
	// []string — one char per row) rather than components.ScrollbarStyle
	// (a single concatenated string). The grid's per-row render loop below
	// interleaves scrollbar chars into each row, so the per-row shape is
	// load-bearing.
	var sbChars []string
	if needsScrollbar {
		sbStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())
		sbChars = components.ScrollbarChars(startRow, visibleRows, totalRows, sbStyle)
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
		return fixedHeightGrid(strings.Join(rowStrings, "\n"), termWidth, availH)
	}

	// Attach scrollbar: each grid row is rowH terminal lines tall.
	// Distribute the sbChars across rows — one char per visible row index.
	// Pad each line to gridWidth so the scrollbar lands at column gridWidth (flush right).
	sbIdx := 0
	var finalRows []string
	for _, rowStr := range rowStrings {
		lines := strings.Split(rowStr, "\n")
		for li := range lines {
			// Append the scrollbar char to the first line of each card row.
			// Pad the line to gridWidth first so the scrollbar ends up at the rightmost column.
			if li == 0 && sbIdx < len(sbChars) {
				padded := lipgloss.NewStyle().Width(gridWidth).Render(lines[li])
				lines[li] = padded + sbChars[sbIdx]
				sbIdx++
			}
		}
		finalRows = append(finalRows, strings.Join(lines, "\n"))
	}
	return fixedHeightGrid(strings.Join(finalRows, "\n"), termWidth, availH)
}

// fixedHeightGrid forces the grid's output to occupy exactly `availH` rows
// so the parent container doesn't shrink to content height when the grid
// has fewer entries than the viewport can hold.
func fixedHeightGrid(content string, termWidth, availH int) string {
	return lipgloss.NewStyle().
		Width(termWidth).
		Height(availH).
		Align(lipgloss.Left, lipgloss.Top).
		Render(content)
}

// CenteredMsg renders a single message centered in the available space.
func CenteredMsg(w, h int, msg string) string {
	return lipgloss.NewStyle().
		Width(w).
		Height(h).
		Align(lipgloss.Center, lipgloss.Center).
		Background(theme.T.Bg()).
		Render(msg)
}

// ── Cursor movement helpers ───────────────────────────────────────────────────

func MoveCursorRight(c GridCursor, total int) GridCursor {
	next := c.col + 1
	if next >= components.CardColumns {
		return c
	}
	if c.row*components.CardColumns+next >= total {
		return c
	}
	c.col = next
	return c
}

func MoveCursorLeft(c GridCursor) GridCursor {
	if c.col > 0 {
		c.col--
	}
	return c
}

func MoveCursorDown(c GridCursor, total int) GridCursor {
	nextRow := c.row + 1
	if nextRow*components.CardColumns+c.col >= total {
		// clamp to last item in last row
		lastIdx := total - 1
		c.row = lastIdx / components.CardColumns
		c.col = lastIdx % components.CardColumns
		return c
	}
	c.row = nextRow
	return c
}

func MoveCursorUp(c GridCursor) GridCursor {
	if c.row > 0 {
		c.row--
	}
	return c
}
