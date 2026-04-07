package components

import (
	"fmt"
	"math"
	"strings"

	"charm.land/lipgloss/v2"
)

type ScrollMode int

const (
	ScrollModePush ScrollMode = iota
	ScrollModeCenter
)

type VirtualizedList struct {
	totalItems   int
	visibleStart int
	visibleEnd   int
	cursor       int
	scrollMode   ScrollMode
	headerHeight int
	footerHeight int
	availableH   int
	itemHeight   int
}

type VirtualizedListOption func(*VirtualizedList)

func WithScrollMode(mode ScrollMode) VirtualizedListOption {
	return func(v *VirtualizedList) {
		v.scrollMode = mode
	}
}

func WithHeaderHeight(h int) VirtualizedListOption {
	return func(v *VirtualizedList) {
		v.headerHeight = h
	}
}

func WithFooterHeight(h int) VirtualizedListOption {
	return func(v *VirtualizedList) {
		v.footerHeight = h
	}
}

func WithItemHeight(h int) VirtualizedListOption {
	return func(v *VirtualizedList) {
		v.itemHeight = h
	}
}

func NewVirtualizedList(totalItems, cursor, availableH int, opts ...VirtualizedListOption) *VirtualizedList {
	v := &VirtualizedList{
		totalItems:   totalItems,
		cursor:       cursor,
		availableH:   availableH,
		scrollMode:   ScrollModePush,
		headerHeight: 0,
		footerHeight: 0,
		itemHeight:   1,
	}
	for _, opt := range opts {
		opt(v)
	}
	v.calculate()
	return v
}

func (v *VirtualizedList) calculate() {
	if v.totalItems == 0 || v.availableH <= 0 {
		v.visibleStart = 0
		v.visibleEnd = 0
		return
	}

	visibleItems := v.availableH - v.headerHeight - v.footerHeight
	if visibleItems < 1 {
		visibleItems = 1
	}

	switch v.scrollMode {
	case ScrollModeCenter:
		scroll := v.cursor - visibleItems/2
		if scroll < 0 {
			scroll = 0
		}
		maxScroll := v.totalItems - visibleItems
		if maxScroll < 0 {
			maxScroll = 0
		}
		if scroll > maxScroll {
			scroll = maxScroll
		}
		v.visibleStart = scroll
		v.visibleEnd = int(math.Min(float64(scroll+visibleItems), float64(v.totalItems)))

	case ScrollModePush:
		fallthrough
	default:
		v.visibleStart = 0
		if v.cursor >= visibleItems {
			v.visibleStart = v.cursor - visibleItems + 1
		}
		v.visibleEnd = int(math.Min(float64(v.visibleStart+visibleItems), float64(v.totalItems)))
	}
}

func (v *VirtualizedList) Update(totalItems, cursor, availableH int) {
	v.totalItems = totalItems
	v.cursor = cursor
	v.availableH = availableH
	v.calculate()
}

func (v *VirtualizedList) VisibleRange() (start, end int) {
	return v.visibleStart, v.visibleEnd
}

func (v *VirtualizedList) HasMoreAbove() bool {
	return v.visibleStart > 0
}

func (v *VirtualizedList) HasMoreBelow() bool {
	return v.visibleEnd < v.totalItems
}

func (v *VirtualizedList) Position() (current, total int) {
	return v.cursor + 1, v.totalItems
}

func (v *VirtualizedList) ScrollIndicator(dim lipgloss.Style) string {
	if v.totalItems == 0 {
		return ""
	}
	above := ""
	below := ""
	if v.HasMoreAbove() {
		above = dim.Render("↑ more")
	}
	if v.HasMoreBelow() {
		below = dim.Render("↓ more")
	}
	if above != "" && below != "" {
		return above + "  " + below
	}
	return above + below
}

func (v *VirtualizedList) Scrollbar(dim lipgloss.Style) string {
	if v.totalItems == 0 {
		return ""
	}
	visibleItems := v.visibleEnd - v.visibleStart
	if visibleItems >= v.totalItems {
		return ""
	}

	trackHeight := visibleItems
	if trackHeight < 3 {
		trackHeight = 3
	}

	position := float64(v.visibleStart) / float64(v.totalItems)
	thumbSize := float64(visibleItems) / float64(v.totalItems)
	if thumbSize < 0.1 {
		thumbSize = 0.1
	}
	if thumbSize > 1.0 {
		thumbSize = 1.0
	}

	thumbLen := int(float64(trackHeight) * thumbSize)
	if thumbLen < 1 {
		thumbLen = 1
	}

	thumbPos := int(float64(trackHeight-thumbLen) * position)
	if thumbPos < 0 {
		thumbPos = 0
	}
	if thumbPos > trackHeight-thumbLen {
		thumbPos = trackHeight - thumbLen
	}

	var bar strings.Builder
	bar.Grow(trackHeight)
	for i := 0; i < trackHeight; i++ {
		if i >= thumbPos && i < thumbPos+thumbLen {
			bar.WriteRune('█')
		} else {
			bar.WriteRune('░')
		}
	}
	return dim.Render(bar.String())
}

func (v *VirtualizedList) VerticalScrollbar(width int, dim lipgloss.Style) string {
	if v.totalItems == 0 || width < 1 {
		return ""
	}
	visibleItems := v.visibleEnd - v.visibleStart
	if visibleItems >= v.totalItems {
		return ""
	}

	position := float64(v.visibleStart) / float64(v.totalItems)
	thumbSize := float64(visibleItems) / float64(v.totalItems)
	if thumbSize < 0.05 {
		thumbSize = 0.05
	}
	if thumbSize > 1.0 {
		thumbSize = 1.0
	}

	thumbLen := int(float64(width) * thumbSize)
	if thumbLen < 1 {
		thumbLen = 1
	}

	thumbPos := int(float64(width-thumbLen) * position)
	if thumbPos < 0 {
		thumbPos = 0
	}
	if thumbPos > width-thumbLen {
		thumbPos = width - thumbLen
	}

	var bar strings.Builder
	bar.Grow(width + 2)
	bar.WriteString("│")
	for i := 0; i < width; i++ {
		if i >= thumbPos && i < thumbPos+thumbLen {
			bar.WriteRune('█')
		} else {
			bar.WriteRune('│')
		}
	}
	bar.WriteString("│")
	return dim.Render(bar.String())
}

func (v *VirtualizedList) RangeIndicator(dim lipgloss.Style) string {
	if v.totalItems == 0 {
		return ""
	}
	return dim.Render(v.FormatRange())
}

func (v *VirtualizedList) FormatRange() string {
	if v.totalItems == 0 {
		return ""
	}
	start := v.visibleStart + 1
	end := v.visibleEnd
	return formatRange(start, end, v.totalItems)
}

func formatRange(start, end, total int) string {
	return fmt.Sprintf("%d-%d of %d", start, end, total)
}
