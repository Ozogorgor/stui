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
	"image/color"
	"strings"
	"sync"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	posterpkg "github.com/stui/stui/internal/ui/components/poster"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/theme"
)

const (
	CardColumns    = 5 // Netflix-style: 5 columns
	CardPosterRows = 9 // Height of the poster area in terminal rows
	CardMinWidth   = 14
	// Content rows inside the card border: poster + title + meta + genre.
	// The outer style adds 2 rows of border on top. Grid.go uses this
	// constant to compute rowH — keep them in lock-step.
	CardContentRows = CardPosterRows + 3
	// Total rendered card height (content + top/bottom border).
	CardTotalRows = CardContentRows + 2
)

// Package-level registry of ImageView instances keyed by the cached poster
// path. Reusing instances lets ImageView's internal chafa cache do its job
// across frames — without this, every re-render re-shells chafa for every
// visible card, which bottlenecks scrolling.
var (
	cardImageViewsMu sync.Mutex
	cardImageViews   = map[string]*ImageView{}
)

// cardImageView returns the (possibly cached) ImageView for a given cached
// poster path + dimensions. Width/height updates on the existing instance
// are a no-op if unchanged and only invalidate the chafa cache when they
// differ — both handled by ImageView.SetSize.
func cardImageView(path string, w, h int) *ImageView {
	cardImageViewsMu.Lock()
	defer cardImageViewsMu.Unlock()
	iv, ok := cardImageViews[path]
	if !ok {
		iv = NewImageView(w, h)
		iv.SetImage(path)
		cardImageViews[path] = iv
		return iv
	}
	iv.SetSize(w, h)
	return iv
}

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
// Title + compact meta are painted onto the last two rows of the poster as
// an opaque dark bar (Netflix-style overlay), so the card footprint is
// pure poster + border — no separate meta rows below.
// selected = true draws the accent-colored border.
func RenderCard(entry ipc.CatalogEntry, w int, selected bool) string {
	// Content area width after cardStyle's border(+2) and padding(+2). Poster
	// is rendered at this width so nothing wraps when the border wraps it.
	innerW := w - 4
	if innerW < 1 {
		innerW = 1
	}
	// Poster now fills the full card interior (was CardPosterRows = 9 with
	// meta rows stacked below). The bottom two lines are overwritten by the
	// meta bar, so the visible poster art is effectively innerH-2 rows tall.
	posterH := CardContentRows

	// ── Poster area ───────────────────────────────────────────────────────
	//
	// Precedence:
	//  1. PosterArt — runtime-side pre-rendered block art; already read today,
	//     future caching work will populate it more eagerly.
	//  2. PosterURL + on-disk cache hit — render through ImageView (chafa).
	//  3. PosterURL + cache miss — enqueue for background download, show
	//     existing placeholder so the user sees SOMETHING immediately.
	//  4. Neither — existing placeholder.
	var poster string
	switch {
	case entry.PosterArt != nil && *entry.PosterArt != "":
		poster = *entry.PosterArt
	case entry.PosterURL != nil && *entry.PosterURL != "":
		if cached, hit := posterpkg.CachedPath(*entry.PosterURL); hit {
			poster = cardImageView(cached, innerW, posterH).View()
		} else {
			posterpkg.Global().Enqueue(*entry.PosterURL)
			poster = renderPlaceholderPoster(entry, innerW, posterH)
		}
	default:
		poster = renderPlaceholderPoster(entry, innerW, posterH)
	}

	// Splice the meta bar onto the last two rows of the rendered poster.
	posterLines := strings.Split(poster, "\n")
	posterLines = overlayMetaBar(posterLines, entry.Title, buildCompactMeta(entry), innerW)
	content := strings.Join(posterLines, "\n")
	// Defense-in-depth: if chafa returned fewer lines than requested, pad
	// to CardContentRows so the card frame doesn't collapse.
	content = clampLines(content, CardContentRows)

	borderColor := theme.T.Border()
	if selected {
		borderColor = theme.T.Accent()
	}
	// Width/Height are TOTAL frame dimensions in lipgloss v2 — border+padding
	// are included in these counts. Pinning both locks every card to the
	// same footprint regardless of poster content.
	cardStyle := lipgloss.NewStyle().
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(borderColor).
		Padding(0, 1).
		Width(w).
		Height(CardTotalRows)

	return cardStyle.Render(content)
}

// buildCompactMeta joins year · ★rating · genre on one line. Parts that
// aren't present are skipped, so cards with partial metadata stay tidy.
func buildCompactMeta(entry ipc.CatalogEntry) string {
	var parts []string
	if entry.Year != nil && *entry.Year != "" {
		parts = append(parts, *entry.Year)
	}
	if entry.Rating != nil && *entry.Rating != "" {
		parts = append(parts, "★"+*entry.Rating)
	}
	if entry.Genre != nil && *entry.Genre != "" {
		parts = append(parts, *entry.Genre)
	}
	return strings.Join(parts, " · ")
}

// overlayMetaBar paints a 2-row dark bar onto the last two elements of
// posterLines: row 1 = bold white title, row 2 = muted white compact meta.
// Both rows are styled to exactly innerW cells so they align with the
// poster block above them. Safe when posterLines has fewer than 2 rows
// (no-op). The caller is responsible for the overall clamp to CardContentRows.
func overlayMetaBar(posterLines []string, title, meta string, innerW int) []string {
	n := len(posterLines)
	if n < 2 {
		return posterLines
	}
	barBg := theme.T.Bg()
	titleRow := bidi.AlignedStyle(
		lipgloss.NewStyle().
			Foreground(lipgloss.Color("#ffffff")).
			Background(barBg).
			Bold(true).
			Width(innerW),
		title,
	).Render(bidi.Apply(Truncate(title, innerW)))
	metaRow := lipgloss.NewStyle().
		Foreground(lipgloss.Color("#cccccc")).
		Background(barBg).
		Width(innerW).
		Render(Truncate(meta, innerW))
	posterLines[n-2] = singleLine(titleRow)
	posterLines[n-1] = singleLine(metaRow)
	return posterLines
}

// singleLine collapses a multi-line string to its first line, preserving any
// ANSI style codes that precede the first "\n".
func singleLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return s[:i]
	}
	return s
}

// clampLines forces a string to exactly `n` lines: truncates excess,
// pads with empty lines when short.
func clampLines(s string, n int) string {
	lines := strings.Split(s, "\n")
	if len(lines) > n {
		lines = lines[:n]
	} else {
		for len(lines) < n {
			lines = append(lines, "")
		}
	}
	return strings.Join(lines, "\n")
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
