package components

// card.go — renders a single poster card in the Netflix-style grid.
//
// Each card is a fixed-width block with:
//   - A poster area (block-art image OR styled placeholder)
//   - Title truncated to card width
//   - Year + rating on one line
//   - Genre tag
//
// Card dimensions are computed dynamically from terminal width / column count.

import (
	"fmt"
	"image/color"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/theme"
)

const (
	CardColumns    = 5 // Netflix-style: 5 columns
	CardPosterRows = 9 // Height of the poster area in terminal rows
	CardMinWidth   = 14
)

// CardWidth calculates the width of each card given terminal width.
func CardWidth(termWidth int) int {
	padding := (CardColumns + 1) * 2 // 2 chars padding between/around cards
	w := (termWidth - padding) / CardColumns
	if w < CardMinWidth {
		return CardMinWidth
	}
	return w
}

// RenderCard renders a single CatalogEntry as a poster card string.
// selected = true draws a violet glow border.
func RenderCard(entry ipc.CatalogEntry, w int, selected bool) string {
	posterH := CardPosterRows

	// ── Poster area ───────────────────────────────────────────────────────
	var poster string
	if entry.PosterArt != nil && *entry.PosterArt != "" {
		// Pre-rendered block art from cache — use directly
		poster = *entry.PosterArt
	} else {
		poster = renderPlaceholderPoster(entry, w, posterH)
	}

	// ── Title ─────────────────────────────────────────────────────────────
	title := Truncate(entry.Title, w-2)
	titleStyle := bidi.AlignedStyle(
		lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true).Width(w),
		entry.Title,
	)
	titleLine := titleStyle.Render(bidi.Apply(title))

	// ── Year + Rating ────────────────────────────────────────────────────
	year := "—"
	if entry.Year != nil && *entry.Year != "" {
		year = *entry.Year
	}
	rating := ""
	if entry.Rating != nil && *entry.Rating != "" {
		barWidth := 6
		if w > 18 {
			barWidth = 8
		}
		rating = CompactRatingBar(*entry.Rating, barWidth)
	}
	metaLine := lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		Width(w).
		Render(fmt.Sprintf("%s  %s", year, rating))

	// ── Genre tag ─────────────────────────────────────────────────────────
	genreTag := ""
	if entry.Genre != nil && *entry.Genre != "" {
		g := Truncate(*entry.Genre, w-4)
		genreTag = lipgloss.NewStyle().
			Foreground(theme.T.AccentAlt()).
			Render("◆ " + g)
	}

	// ── Card border ───────────────────────────────────────────────────────
	content := lipgloss.JoinVertical(lipgloss.Left,
		poster,
		titleLine,
		metaLine,
		genreTag,
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

// posterColorPalette is the shared color palette used for placeholder posters.
// The same set is used for both grid cards and the detail view.
var posterColorPalette = []color.Color{
	lipgloss.Color("#1a0533"), // deep violet
	lipgloss.Color("#001a2e"), // deep navy
	lipgloss.Color("#0d1f0d"), // dark green
	lipgloss.Color("#2a1a00"), // dark amber
	lipgloss.Color("#1a001a"), // deep magenta
	lipgloss.Color("#001a1a"), // deep teal
	lipgloss.Color("#2a0a0a"), // deep red
	lipgloss.Color("#0a0a2a"), // deep blue
}

// RenderPosterPlaceholder generates a styled text block for when no block-art
// is available. genre may be empty; if non-empty a small genre label is shown
// below the initials (used in the detail view).
func RenderPosterPlaceholder(title, genre string, w, h int) string {
	bgColor := posterColorPalette[TitleHash(title)%len(posterColorPalette)]
	initials := PosterInitials(title)

	innerH := h - 2
	topPad := innerH / 2
	bottomPad := innerH - topPad - 1
	if genre != "" {
		// One extra line for the genre label — shift center up
		topPad--
		if topPad < 0 {
			topPad = 0
		}
		bottomPad = innerH - topPad - 2
		if bottomPad < 0 {
			bottomPad = 0
		}
	}

	blank := strings.Repeat(" ", w)
	var lines []string
	for range topPad {
		lines = append(lines, blank)
	}
	lines = append(lines,
		lipgloss.NewStyle().
			Foreground(lipgloss.Color("#ffffff")).
			Background(bgColor).
			Bold(true).
			Width(w).
			Align(lipgloss.Center).
			Render(initials),
	)
	if genre != "" {
		lines = append(lines,
			lipgloss.NewStyle().
				Foreground(lipgloss.Color("#888888")).
				Background(bgColor).
				Width(w).
				Align(lipgloss.Center).
				Render(Truncate(genre, w-2)),
		)
	}
	for range bottomPad {
		lines = append(lines, blank)
	}

	return lipgloss.NewStyle().
		Background(bgColor).
		Width(w).
		Height(h).
		Render(strings.Join(lines, "\n"))
}

// renderPlaceholderPoster renders a placeholder poster for a catalog entry.
func renderPlaceholderPoster(entry ipc.CatalogEntry, w, h int) string {
	genre := ""
	if entry.Genre != nil {
		genre = *entry.Genre
	}
	return RenderPosterPlaceholder(entry.Title, genre, w, h)
}

// PosterInitials returns 1-2 letter initials for display in placeholder posters.
func PosterInitials(title string) string {
	words := strings.Fields(title)
	// Skip common articles
	skip := map[string]bool{"the": true, "a": true, "an": true, "of": true}
	var letters []string
	for _, w := range words {
		if len(letters) >= 2 {
			break
		}
		lower := strings.ToLower(w)
		if skip[lower] {
			continue
		}
		if len(w) > 0 {
			letters = append(letters, strings.ToUpper(string([]rune(w)[0])))
		}
	}
	if len(letters) == 0 && len(title) > 0 {
		return strings.ToUpper(string([]rune(title)[0]))
	}
	return strings.Join(letters, "")
}

// TitleHash is a simple djb2-style hash for consistent color assignment.
func TitleHash(s string) int {
	h := 5381
	for _, c := range s {
		h = h*33 + int(c)
	}
	if h < 0 {
		h = -h
	}
	return h
}
