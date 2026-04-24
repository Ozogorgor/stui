package screens

// detail_related.go — RELATED titles row.
//
// Reads ds.Meta.Related which is populated by the "related" verb of a
// GetDetailMetadata fan-out. The rendering is intentionally minimal
// (mini cards, colored background, initials) — Chunk 7 will rebuild it
// with artwork, year chips and proper keyboard navigation.

import (
	"image/color"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// renderRelatedRow is the bottom-of-detail carousel. It owns the outer
// frame and the loading/empty fallbacks; the card rendering is delegated
// to small helpers below.
func renderRelatedRow(ds *DetailState, w, h int) string {
	header := theme.T.SimilarHeaderStyle().Width(w - 2).Render(detailRelatedHeader)

	items := ds.Meta.Related.Items

	if ds.Meta.RelatedStatus == FetchPending {
		loading := lipgloss.NewStyle().
			Foreground(theme.T.Neon()).
			PaddingLeft(2).
			Render(detailLoadingRelated)
		return lipgloss.NewStyle().
			Background(theme.T.Surface()).
			Width(w).Height(h).
			Render(lipgloss.JoinVertical(lipgloss.Left, header, loading))
	}

	if len(items) == 0 {
		empty := lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			PaddingLeft(2).
			Render(detailEmptyRelated)
		return lipgloss.NewStyle().
			Background(theme.T.Surface()).
			Width(w).Height(h).
			Render(lipgloss.JoinVertical(lipgloss.Left, header, empty))
	}

	// Render up to similarCardCols mini cards
	cardH := h - lipgloss.Height(header) - 1
	miniW := (w - (similarCardCols+1)*2) / similarCardCols
	if miniW < 10 {
		miniW = 10
	}

	var cards []string
	start := ds.Meta.RelatedCursor
	if start >= len(items) {
		start = 0
	}
	end := min(start+similarCardCols, len(items))

	for i := start; i < end; i++ {
		e := items[i]
		selected := (ds.Focus == FocusDetailRelated && i == ds.Meta.RelatedCursor)

		// Minimal card: just colored block + title
		bg := similarCardBg(e.Title)
		inits := components.PosterInitials(e.Title)

		posterBlock := lipgloss.NewStyle().
			Background(bg).
			Width(miniW).
			Height(cardH-3).
			Align(lipgloss.Center, lipgloss.Center).
			Render(inits)

		titleStr := components.Truncate(e.Title, miniW)
		titleBlock := lipgloss.NewStyle().
			Foreground(theme.T.Text()).
			Width(miniW).
			Render(titleStr)

		var border lipgloss.Style
		if selected {
			border = lipgloss.NewStyle().
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(theme.T.Accent()).
				Width(miniW)
		} else {
			border = lipgloss.NewStyle().
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(theme.T.Border()).
				Width(miniW)
		}

		card := border.Render(lipgloss.JoinVertical(lipgloss.Left, posterBlock, titleBlock))
		cards = append(cards, card)
	}

	// Scroll arrow if more available
	if end < len(items) {
		arrow := lipgloss.NewStyle().
			Foreground(theme.T.AccentAlt()).
			Align(lipgloss.Center, lipgloss.Center).
			Height(cardH).
			Render("›")
		cards = append(cards, arrow)
	}

	row := lipgloss.JoinHorizontal(lipgloss.Top, cards...)
	content := lipgloss.JoinVertical(lipgloss.Left, header, row)

	return lipgloss.NewStyle().
		Background(theme.T.Surface()).
		Width(w).Height(h).
		PaddingLeft(1).
		Render(content)
}

// similarCardBg is a palette-hashed background for a card, kept stable
// across renders so the user has a visual anchor while scrolling.
func similarCardBg(title string) color.Color {
	colors := []color.Color{
		lipgloss.Color("#0d0d25"),
		lipgloss.Color("#0a1a0a"),
		lipgloss.Color("#1a0a0a"),
		lipgloss.Color("#0a0a1a"),
		lipgloss.Color("#1a1a00"),
		lipgloss.Color("#001a1a"),
	}
	return colors[components.TitleHash(title)%len(colors)]
}
