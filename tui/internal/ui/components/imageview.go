// imageview.go — Reusable terminal image rendering component.
//
// Renders images using the best available terminal protocol:
//   - Kitty graphics protocol (Ghostty, Kitty, WezTerm) — true color images
//   - Unicode symbols via chafa (all other terminals) — half-block fallback
//
// Usage:
//
//	iv := components.NewImageView(20, 10) // width x height in cells
//	iv.SetImage("/path/to/cover.jpg")
//	rendered := iv.View() // returns string for embedding in a View
//
// The component caches rendered output and only re-shells to chafa when
// the image path or dimensions change. Safe to call View() every frame.

package components

import (
	"fmt"
	"os/exec"
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
	cachedPath   string
	cachedW, cachedH int
	cachedLines  []string // pre-split lines for symbols; single element for kitty
	placeholder  string
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
func (iv *ImageView) Lines() []string {
	iv.mu.Lock()
	defer iv.mu.Unlock()

	if iv.path == "" {
		return iv.placeholderLines()
	}

	// Check cache
	if iv.cachedPath == iv.path && iv.cachedW == iv.width && iv.cachedH == iv.height && len(iv.cachedLines) > 0 {
		return iv.cachedLines
	}

	// Render via chafa
	lines := iv.render()
	iv.cachedPath = iv.path
	iv.cachedW = iv.width
	iv.cachedH = iv.height
	iv.cachedLines = lines
	return lines
}

// View returns the rendered image as a single string (lines joined with \n).
func (iv *ImageView) View() string {
	return strings.Join(iv.Lines(), "\n")
}

func (iv *ImageView) render() []string {
	format := "symbols"
	if iv.protocol == ImageProtocolKitty {
		format = "kitty"
	}

	out, err := exec.Command("chafa",
		"--format", format,
		"--size", fmt.Sprintf("%dx%d", iv.width, iv.height),
		"--animate", "off",
		iv.path,
	).Output()
	if err != nil || len(out) == 0 {
		return iv.placeholderLines()
	}

	raw := strings.TrimRight(string(out), "\n")

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

	// Symbols: one text line per row
	lines := strings.Split(raw, "\n")
	// Pad or trim to exact height
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
		lines[mid] = iv.placeholder
	}
	return lines
}
