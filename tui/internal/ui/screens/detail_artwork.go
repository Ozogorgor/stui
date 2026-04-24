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
// backdrop index and a navigation hint. Loading/empty states fall back
// to a single dim label so the poster area never stays blank.
func renderBackdropCarousel(ds *DetailState, width int) string {
	_ = width // kept for future responsive sizing

	switch ds.Meta.ArtworkStatus {
	case FetchPending:
		// Stay quiet while we don't know if there'll be backdrops — no
		// point shouting "Loading artwork…" when the poster placeholder
		// is already filling the block.
		return ""
	case FetchEmpty:
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
