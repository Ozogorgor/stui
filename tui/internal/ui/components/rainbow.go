package components

// rainbow.go — animated rainbow border for the focused grid card.
//
// Lipgloss applies a single Foreground colour across all border cells, so
// gradients aren't expressible through its `BorderForeground`. This file
// renders a rounded box manually, colouring each border character with a
// per-position HSL hue. Animating `RainbowOffset` (driven by a tea.Tick
// in the model) makes the rainbow flow clockwise around the perimeter.
//
// Used only for the SELECTED card in the Movies/Series/Music grids — every
// other card retains its flat `theme.T.Border()` so the focused item reads
// as the spotlight rather than visual noise everywhere.

import (
	"fmt"
	"math"
	"strings"

	"charm.land/lipgloss/v2"
)

// RainbowOffset is the package-level hue offset (in degrees) added to each
// border cell's base hue. The model's rainbow-tick handler increments this
// each frame; `RainbowBorder` reads it via the `hueOffset` parameter that
// callers pass in. We keep it package-level so a new param doesn't have to
// thread through every grid call site.
//
// Bubble Tea's Update + View run serially, so a plain int is safe.
var RainbowOffset int

// RainbowBorder renders `content` inside a rounded box where every border
// character is coloured by an HSL hue derived from its perimeter position.
//
// Output is exactly `w` cells wide and `h` rows tall, matching what
// `lipgloss.NewStyle().Border(RoundedBorder).Padding(0, 1).Width(w).Height(h)`
// would produce — except every border cell carries its own ANSI 24-bit fg.
//
// `content` lines are padded/truncated to `w-4` visible cells per row and
// `h-2` rows total. The 1-col side padding (between bar and content) is
// emitted internally so callers can render their inner content directly.
func RainbowBorder(content string, w, h int, hueOffset int) string {
	if w < 4 || h < 2 {
		return content
	}
	innerW := w - 4
	innerH := h - 2

	lines := strings.Split(content, "\n")
	for len(lines) < innerH {
		lines = append(lines, "")
	}
	lines = lines[:innerH]
	for i := range lines {
		lines[i] = padOrTruncCells(lines[i], innerW)
	}

	// Clockwise perimeter positions (total = 2*w + 2*(h-2)):
	//   Top row     [0       .. w-1]          w cells, left→right
	//   Right mid   [w       .. w+h-3]        h-2 cells, top→bottom
	//   Bottom-R    [w+h-2]                   bottom-right corner
	//   Bottom mid  [w+h-1   .. 2w+h-4]       w-2 cells, right→left
	//   Bottom-L    [2w+h-3]                  bottom-left corner
	//   Left mid    [2w+h-2  .. 2w+2h-5]      h-2 cells, bottom→top
	perim := 2*w + 2*(h-2)
	if perim < 1 {
		perim = 1
	}

	var b strings.Builder

	// Top row.
	b.WriteString(colorize("╭", 0, perim, hueOffset))
	for i := 0; i < w-2; i++ {
		b.WriteString(colorize("─", 1+i, perim, hueOffset))
	}
	b.WriteString(colorize("╮", w-1, perim, hueOffset))
	b.WriteByte('\n')

	// Mid rows. Right bar walks top→bottom (positions w..w+h-3); left bar
	// walks top→bottom in render order but is positioned bottom→top in
	// clockwise traversal — topmost mid row owns the highest left-mid pos.
	for r := 0; r < innerH; r++ {
		leftPos := 2*w + 2*h - 5 - r
		rightPos := w + r
		b.WriteString(colorize("│", leftPos, perim, hueOffset))
		b.WriteByte(' ')
		b.WriteString(lines[r])
		b.WriteByte(' ')
		b.WriteString(colorize("│", rightPos, perim, hueOffset))
		b.WriteByte('\n')
	}

	// Bottom row. Bottom-left corner first (render order), then mid cells
	// emitted left→right but their clockwise positions decrement.
	b.WriteString(colorize("╰", 2*w+h-3, perim, hueOffset))
	for i := 0; i < w-2; i++ {
		b.WriteString(colorize("─", (2*w+h-4)-i, perim, hueOffset))
	}
	b.WriteString(colorize("╯", w+h-2, perim, hueOffset))

	return b.String()
}

// colorize wraps `s` with an ANSI 24-bit fg derived from the HSL hue at
// perimeter `pos`, offset by `hueOffset` (degrees). Saturation/lightness
// are tuned for vivid-but-not-eye-bleed (95% / 55%).
func colorize(s string, pos, perim, hueOffset int) string {
	hueDeg := ((pos*360/perim)+hueOffset)%360 + 360
	hue := float64(hueDeg % 360)
	r, g, b := hslToRGB(hue, 0.95, 0.55)
	return lipgloss.NewStyle().
		Foreground(lipgloss.Color(fmt.Sprintf("#%02x%02x%02x", r, g, b))).
		Render(s)
}

// hslToRGB converts HSL (h ∈ [0,360), s/l ∈ [0,1]) to 8-bit RGB.
func hslToRGB(h, s, l float64) (uint8, uint8, uint8) {
	c := (1 - math.Abs(2*l-1)) * s
	x := c * (1 - math.Abs(math.Mod(h/60, 2)-1))
	m := l - c/2
	var r1, g1, b1 float64
	switch {
	case h < 60:
		r1, g1, b1 = c, x, 0
	case h < 120:
		r1, g1, b1 = x, c, 0
	case h < 180:
		r1, g1, b1 = 0, c, x
	case h < 240:
		r1, g1, b1 = 0, x, c
	case h < 300:
		r1, g1, b1 = x, 0, c
	default:
		r1, g1, b1 = c, 0, x
	}
	return uint8(math.Round((r1 + m) * 255)),
		uint8(math.Round((g1 + m) * 255)),
		uint8(math.Round((b1 + m) * 255))
}

// padOrTruncCells pads/truncates `s` to exactly `n` visible cells. Visible
// width is computed via lipgloss so embedded ANSI escapes are ignored.
func padOrTruncCells(s string, n int) string {
	vis := lipgloss.Width(s)
	if vis == n {
		return s
	}
	if vis < n {
		return s + strings.Repeat(" ", n-vis)
	}
	runes := []rune(s)
	for lipgloss.Width(string(runes)) > n && len(runes) > 0 {
		runes = runes[:len(runes)-1]
	}
	return string(runes)
}
