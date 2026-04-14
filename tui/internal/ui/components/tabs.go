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

// RenderTabs renders tabs with a full-width underline automatically.
// The underline fills from end of tabs to screenWidth.
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

	// Calculate underline to fill remaining width
	if screenWidth > 0 {
		underlineStyle := lipgloss.NewStyle().Foreground(borderColor)
		tabsW := lipgloss.Width(tabsContent)
		remain := max(0, screenWidth-tabsW)
		if remain > 0 {
			underline := underlineStyle.Render(strings.Repeat("─", remain))
			return tabsContent + underline
		}
	}

	return tabsContent
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
