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
	"strconv"
	"strings"
	"sync"
	"unicode"

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
	// Render chafa at the VISIBLE poster height (innerH minus the 2 rows
	// the meta bar will occupy). Rendering at the full interior height
	// caused chafa to fit-to-height using all 12 rows, then the meta bar
	// cropped the bottom of the image — visually squishing portrait
	// posters (movies/series, 2:3) since their bottom 2 rows of actual
	// art were hidden. Square album posters happened to render with
	// empty padding in those rows, so they weren't affected.
	//
	// We pad the rendered output back to CardContentRows below so the
	// meta bar still sits at the card's bottom edge.
	posterH := CardContentRows - 2

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

	// Center the rendered poster horizontally within the card's inner
	// width. Chafa left-aligns its output, so portrait posters (which
	// don't fill the full innerW) had visible empty space on the right.
	// Width+Align(Center) pads each line equally on both sides.
	poster = lipgloss.NewStyle().
		Width(innerW).
		Align(lipgloss.Center).
		Render(poster)

	// Pad the rendered poster up to the full inner card height so the
	// meta-bar overlay below lands on the bottom 2 rows of the card,
	// not the bottom 2 rows of the image. Empty rows go between the
	// image and the meta bar — so portrait images (rendered to fit
	// within posterH-2) keep their full visible height, square images
	// stay nested at the top, and the meta bar always sits flush
	// against the card's bottom border.
	posterLines := strings.Split(poster, "\n")
	for len(posterLines) < CardContentRows {
		posterLines = append(posterLines, "")
	}
	posterLines = overlayRatingBadge(posterLines, entry, innerW)
	posterLines = overlayMetaBar(posterLines, buildTitleLine(entry), buildCompactMeta(entry), innerW)
	content := strings.Join(posterLines, "\n")
	// Defense-in-depth: if chafa returned MORE lines than expected (rare
	// but possible with some symbol maps), trim/pad to CardContentRows.
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

// buildTitleLine returns the album/movie title with the release year
// appended in parentheses if present. Year used to live in the meta
// row alongside artist + genre; pulling it up to the title row gives
// the artist+genre pair more breathing room and reads more naturally
// ("OK Computer (1997)" mirrors how album titles are written
// elsewhere). When year is absent the title stands alone.
func buildTitleLine(entry ipc.CatalogEntry) string {
	if entry.Year != nil && *entry.Year != "" {
		return entry.Title + " (" + *entry.Year + ")"
	}
	return entry.Title
}

// buildCompactMeta joins artist · genre on one line. Year used to
// be here too but moved up to the title row (see buildTitleLine).
// Genre is taken as the first comma-separated token only — Discogs
// sometimes returns "Rock, Indie Rock, Alternative" and the long
// form crowds out the artist on narrow cards.
//
// Rating is NOT in the meta line — it's overlaid as a top-right
// badge on the poster (see overlayRatingBadge).
func buildCompactMeta(entry ipc.CatalogEntry) string {
	var parts []string
	if entry.Artist != nil && *entry.Artist != "" {
		parts = append(parts, *entry.Artist)
	}
	if entry.Genre != nil && *entry.Genre != "" {
		g := *entry.Genre
		if i := strings.Index(g, ","); i >= 0 {
			g = g[:i]
		}
		g = titleCaseGenre(strings.TrimSpace(g))
		if g != "" {
			parts = append(parts, g)
		}
	}
	return strings.Join(parts, " · ")
}

// titleCaseGenre uppercases the first letter of each word/segment.
// Word boundaries are whitespace OR hyphens — so "indie rock" →
// "Indie Rock" and "hip-hop" → "Hip-Hop". Everything else is
// lowercased so source variations like "ROCK", "Rock", and "rock"
// all collapse to "Rock".
func titleCaseGenre(s string) string {
	var b strings.Builder
	b.Grow(len(s))
	capitalize := true
	for _, r := range s {
		if unicode.IsSpace(r) || r == '-' {
			b.WriteRune(r)
			capitalize = true
			continue
		}
		if capitalize {
			b.WriteRune(unicode.ToUpper(r))
			capitalize = false
		} else {
			b.WriteRune(unicode.ToLower(r))
		}
	}
	return b.String()
}

// overlayRatingBadge replaces the rightmost cells of posterLines[0]
// with a "★X.X" rating badge (one decimal, accent-colored, bold).
// No-op when posterLines is empty or the entry has no rating.
//
// Ratings arrive as raw numeric strings (often more than one
// decimal — "8.456"). We round to one decimal here for display;
// the underlying weighted-rating logic stays untouched.
func overlayRatingBadge(posterLines []string, entry ipc.CatalogEntry, innerW int) []string {
	if len(posterLines) == 0 || entry.Rating == nil || *entry.Rating == "" {
		return posterLines
	}
	rating := *entry.Rating
	if r, err := strconv.ParseFloat(rating, 64); err == nil {
		rating = fmt.Sprintf("%.1f", r)
	}
	badge := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Background(theme.T.Bg()).
		Bold(true).
		Render("★" + rating)
	badgeW := lipgloss.Width(badge)
	if badgeW >= innerW {
		// Pathological narrow card — bail rather than blow out the layout.
		return posterLines
	}
	// Truncate the first poster row to (innerW - badgeW) cells so the
	// badge fits on the right. lipgloss's MaxWidth is ANSI-aware.
	leftStyle := lipgloss.NewStyle().Width(innerW - badgeW).MaxWidth(innerW - badgeW)
	posterLines[0] = leftStyle.Render(posterLines[0]) + badge
	return posterLines
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
