package components

import (
	"fmt"
	"math"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type RatingBarStyle int

const (
	RatingBarStyleCompact RatingBarStyle = iota
	RatingBarStyleDetailed
	RatingBarStyleMinimal
)

type FractionalRating struct {
	maxRating  float64
	segments   int
	style      RatingBarStyle
	barWidth   int
	filledChar string
	emptyChar  string
}

func NewFractionalRating(opts ...RatingBarOption) *FractionalRating {
	rb := &FractionalRating{
		maxRating:  10.0,
		segments:   10,
		style:      RatingBarStyleCompact,
		barWidth:   10,
		filledChar: "█",
		emptyChar:  "░",
	}
	for _, opt := range opts {
		opt(rb)
	}
	return rb
}

type RatingBarOption func(*FractionalRating)

func WithMaxRating(max float64) RatingBarOption {
	return func(rb *FractionalRating) {
		rb.maxRating = max
		rb.segments = int(math.Ceil(max))
	}
}

func WithBarWidth(width int) RatingBarOption {
	return func(rb *FractionalRating) {
		rb.barWidth = width
	}
}

func WithStyle(style RatingBarStyle) RatingBarOption {
	return func(rb *FractionalRating) {
		rb.style = style
	}
}

func (r *FractionalRating) Render(ratingStr string) string {
	rating := parseRating(ratingStr)
	if rating < 0 {
		return ""
	}
	return r.RenderValue(rating)
}

func (r *FractionalRating) RenderValue(rating float64) string {
	switch r.style {
	case RatingBarStyleMinimal:
		return r.renderMinimal(rating)
	case RatingBarStyleDetailed:
		return r.renderDetailed(rating)
	default:
		return r.renderCompact(rating)
	}
}

func (r *FractionalRating) renderCompact(rating float64) string {
	if rating < 0 || rating > r.maxRating {
		return ""
	}

	filled := int(math.Round(rating / r.maxRating * float64(r.barWidth)))
	if filled > r.barWidth {
		filled = r.barWidth
	}
	empty := r.barWidth - filled

	yellow := lipgloss.NewStyle().Foreground(theme.T.Yellow())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextMuted())

	bar := yellow.Render(strings.Repeat(r.filledChar, filled))
	bar += dim.Render(strings.Repeat(r.emptyChar, empty))

	ratingText := fmt.Sprintf(" %.1f", rating)
	ratingStyled := yellow.Render(ratingText)

	return "[" + bar + "]" + ratingStyled
}

func (r *FractionalRating) renderMinimal(rating float64) string {
	if rating < 0 || rating > r.maxRating {
		return ""
	}

	filled := int(math.Round(rating / r.maxRating * float64(r.barWidth)))
	if filled > r.barWidth {
		filled = r.barWidth
	}
	empty := r.barWidth - filled

	yellow := lipgloss.NewStyle().Foreground(theme.T.Yellow())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextMuted())

	bar := yellow.Render(strings.Repeat(r.filledChar, filled))
	bar += dim.Render(strings.Repeat(r.emptyChar, empty))

	return bar
}

func (r *FractionalRating) renderDetailed(rating float64) string {
	if rating < 0 || rating > r.maxRating {
		return ""
	}

	yellow := lipgloss.NewStyle().Foreground(theme.T.Yellow())

	percentage := (rating / r.maxRating) * 100
	return yellow.Render(fmt.Sprintf("%.1f/%.0f (%.0f%%)", rating, r.maxRating, percentage))
}

func parseRating(s string) float64 {
	if s == "" {
		return -1
	}
	var rating float64
	n, err := fmt.Sscanf(s, "%f", &rating)
	if err != nil || n != 1 {
		return -1
	}
	return rating
}

func CompactRatingBar(rating string, width int) string {
	rb := NewFractionalRating(
		WithBarWidth(width),
		WithStyle(RatingBarStyleCompact),
	)
	return rb.Render(rating)
}

func MinimalRatingBar(rating string, width int) string {
	rb := NewFractionalRating(
		WithBarWidth(width),
		WithStyle(RatingBarStyleMinimal),
	)
	return rb.Render(rating)
}
