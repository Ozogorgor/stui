// mosaic_render.go — pure-Go image rendering via charmbracelet/x/mosaic.
//
// Replaces the previous chafa shell-out (~50-200ms per fork) with an
// in-process decode + half-block render. Output shape is unchanged
// (newline-separated ANSI lines) so the existing disk cache, in-memory
// cache, and parse pipeline don't need to know which renderer produced
// the bytes.

package components

import (
	"image"
	_ "image/jpeg" // decoder registration
	_ "image/png"  // decoder registration
	"os"
	"strings"

	"github.com/charmbracelet/x/mosaic"
	_ "golang.org/x/image/webp" // TMDB occasionally serves WebP posters
)

// cellAspectRatio captures the height:width ratio of a terminal cell
// in the user's font. Most monospace fonts render cells at roughly
// 1:2 (twice as tall as wide). Tunable here without touching the
// rendering code.
const cellAspectRatio = 2.0

// renderMosaic decodes the image file at `path` and renders it into a
// `cellW` x `cellH` cell box, preserving the source aspect ratio.
// Returns the rendered ANSI bytes on success or an error if the file
// can't be opened or decoded (caller falls back to placeholder).
//
// Sizing notes:
//   - mosaic.Width / mosaic.Height take PIXEL counts. Each output cell
//     consumes a 2x2 pixel block (mosaic iterates by +=2 in both axes).
//   - Cells are not square on screen — they're roughly 1:2 (W:H), so
//     fitting a poster's pixel-aspect into a "cellW x cellH" area
//     requires accounting for that, otherwise portrait posters render
//     squashed. fitToCells does that math.
func renderMosaic(path string, cellW, cellH int) ([]byte, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	img, _, err := image.Decode(f)
	if err != nil {
		return nil, err
	}

	pixelW, pixelH := fitToCells(img.Bounds().Dx(), img.Bounds().Dy(), cellW, cellH)
	m := mosaic.New().Width(pixelW).Height(pixelH)
	// Emit raw mosaic output (left-aligned, top-anchored, no padding).
	// Callers (card.go, related strips) own the layout via lipgloss
	// Width/Align — adding our own padding here would compound with
	// theirs and shift posters off-center.
	return []byte(strings.TrimRight(m.Render(img), "\n")), nil
}

// fitToCells computes mosaic pixel dimensions for an image of
// (srcW x srcH) pixels rendered into a (cellW x cellH) cell box,
// preserving the source aspect ratio. Output is always within the
// cell box; the unused dimension is left short and the caller pads.
//
// Math: each cell occupies 1 horizontal display unit but
// `cellAspectRatio` vertical display units (cells are ~2x as tall
// as wide). The source's aspect ratio is matched against the box's
// effective display aspect, then the limiting dimension is chosen.
// Final pixel counts are cells * 2 because mosaic uses 2x2 pixel
// blocks per cell in both axes.
func fitToCells(srcW, srcH, cellW, cellH int) (pixelW, pixelH int) {
	if srcW <= 0 || srcH <= 0 || cellW <= 0 || cellH <= 0 {
		return cellW * 2, cellH * 2
	}
	srcAspect := float64(srcW) / float64(srcH)
	boxAspect := float64(cellW) / (float64(cellH) * cellAspectRatio)

	var fitCellW, fitCellH float64
	if srcAspect > boxAspect {
		// Source is wider than box → width-limited.
		fitCellW = float64(cellW)
		fitCellH = float64(cellW) / srcAspect / cellAspectRatio
	} else {
		// Source is taller than box → height-limited.
		fitCellH = float64(cellH)
		fitCellW = float64(cellH) * srcAspect * cellAspectRatio
	}

	pixelW = int(fitCellW * 2)
	pixelH = int(fitCellH * 2)
	if pixelW < 2 {
		pixelW = 2
	}
	if pixelH < 2 {
		pixelH = 2
	}
	return pixelW, pixelH
}
