package components

import (
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

// Scrollbar renders a fixed-width vertical scrollbar (1 char wide,
// viewH lines tall) ready to drop into a horizontal layout next to
// scrollable content.
//
// Always renders the track even when all items fit, so the column
// stays visually reserved and layout doesn't shift between short
// and overflowing lists.
//
// Colors are read from theme.T (Accent for thumb, TextDim for track)
// — callers do not pass a style. Use:
//
//	bar := components.Scrollbar(scroll, viewH, total)
//	return lipgloss.JoinHorizontal(lipgloss.Top, content, " ", bar)
//
// to place the bar as a separate column adjacent to scrollable
// content.
func Scrollbar(scroll, viewH, totalItems int) string {
	if viewH <= 0 {
		return ""
	}
	// Use Background-only styles with a space character. Terminals
	// fill the entire cell (including inter-row leading from font
	// line-spacing) with the cell's bg colour, whereas a foreground
	// `█` glyph only paints the glyph's pixel extent — which leaves
	// hairline gaps between rows in many fonts and reads as ticks
	// instead of a continuous bar. This is the same idiom used by
	// terminal progress bars / scrollbars in tools like btop, htop,
	// and lazygit.
	thumb := lipgloss.NewStyle().Background(theme.T.Accent())
	track := lipgloss.NewStyle().Background(theme.T.TextDim())

	// All items visible (or empty list) — thumb fills the whole track.
	if totalItems <= viewH {
		out := strings.Builder{}
		out.Grow(viewH * 8)
		for i := 0; i < viewH; i++ {
			if i > 0 {
				out.WriteString("\n")
			}
			out.WriteString(thumb.Render(" "))
		}
		return out.String()
	}

	// Thumb size proportional to viewport/total ratio, min 1.
	thumbH := viewH * viewH / totalItems
	if thumbH < 1 {
		thumbH = 1
	}
	maxScroll := totalItems - viewH
	if scroll < 0 {
		scroll = 0
	}
	if scroll > maxScroll {
		scroll = maxScroll
	}
	thumbPos := 0
	if maxScroll > 0 {
		thumbPos = scroll * (viewH - thumbH) / maxScroll
	}

	out := strings.Builder{}
	out.Grow(viewH * 8)
	for i := 0; i < viewH; i++ {
		if i > 0 {
			out.WriteString("\n")
		}
		if i >= thumbPos && i < thumbPos+thumbH {
			out.WriteString(thumb.Render(" "))
		} else {
			out.WriteString(track.Render(" "))
		}
	}
	return out.String()
}
