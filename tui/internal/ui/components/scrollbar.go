package components

import (
	"charm.land/lipgloss/v2"
)

// ScrollbarChars returns scrollbar characters for a list view.
// Returns a slice of viewH single-character strings forming a vertical
// scrollbar track. The track channel (░) is ALWAYS rendered — when all
// items fit, the thumb (█) fills the full height so the user sees a solid
// bar, keeping the column visually reserved and layout stable.
func ScrollbarChars(scroll, viewH, totalItems int, style lipgloss.Style) []string {
	if viewH <= 0 {
		return nil
	}
	chars := make([]string, viewH)

	if totalItems <= viewH {
		// All items visible — thumb fills the whole track.
		for i := range chars {
			chars[i] = style.Render("█")
		}
		return chars
	}

	// Thumb size proportional to viewport/total ratio, min 1.
	thumbH := viewH * viewH / totalItems
	if thumbH < 1 {
		thumbH = 1
	}
	maxScroll := totalItems - viewH
	thumbPos := 0
	if maxScroll > 0 {
		thumbPos = scroll * (viewH - thumbH) / maxScroll
	}
	for i := 0; i < viewH; i++ {
		if i >= thumbPos && i < thumbPos+thumbH {
			chars[i] = style.Render("█")
		} else {
			chars[i] = style.Render("░")
		}
	}
	return chars
}

// ScrollbarStyle returns a styled scrollbar component for a list.
// cursor - current cursor position
// viewHeight - number of visible rows
// totalItems - total number of items in the list
// dim - lipgloss style for the scrollbar (usually theme.T.TextDim())
// Always shows scrollbar track even if all items fit (like standard scrollbars)
func ScrollbarStyle(cursor, viewHeight, totalItems int, dim lipgloss.Style) string {
	if totalItems == 0 || viewHeight <= 0 {
		return ""
	}

	// Always show scrollbar - calculate thumb position even if no scrolling needed
	thumbH := viewHeight * viewHeight / totalItems
	if thumbH < 1 {
		thumbH = 1
	}
	maxScroll := totalItems - viewHeight
	scroll := cursor
	if scroll > maxScroll {
		scroll = maxScroll
	}
	if scroll < 0 {
		scroll = 0
	}
	thumbPos := 0
	if maxScroll > 0 {
		thumbPos = scroll * (viewHeight - thumbH) / maxScroll
	}

	// Build scrollbar string (always shows track)
	var bar string
	for i := 0; i < viewHeight; i++ {
		if i >= thumbPos && i < thumbPos+thumbH {
			bar += "█"
		} else {
			bar += "░"
		}
	}
	return dim.Render(bar)
}
