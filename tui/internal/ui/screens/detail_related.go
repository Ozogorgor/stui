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
	posterpkg "github.com/stui/stui/internal/ui/components/poster"
	"github.com/stui/stui/pkg/theme"
)

// Per-URL chafa ImageView cache for related-row posters. Keyed on the
// resolved on-disk cache path so the chafa rasteriser doesn't re-shell
// every frame, and so each item's poster is independent of every other.
// Mirrors the detailPosterImageViews map detail.go keeps for the main
// poster column — same lifetime semantics (process-wide), same eviction
// model (none; relies on stui's session length being bounded).
var relatedPosterImageViews = map[string]*components.ImageView{}

func relatedCardImageView(path string, w, h int) *components.ImageView {
	iv, ok := relatedPosterImageViews[path]
	if !ok {
		iv = components.NewImageView(w, h)
		iv.SetImage(path)
		relatedPosterImageViews[path] = iv
	}
	iv.SetSize(w, h)
	return iv
}

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
		// Plugins returned no related items — hide the whole row so
		// we don't waste vertical space on an empty placeholder.
		return ""
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

		posterH := cardH - 3
		// Render chafa poster when the URL has been cached on disk;
		// otherwise enqueue the URL for background download and show
		// the initials-on-color fallback so the card never goes blank.
		var posterBlock string
		var posterURL string
		if e.PosterURL != nil {
			posterURL = *e.PosterURL
		}
		if posterURL != "" {
			if cached, hit := posterpkg.CachedPath(posterURL); hit {
				posterBlock = relatedCardImageView(cached, miniW, posterH).View()
			} else {
				posterpkg.Global().Enqueue(posterURL)
			}
		}
		if posterBlock == "" {
			bg := similarCardBg(e.Title)
			inits := components.PosterInitials(e.Title)
			posterBlock = lipgloss.NewStyle().
				Background(bg).
				Width(miniW).
				Height(posterH).
				Align(lipgloss.Center, lipgloss.Center).
				Render(inits)
		}

		// Hard-clamp the title to a single row regardless of the rune
		// count Truncate produced. Wide-character titles (CJK, full-width
		// punctuation) and stray newlines used to expand the card vertically
		// because lipgloss wraps when the rendered width exceeds Width().
		// Height(1) + MaxHeight(1) pins the card height to posterH + 1.
		titleStr := components.Truncate(e.Title, miniW)
		titleBlock := lipgloss.NewStyle().
			Foreground(theme.T.Text()).
			Width(miniW).
			Height(1).
			MaxHeight(1).
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
