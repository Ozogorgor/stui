// Package layout provides reusable layout calculation helpers for the stui TUI.
//
// BubbleTea layout logic tends to grow as the UI matures — column counts,
// responsive breakpoints, panel ratios, and flex distribution all end up
// scattered across multiple render functions.  Centralising them here keeps
// the screen and component files focused on rendering, not arithmetic.
package layout

// GridLayout describes the column/row geometry of the poster grid.
type GridLayout struct {
	// Cols is the number of poster columns that fit at the current terminal width.
	Cols int
	// CardWidth is the width in cells of each poster card (including padding).
	CardWidth int
	// CardHeight is the height in cells of each poster card.
	CardHeight int
	// HorizontalPad is the total horizontal padding between the left edge and
	// the first column (centres the grid inside the terminal).
	HorizontalPad int
	// TermWidth is the terminal width this layout was computed for.
	TermWidth int
}

// NewGridLayout computes the grid geometry for the given terminal width.
//
// Column count scales with terminal width:
//
//	< 80   cols → 2 columns
//	< 120  cols → 3 columns
//	< 160  cols → 4 columns
//	≥ 160  cols → 5 columns (Netflix-style default)
//
// Card dimensions are derived from column count to maintain a consistent
// aspect ratio.
func NewGridLayout(termWidth int) GridLayout {
	cols := colsForWidth(termWidth)
	cardW := CardWidthForCols(termWidth, cols)
	cardH := cardHeightForWidth(cardW)
	used := cols * cardW
	pad := 0
	if termWidth > used {
		pad = (termWidth - used) / 2
	}
	return GridLayout{
		Cols:          cols,
		CardWidth:     cardW,
		CardHeight:    cardH,
		HorizontalPad: pad,
		TermWidth:     termWidth,
	}
}

// colsForWidth returns the number of grid columns for a terminal width.
func colsForWidth(w int) int {
	switch {
	case w < 80:
		return 2
	case w < 120:
		return 3
	case w < 160:
		return 4
	default:
		return 5
	}
}

// CardWidthForCols calculates the card width in cells given a column count.
// Exported so card.go can call it without duplicating the calculation.
func CardWidthForCols(termWidth, cols int) int {
	if cols <= 0 {
		return 20
	}
	w := termWidth / cols
	if w < 14 {
		return 14
	}
	return w
}

// cardHeightForWidth returns an appropriate card height for the card width.
// Maintains a roughly 1:1.6 (golden ratio) poster aspect ratio.
func cardHeightForWidth(cardWidth int) int {
	h := int(float64(cardWidth) * 1.6)
	if h < 8 {
		return 8
	}
	return h
}

// TotalRows returns how many complete rows a set of items fills.
func (g GridLayout) TotalRows(itemCount int) int {
	if g.Cols <= 0 {
		return 0
	}
	return (itemCount + g.Cols - 1) / g.Cols
}

// IndexAt returns the flat item index for a (row, col) position.
// Returns -1 if the position is out of bounds for itemCount items.
func (g GridLayout) IndexAt(row, col, itemCount int) int {
	idx := row*g.Cols + col
	if idx >= itemCount {
		return -1
	}
	return idx
}

// RowCol returns the (row, col) position for a flat item index.
func (g GridLayout) RowCol(idx int) (row, col int) {
	if g.Cols <= 0 {
		return 0, 0
	}
	return idx / g.Cols, idx % g.Cols
}

// VisibleRowRange returns the [startRow, endRow) range of rows visible in a
// viewport of height viewHeight cells, centred on cursorRow.
func (g GridLayout) VisibleRowRange(totalItems, viewHeight, cursorRow int) (start, end int) {
	totalRows := g.TotalRows(totalItems)
	maxVisible := viewHeight / g.CardHeight
	if maxVisible < 1 {
		maxVisible = 1
	}
	if totalRows <= maxVisible {
		return 0, totalRows
	}
	// Keep cursor in the middle of the viewport
	half := maxVisible / 2
	start = cursorRow - half
	if start < 0 {
		start = 0
	}
	end = start + maxVisible
	if end > totalRows {
		end = totalRows
		start = end - maxVisible
		if start < 0 {
			start = 0
		}
	}
	return start, end
}
