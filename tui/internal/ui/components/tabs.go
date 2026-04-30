package components

import (
	"strings"

	"charm.land/lipgloss/v2"
	"image/color"
)

// TabOption defines a single tab with dynamic label.
type TabOption struct {
	Label    string
	IsActive bool
}

// padRowToWidth right-pads each line of a multi-line string so every
// row is exactly `w` cells wide. Lipgloss-aware (counts visual width
// via lipgloss.Width). Used by RenderTabs to keep the tab bar a
// uniform rectangle — without this, only the bottom row carried the
// `─` underline that filled the width, while the top/label rows were
// just the narrow `tabsW` block, and JoinVertical above us would
// right-pad them with default-styled spaces. That ragged-shape leaked
// styling into adjacent rows and broke scrollbar alignment in the
// detail screen.
func padRowToWidth(s string, w int) string {
	out := strings.Builder{}
	out.Grow(len(s) + w)
	lines := strings.Split(s, "\n")
	for i, line := range lines {
		if i > 0 {
			out.WriteString("\n")
		}
		vis := lipgloss.Width(line)
		out.WriteString(line)
		if vis < w {
			out.WriteString(strings.Repeat(" ", w-vis))
		}
	}
	return out.String()
}

// RenderTabs renders tabs with a full-width underline automatically.
// The underline fills from end of tabs to screenWidth.
//
// Returns a uniform-width multi-line block: every row is exactly
// `screenWidth` cells wide. The bottom row carries the `─` underline
// connecting to the right edge; the top and label rows are
// right-padded with spaces. Callers that JoinVertical this with
// further content get a clean rectangle without ragged-row bleed.
func RenderTabs(options []TabOption, borderColor, accentColor color.Color, screenWidth int) string {
	activeTabBorder := lipgloss.Border{
		Top:         "─",
		Bottom:      " ", // Space connects to content below
		Left:        "│",
		Right:       "│",
		TopLeft:     "╭",
		TopRight:    "╮",
		BottomLeft:  "┘",
		BottomRight: "└",
	}
	inactiveTabBorder := lipgloss.Border{
		Top:         "─",
		Bottom:      "─", // Underline for inactive
		Left:        "│",
		Right:       "│",
		TopLeft:     "╭",
		TopRight:    "╮",
		BottomLeft:  "┴",
		BottomRight: "┴",
	}

	// Active tabs use accent color, inactive use border color
	activeTabStyle := lipgloss.NewStyle().
		Border(activeTabBorder, true).
		BorderForeground(accentColor).
		Padding(0, 1).
		Bold(true)

	inactiveTabStyle := lipgloss.NewStyle().
		Border(inactiveTabBorder, true).
		BorderForeground(borderColor).
		Padding(0, 1)

	// Build tab views
	var tabViews []string
	for _, opt := range options {
		if opt.IsActive {
			tabViews = append(tabViews, activeTabStyle.Render(opt.Label))
		} else {
			tabViews = append(tabViews, inactiveTabStyle.Render(opt.Label))
		}
	}

	tabsContent := lipgloss.JoinHorizontal(lipgloss.Top, tabViews...)

	// Calculate underline to fill remaining width.
	//
	// Stops 1 col short of the right edge to prevent the rendered
	// `─` chars from spilling into the next row (which produced
	// "ghost" fragments at column 0 of the body row below). The
	// padRowToWidth call below right-pads with a single space so
	// the whole bottom row still measures `screenWidth` cells.
	if screenWidth > 0 {
		underlineStyle := lipgloss.NewStyle().Foreground(borderColor)
		tabsW := lipgloss.Width(tabsContent)
		remain := max(0, screenWidth-tabsW-2)
		if remain > 0 {
			underline := underlineStyle.Render(strings.Repeat("─", remain))
			combined := tabsContent + underline
			return padRowToWidth(combined, screenWidth)
		}
	}

	return padRowToWidth(tabsContent, screenWidth)
}

// Tabs renders a simple tab list without underline (backward compatible).
func Tabs(activeIndex int, labels []string, borderColor color.Color) string {
	options := make([]TabOption, len(labels))
	for i, label := range labels {
		options[i] = TabOption{
			Label:    label,
			IsActive: i == activeIndex,
		}
	}
	return RenderTabs(options, borderColor, borderColor, 0)
}
