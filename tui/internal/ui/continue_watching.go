package ui

// continue_watching.go — "Continue Watching" horizontal row rendered above the
// main catalog grid. Shows up to cwMaxItems in-progress entries for the active
// tab, each as a poster card with a progress bar.

import (
	"fmt"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

// cwTabActive returns true when the given tab supports "Continue Watching"
// (i.e. it is a video-media tab backed by watch history).
// Music and Collections are excluded.
func cwTabActive(t state.Tab) bool {
	switch t {
	case state.TabMovies, state.TabSeries:
		return true
	default:
		return false
	}
}

const cwMaxItems = 5

// cwItems returns in-progress entries for the given tab ID, sorted by
// LastWatched descending, capped at cwMaxItems.
func cwItems(store watchhistory.StoreInterface, tabID string) []watchhistory.Entry {
	all := store.InProgress()
	var filtered []watchhistory.Entry
	for _, e := range all {
		if len(filtered) == cwMaxItems {
			break
		}
		if e.Tab == tabID {
			filtered = append(filtered, e)
		}
	}
	return filtered
}

// historyEntryToCatalogEntry converts a watchhistory.Entry to an ipc.CatalogEntry
// so it can be used with the existing detail-open flow.
func historyEntryToCatalogEntry(e watchhistory.Entry) ipc.CatalogEntry {
	var year *string
	if e.Year != "" {
		y := e.Year
		year = &y
	}
	var imdbID *string
	if e.ImdbID != "" {
		id := e.ImdbID
		imdbID = &id
	}
	tab := e.Tab
	return ipc.CatalogEntry{
		ID:       e.ID,
		Title:    e.Title,
		Year:     year,
		Provider: e.Provider,
		ImdbID:   imdbID,
		Tab:      tab,
	}
}

// cwTimeLeft returns a human-readable "Xh Ym left" string for remaining
// playback time. Returns "" when duration is 0 (unknown).
func cwTimeLeft(position, duration float64) string {
	if duration <= 0 {
		return ""
	}
	remaining := duration - position
	if remaining < 0 {
		remaining = 0
	}
	totalMinutes := int(remaining / 60)
	hours := totalMinutes / 60
	minutes := totalMinutes % 60
	if hours > 0 {
		return fmt.Sprintf("%dh %02dm left", hours, minutes)
	}
	return fmt.Sprintf("%dm left", minutes)
}

// cwSubtitle returns a short descriptor line for the card, e.g.
// "S3E5 · 1h 00m left", "Series · 1h 30m left", or "Movie · 1h 30m left".
func cwSubtitle(e watchhistory.Entry) string {
	timeStr := cwTimeLeft(e.Position, e.Duration)
	var typeLabel string
	switch e.Tab {
	case string(ipc.TabSeries):
		if e.Season > 0 && e.Episode > 0 {
			typeLabel = fmt.Sprintf("S%dE%d", e.Season, e.Episode)
		} else {
			typeLabel = "Series"
		}
	default:
		typeLabel = "Movie"
	}
	if timeStr == "" {
		return typeLabel
	}
	return typeLabel + " · " + timeStr
}

// cwProgressBar renders a fixed-width progress bar using block characters.
// Styling (accent color) is applied to the complete bar string.
func cwProgressBar(position, duration float64, w int) string {
	if w <= 0 {
		return ""
	}
	var ratio float64
	if duration > 0 {
		ratio = position / duration
		if ratio > 1 {
			ratio = 1
		}
		if ratio < 0 {
			ratio = 0
		}
	}
	filled := int(ratio * float64(w))
	empty := w - filled

	bar := strings.Repeat("█", filled) + strings.Repeat("░", empty)
	return lipgloss.NewStyle().Foreground(theme.T.Accent()).Render(bar)
}

// renderContinueWatchingCard renders one entry as a card: placeholder poster,
// title, subtitle line, and progress bar.
func renderContinueWatchingCard(e watchhistory.Entry, w int, selected bool) string {
	posterH := components.CardPosterRows

	// Poster
	poster := components.RenderPosterPlaceholder(e.Title, "", w, posterH)

	// Title
	title := components.Truncate(e.Title, w-2)
	titleStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Bold(true).
		Width(w)
	titleLine := titleStyle.Render(title)

	// Subtitle
	sub := cwSubtitle(e)
	subLine := lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		Width(w).
		Render(components.Truncate(sub, w-2))

	// Progress bar
	barW := w - 2
	if barW < 1 {
		barW = 1
	}
	bar := cwProgressBar(e.Position, e.Duration, barW)
	barLine := lipgloss.NewStyle().Width(w).Render(bar)

	content := lipgloss.JoinVertical(lipgloss.Left,
		poster,
		titleLine,
		subLine,
		barLine,
	)

	var cardStyle lipgloss.Style
	if selected {
		cardStyle = lipgloss.NewStyle().
			BorderStyle(lipgloss.RoundedBorder()).
			BorderForeground(theme.T.Accent()).
			Padding(0, 1).
			Width(w)
	} else {
		cardStyle = lipgloss.NewStyle().
			BorderStyle(lipgloss.RoundedBorder()).
			BorderForeground(theme.T.Border()).
			Padding(0, 1).
			Width(w)
	}

	return cardStyle.Render(content)
}

// cwCurrentItems returns CW items for the active tab, or nil if not applicable.
func (m Model) cwCurrentItems() []watchhistory.Entry {
	if m.historyStore == nil || !cwTabActive(m.state.ActiveTab) {
		return nil
	}
	return cwItems(m.historyStore, m.state.ActiveTab.MediaTabID())
}

// renderContinueWatchingRow renders the full "Continue Watching" section:
// a header label followed by a horizontal row of cards.
func renderContinueWatchingRow(entries []watchhistory.Entry, cursor int, focused bool, termWidth int) string {
	if len(entries) == 0 {
		return ""
	}

	// Header
	headerStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Bold(true).
		PaddingLeft(2)
	header := headerStyle.Render("Continue Watching")

	// Cards
	cardW := components.CardWidth(termWidth)
	var cards []string
	for i, e := range entries {
		selected := focused && i == cursor
		cards = append(cards, renderContinueWatchingCard(e, cardW, selected))
	}
	row := lipgloss.JoinHorizontal(lipgloss.Top, cards...)
	rowLine := lipgloss.NewStyle().PaddingLeft(2).Render(row)

	return lipgloss.JoinVertical(lipgloss.Left, header, rowLine)
}
