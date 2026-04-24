package screens

// detail_artwork.go — backdrop carousel strip rendered below the poster.
//
// Kitty-graphics/sixel image rendering isn't wired in yet; this strip is
// intentionally text-only — it surfaces the currently selected index and
// a nav hint so the user knows the ←/→ cycle binding is available.

import (
	"fmt"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

// renderBackdropCarousel returns a narrow strip listing the current
// backdrop index and a navigation hint. This lives inside the poster
// column and only renders once the "artwork" verb resolves with data.
// Loading/empty labels are emitted by renderBackdropStatusStrip at
// full width instead, so the long strings never get word-wrapped by
// the narrow poster column.
func renderBackdropCarousel(ds *DetailState, width int) string {
	_ = width // kept for future responsive sizing

	if ds.Meta.ArtworkStatus != FetchLoaded {
		return ""
	}

	bd := ds.Meta.Artwork.Backdrops
	if len(bd) == 0 {
		return ""
	}

	idx := ds.Meta.ArtworkCursor
	if idx < 0 || idx >= len(bd) {
		idx = 0
	}

	indicator := lipgloss.NewStyle().
		Foreground(theme.T.AccentAlt()).
		Render(fmt.Sprintf("[%d/%d]", idx+1, len(bd)))
	hint := lipgloss.NewStyle().
		Foreground(theme.T.TextDim()).
		Faint(true).
		Render("←/→ cycle")

	return lipgloss.JoinHorizontal(lipgloss.Left, indicator, "  ", hint)
}

// renderBackdropStatusStrip returns a full-width, single-line faint
// label describing the artwork-verb fetch state. Rendered as a row
// between the main column split and the related row so the label has
// enough horizontal room to stay on one line (the poster column is
// only ~18 chars wide, which would word-wrap "No artwork available").
// Returns "" when the verb has loaded successfully — data-loaded state
// is surfaced by renderBackdropCarousel inside the poster column.
func renderBackdropStatusStrip(ds *DetailState, width int) string {
	switch ds.Meta.ArtworkStatus {
	case FetchPending:
		return lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			Faint(true).
			Width(width).
			PaddingLeft(2).
			Render(detailLoadingArtwork)
	case FetchEmpty:
		return lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			Faint(true).
			Width(width).
			PaddingLeft(2).
			Render(detailEmptyArtwork)
	}
	return ""
}
