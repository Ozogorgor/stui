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

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
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
// termWidth/termHeight are used for layout.
func RenderGrid(
	entries []ipc.CatalogEntry,
	cursor GridCursor,
	termWidth, termHeight int,
	isLoading bool,
	runtimeStatus string,
) string {
	availH := termHeight - 7 // account for top bar + status bar

	// ── Loading / empty states ────────────────────────────────────────────
	if isLoading {
		return CenteredMsg(termWidth, availH,
			lipgloss.NewStyle().Foreground(theme.T.Neon()).Render("⠿  Loading…"),
		)
	}
	if len(entries) == 0 {
		hint := "Press / to search"
		if runtimeStatus == "connecting" {
			hint = "Connecting to runtime…"
		} else if runtimeStatus == "error" {
			hint = "Runtime unavailable — try: OMDB_API_KEY=... stui"
		}
		return CenteredMsg(termWidth, availH,
			lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render(hint),
		)
	}

	cw := components.CardWidth(termWidth)
	cols := components.CardColumns

	// Compute visible row window
	totalRows := (len(entries) + cols - 1) / cols
	visibleRows := availH / (components.CardPosterRows + 4 + 2) // card height + meta lines + border
	if visibleRows < 1 {
		visibleRows = 1
	}

	startRow := 0
	if cursor.row >= visibleRows {
		startRow = cursor.row - visibleRows + 1
	}
	endRow := min(startRow+visibleRows, totalRows)

	var rowStrings []string

	for rowIdx := startRow; rowIdx < endRow; rowIdx++ {
		var cardStrings []string

		for colIdx := 0; colIdx < cols; colIdx++ {
			idx := rowIdx*cols + colIdx
			if idx >= len(entries) {
				// Empty filler card to maintain grid alignment
				filler := lipgloss.NewStyle().
					Width(cw + 4). // card width + border + padding
					Height(components.CardPosterRows + 5).
					Render("")
				cardStrings = append(cardStrings, filler)
				continue
			}

			selected := (rowIdx == cursor.row && colIdx == cursor.col)
			card := components.RenderCard(entries[idx], cw, selected)
			cardStrings = append(cardStrings, card)
		}

		row := lipgloss.JoinHorizontal(lipgloss.Top, cardStrings...)
		rowStrings = append(rowStrings, row)
	}

	grid := strings.Join(rowStrings, "\n")
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(termWidth).
		Height(availH).
		Render(grid)
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
