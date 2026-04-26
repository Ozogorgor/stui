// Package mediaheader renders the shared "poster + title + meta" chrome
// used by every screen that previews a single media entry (movie, series,
// episode browser, etc.).
//
// Only the poster block is centralised here; surrounding layout (info
// column, description, cast/crew, etc.) stays with each screen since it
// varies per use-case. Centralising the poster keeps chafa caching, URL
// download enqueueing, and placeholder fallback logic in one place — so a
// change to placeholder styling, fade-in behaviour, or chafa parameters
// updates every screen at once.
package mediaheader

import (
	"github.com/stui/stui/internal/ui/components"
	posterpkg "github.com/stui/stui/internal/ui/components/poster"
)

// Default poster-block dimensions. Callers may override per-screen, but
// these match the dimensions detail.go used historically — keep them as
// the "canonical" values so screens default to the same look.
const (
	PosterWidth  = 22 // chars
	PosterHeight = 14 // rows
)

// Inputs is the minimum data RenderPoster needs. Each field maps directly
// to one of the precedence steps below — a missing field falls through to
// the next.
type Inputs struct {
	// Title and Genre feed the placeholder when no poster is available.
	Title string
	Genre string

	// PosterArt is a pre-rendered block-art string. When non-empty it is
	// rendered verbatim — bypasses chafa entirely. Reserved for future
	// cache warm-up paths that pre-render at install time.
	PosterArt string

	// PosterURL is the source URL. When the poster cache holds a copy on
	// disk it gets rendered through ImageView (chafa / kitty graphics).
	// When the cache misses, the URL is enqueued for background download
	// and a placeholder is rendered in its place; a later re-render picks
	// up the freshly-cached file.
	PosterURL string
}

// imageViews caches one ImageView per cached file path so chafa's
// internal output cache is reused across re-renders. The cache is keyed
// by the on-disk path (which is hashed from the URL by the poster
// package) so different URLs never share a view.
var imageViews = map[string]*components.ImageView{}

func cardImageView(path string, w, h int) *components.ImageView {
	iv, ok := imageViews[path]
	if !ok {
		iv = components.NewImageView(w, h)
		iv.SetImage(path)
		imageViews[path] = iv
	}
	iv.SetSize(w, h)
	return iv
}

// RenderPoster returns the poster string sized to fit `w x h`. No outer
// padding or border — wrap with lipgloss in the caller if needed.
//
// Precedence (mirrors components/card.go:102-114):
//
//  1. PosterArt — pre-rendered block art.
//  2. PosterURL with on-disk cache hit → chafa via ImageView.
//  3. PosterURL with cache miss → enqueue download, render placeholder.
//  4. No poster data → placeholder.
func RenderPoster(in Inputs, w, h int) string {
	switch {
	case in.PosterArt != "":
		return in.PosterArt
	case in.PosterURL != "":
		if cached, hit := posterpkg.CachedPath(in.PosterURL); hit {
			return cardImageView(cached, w, h).View()
		}
		posterpkg.Global().Enqueue(in.PosterURL)
		return components.RenderPosterPlaceholder(in.Title, in.Genre, w, h)
	default:
		return components.RenderPosterPlaceholder(in.Title, in.Genre, w, h)
	}
}

// RenderBackdrop renders a single backdrop at `w x h`. Unlike
// RenderPoster, this returns an empty string on cache miss / no URL —
// backdrops are decorative, so an absent backdrop should leave the slot
// empty rather than fill it with a placeholder. The cache miss still
// enqueues the URL for download so a subsequent re-render picks it up.
func RenderBackdrop(url string, w, h int) string {
	if url == "" {
		return ""
	}
	if cached, hit := posterpkg.CachedPath(url); hit {
		return cardImageView(cached, w, h).View()
	}
	posterpkg.Global().Enqueue(url)
	return ""
}
