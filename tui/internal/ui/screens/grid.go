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
	Row int // Exported for mouse handling
	Col int // Exported for mouse handling
}

func (g GridCursor) Index(cols int) int {
	return g.Row*cols + g.Col
}

// IsAtTopRow returns true when the cursor is on the first row of the grid.
func (c GridCursor) IsAtTopRow() bool {
	return c.Row == 0
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
	rowH := components.CardTotalRows // authoritative — matches components.RenderCard
	visibleRows := availH / rowH
	if visibleRows < 1 {
		visibleRows = 1
	}

	needsScrollbar := totalRows > visibleRows
	gridWidth := termWidth
	if needsScrollbar {
		gridWidth -= 1 // reserve 1 column for the scrollbar
	}

	cw := components.CardWidth(gridWidth)

	startRow := 0
	if cursor.Row >= visibleRows {
		startRow = cursor.Row - visibleRows + 1
	}
	endRow := min(startRow+visibleRows, totalRows)

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
			selected := (rowIdx == cursor.Row && colIdx == cursor.Col)
			cardStrings = append(cardStrings, components.RenderCard(entries[idx], cw, selected))
		}
		row := lipgloss.JoinHorizontal(lipgloss.Top, cardStrings...)
		rowStrings = append(rowStrings, row)
	}

	gridContent := strings.Join(rowStrings, "\n")

	if !needsScrollbar {
		return fixedHeightGrid(gridContent, termWidth, availH)
	}

	// Wrap the bar in the card-panel background so the seam between the
	// rightmost card and the bar column is invisible against the card
	// background. Scrollbar tracking is in TERMINAL-LINE units, not
	// card-row units: each card-row is rowH terminal lines tall, so the
	// track represents visibleRows*rowH viewport positions out of
	// totalRows*rowH total — passing card-row units would collapse the
	// track to visibleRows positions and the thumb wouldn't move until
	// the very last row.
	bg := lipgloss.NewStyle().Background(theme.T.Bg())
	bar := bg.Render(components.Scrollbar(startRow*rowH, visibleRows*rowH, totalRows*rowH))
	// Force the grid content to exactly gridWidth visual chars so the bar
	// lands flush against the panel's right edge. CardWidth's integer
	// division can leave a few cols of slack; without this clamp,
	// fixedHeightGrid's left-alignment would push the bar inward.
	gridContent = lipgloss.NewStyle().Width(gridWidth).Render(gridContent)
	return fixedHeightGrid(lipgloss.JoinHorizontal(lipgloss.Top, gridContent, bar), termWidth, availH)
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
	next := c.Col + 1
	if next >= components.CardColumns {
		return c
	}
	if c.Row*components.CardColumns+next >= total {
		return c
	}
	c.Col = next
	return c
}

func MoveCursorLeft(c GridCursor) GridCursor {
	if c.Col > 0 {
		c.Col--
	}
	return c
}

func MoveCursorDown(c GridCursor, total int) GridCursor {
	nextRow := c.Row + 1
	if nextRow*components.CardColumns+c.Col >= total {
		// clamp to last item in last row
		lastIdx := total - 1
		c.Row = lastIdx / components.CardColumns
		c.Col = lastIdx % components.CardColumns
		return c
	}
	c.Row = nextRow
	return c
}

func MoveCursorUp(c GridCursor) GridCursor {
	if c.Row > 0 {
		c.Row--
	}
	return c
}
