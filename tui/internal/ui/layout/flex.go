package layout

// PanelSplit describes a two-panel horizontal or vertical layout.
// Used for the detail overlay (poster | info), status bars, and future
// side-by-side views.
type PanelSplit struct {
	LeftWidth  int
	RightWidth int
	Total      int
}

// SplitHorizontal divides totalWidth into two panels according to a ratio.
//
//	ratio = 0.0 → left gets nothing
//	ratio = 0.5 → equal halves
//	ratio = 1.0 → left gets everything
//
// A minimum width of minLeft / minRight cells is guaranteed for each panel
// (if the total is too small, both minimums are clamped as best-effort).
func SplitHorizontal(totalWidth int, ratio float64, minLeft, minRight int) PanelSplit {
	if ratio < 0 {
		ratio = 0
	}
	if ratio > 1 {
		ratio = 1
	}
	left := int(float64(totalWidth) * ratio)
	// Enforce minimums
	if left < minLeft {
		left = minLeft
	}
	right := totalWidth - left
	if right < minRight {
		right = minRight
		left = totalWidth - right
		if left < 0 {
			left = 0
		}
	}
	return PanelSplit{LeftWidth: left, RightWidth: right, Total: totalWidth}
}

// DetailSplit returns the panel split for the detail overlay.
// The poster occupies the left panel, metadata the right.
//
//	< 100 cols → 35% poster / 65% info
//	≥ 100 cols → 40% poster / 60% info
func DetailSplit(termWidth int) PanelSplit {
	ratio := 0.35
	if termWidth >= 100 {
		ratio = 0.40
	}
	return SplitHorizontal(termWidth, ratio, 20, 30)
}

// FlexRow distributes totalWidth across n equal columns with optional gaps.
// Returns a slice of per-column widths (may vary by ±1 cell due to rounding).
func FlexRow(totalWidth, cols, gap int) []int {
	if cols <= 0 {
		return nil
	}
	totalGap := gap * (cols - 1)
	available := totalWidth - totalGap
	if available < cols {
		available = cols // ensure at least 1 cell per column
	}
	base := available / cols
	remainder := available % cols
	widths := make([]int, cols)
	for i := range widths {
		widths[i] = base
		if i < remainder {
			widths[i]++ // distribute leftover cells to leading columns
		}
	}
	return widths
}

// Inset returns the usable content area after applying symmetric padding.
func Inset(width, height, horizontalPad, verticalPad int) (w, h int) {
	w = width - 2*horizontalPad
	h = height - 2*verticalPad
	if w < 0 {
		w = 0
	}
	if h < 0 {
		h = 0
	}
	return w, h
}

// Clamp constrains v to the range [lo, hi].
func Clamp(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}
