// imageview.go — Reusable terminal image rendering component.
//
// Renders images via charmbracelet/x/mosaic (pure-Go half-block ANSI).
// Kitty graphics protocol code is kept as a future hook but unused
// today because Bubbletea's diff-based alt-screen redraws don't survive
// the inline image escapes.
//
// Usage:
//
//	iv := components.NewImageView(20, 10) // width x height in cells
//	iv.SetImage("/path/to/cover.jpg")
//	rendered := iv.View() // returns string for embedding in a View
//
// The component caches rendered output and only re-decodes when
// the image path or dimensions change. Safe to call View() every frame.

package components

import (
	"strings"
	"sync"
)

// ImageProtocol is the terminal image rendering protocol to use.
type ImageProtocol int

const (
	ImageProtocolSymbols ImageProtocol = iota // Unicode half-blocks (any terminal)
	ImageProtocolKitty                        // Kitty graphics protocol
)

// DetectImageProtocol checks the terminal and returns the best protocol.
// Currently always returns Symbols because Kitty graphics protocol
// doesn't survive Bubbletea's diff-based alt-screen redraws.
// The Kitty code path is kept for future use when a compatible
// rendering approach is found.
func DetectImageProtocol() ImageProtocol {
	return ImageProtocolSymbols
}

// ImageView renders an image file into terminal-compatible output.
type ImageView struct {
	mu       sync.Mutex
	width    int
	height   int
	path     string // current image file path
	protocol ImageProtocol
	// Cache
	cachedPath       string
	cachedW, cachedH int
	cachedLines      []string // pre-split lines for symbols; single element for kitty
	placeholder      string
}

// NewImageView creates an ImageView with the given cell dimensions.
func NewImageView(width, height int) *ImageView {
	return &ImageView{
		width:    width,
		height:   height,
		protocol: DetectImageProtocol(),
	}
}

// SetSize updates the display dimensions. Invalidates cache if changed.
func (iv *ImageView) SetSize(w, h int) {
	iv.mu.Lock()
	defer iv.mu.Unlock()
	if iv.width != w || iv.height != h {
		iv.width = w
		iv.height = h
		iv.cachedPath = "" // invalidate
	}
}

// SetImage sets the image file to render. Invalidates cache if changed.
func (iv *ImageView) SetImage(path string) {
	iv.mu.Lock()
	defer iv.mu.Unlock()
	if iv.path != path {
		iv.path = path
		iv.cachedPath = "" // invalidate
	}
}

// SetPlaceholder sets fallback text when no image is available.
func (iv *ImageView) SetPlaceholder(s string) {
	iv.placeholder = s
}

// Lines returns the rendered image as a slice of strings, one per row.
// Always returns exactly `height` lines. Safe for embedding in a
// line-by-line TUI layout.
//
// Two-tier cache + async fallback:
//
//  1. L1 (in-memory, per ImageView): hit → return immediately.
//  2. L2 (disk): hit → load + populate L1 → return.
//  3. Miss → enqueue an async render job, return placeholder lines.
//     The pool emits ImageRenderedMsg on completion; the controller
//     triggers a View() refresh and the next Lines() call hits L1
//     via L2 (the worker wrote the result to disk).
//
// This keeps View() non-blocking: the grid paints immediately with
// placeholders, and posters fade in as their renders complete.
func (iv *ImageView) Lines() []string {
	iv.mu.Lock()
	defer iv.mu.Unlock()

	if iv.path == "" {
		return iv.placeholderLines()
	}

	// L1 in-memory cache.
	if iv.cachedPath == iv.path && iv.cachedW == iv.width && iv.cachedH == iv.height && len(iv.cachedLines) > 0 {
		return iv.cachedLines
	}

	// L2 disk cache. Skip the renderer entirely on warm hit.
	if cached, hit := imageRenderCacheGet(iv.path, iv.width, iv.height); hit {
		lines := iv.parseRendered(string(cached))
		iv.cachedPath = iv.path
		iv.cachedW = iv.width
		iv.cachedH = iv.height
		iv.cachedLines = lines
		return lines
	}

	// Miss on both tiers. Kick an async render and return placeholder
	// lines for this frame — the worker pool will refill the disk
	// cache and fire ImageRenderedMsg, which triggers a View() refresh
	// that hits L2 on the next pass.
	EnqueueImageRender(iv.path, iv.width, iv.height)
	return iv.placeholderLines()
}

// View returns the rendered image as a single string (lines joined with \n).
func (iv *ImageView) View() string {
	return strings.Join(iv.Lines(), "\n")
}

// parseRendered splits the renderer's raw output into one line per row,
// padded/trimmed to exactly iv.height lines. Shared by the live-render
// and disk-cache-hit paths so they produce identical layouts.
func (iv *ImageView) parseRendered(raw string) []string {
	raw = strings.TrimRight(raw, "\n")

	if iv.protocol == ImageProtocolKitty {
		// Kitty output is a single escape sequence that occupies
		// width x height cells. Pad with empty lines so the caller
		// can reserve the right number of rows in the layout.
		lines := make([]string, iv.height)
		lines[0] = raw
		for i := 1; i < iv.height; i++ {
			lines[i] = ""
		}
		return lines
	}

	// Symbols: one text line per row.
	lines := strings.Split(raw, "\n")
	// Pad or trim to exact height.
	for len(lines) < iv.height {
		lines = append(lines, "")
	}
	if len(lines) > iv.height {
		lines = lines[:iv.height]
	}
	return lines
}

func (iv *ImageView) placeholderLines() []string {
	lines := make([]string, iv.height)
	if iv.placeholder != "" {
		mid := iv.height / 2
		// Center the placeholder horizontally
		pad := (iv.width - len([]rune(iv.placeholder))) / 2
		if pad < 0 {
			pad = 0
		}
		lines[mid] = strings.Repeat(" ", pad) + iv.placeholder
	}
	return lines
}
