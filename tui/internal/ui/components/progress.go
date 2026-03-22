package components

import (
	"fmt"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type ProgressBar struct {
	value       float64
	max         float64
	width       int
	label       string
	showValue   bool
	showPercent bool
	filledChar  string
	emptyChar   string
}

type ProgressBarOption func(*ProgressBar)

func WithLabel(label string) ProgressBarOption {
	return func(p *ProgressBar) {
		p.label = label
	}
}

func WithWidth(width int) ProgressBarOption {
	return func(p *ProgressBar) {
		p.width = width
	}
}

func WithShowValue(show bool) ProgressBarOption {
	return func(p *ProgressBar) {
		p.showValue = show
	}
}

func WithShowPercent(show bool) ProgressBarOption {
	return func(p *ProgressBar) {
		p.showPercent = show
	}
}

func WithChars(filled, empty string) ProgressBarOption {
	return func(p *ProgressBar) {
		p.filledChar = filled
		p.emptyChar = empty
	}
}

func NewProgressBar(value, max float64, opts ...ProgressBarOption) *ProgressBar {
	p := &ProgressBar{
		value:       value,
		max:         max,
		width:       20,
		showValue:   true,
		showPercent: true,
		filledChar:  "█",
		emptyChar:   "░",
	}
	for _, opt := range opts {
		opt(p)
	}
	return p
}

func (p *ProgressBar) SetValue(value float64) {
	p.value = value
	if p.value > p.max {
		p.value = p.max
	}
	if p.value < 0 {
		p.value = 0
	}
}

func (p *ProgressBar) Percentage() float64 {
	if p.max == 0 {
		return 0
	}
	return (p.value / p.max) * 100
}

func (p *ProgressBar) View() string {
	percent := p.Percentage()
	barLen := p.width - 2
	filled := int((percent / 100) * float64(barLen))
	if filled > barLen {
		filled = barLen
	}
	empty := barLen - filled

	bar := strings.Repeat(p.filledChar, filled) + strings.Repeat(p.emptyChar, empty)

	accent := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	var content string
	if p.showValue && p.showPercent {
		content = fmt.Sprintf("%s %s %d%%", p.label, accent.Render(bar), int(percent))
	} else if p.showValue {
		content = fmt.Sprintf("%s %s", p.label, accent.Render(bar))
	} else if p.showPercent {
		content = fmt.Sprintf("%s %d%%", accent.Render(bar), int(percent))
	} else {
		content = accent.Render(bar)
	}

	return dim.Render(content)
}

func truncate(s string, max int) string {
	if max <= 0 {
		return ""
	}
	runes := []rune(s)
	if len(runes) <= max {
		return s
	}
	return string(runes[:max-1]) + "…"
}

type DownloadProgress struct {
	items map[string]*DownloadItem
	width int
}

type DownloadItem struct {
	Name       string
	Total      int64
	Downloaded int64
	Status     string
}

func NewDownloadProgress(width int) *DownloadProgress {
	return &DownloadProgress{
		items: make(map[string]*DownloadItem),
		width: width,
	}
}

func (dp *DownloadProgress) Add(id, name string, total int64) {
	dp.items[id] = &DownloadItem{
		Name:       name,
		Total:      total,
		Downloaded: 0,
		Status:     "Downloading",
	}
}

func (dp *DownloadProgress) Update(id string, downloaded int64) {
	if item, ok := dp.items[id]; ok {
		item.Downloaded = downloaded
	}
}

func (dp *DownloadProgress) Complete(id string) {
	if item, ok := dp.items[id]; ok {
		item.Status = "Complete"
		item.Downloaded = item.Total
	}
}

func (dp *DownloadProgress) Remove(id string) {
	delete(dp.items, id)
}

func (dp *DownloadProgress) View() string {
	if len(dp.items) == 0 {
		return ""
	}

	var sb strings.Builder
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	accent := lipgloss.NewStyle().Foreground(theme.T.Accent())
	green := lipgloss.NewStyle().Foreground(theme.T.Green())

	for id, item := range dp.items {
		var percent float64
		if item.Total > 0 {
			percent = float64(item.Downloaded) / float64(item.Total) * 100
		}

		barLen := dp.width - len(item.Name) - 20
		if barLen < 5 {
			barLen = 20
		}
		filled := int((percent / 100) * float64(barLen))
		bar := strings.Repeat("█", filled) + strings.Repeat("░", barLen-filled)

		status := green.Render("✓")
		if item.Status == "Downloading" {
			status = accent.Render("⟳")
		}

		sb.WriteString(fmt.Sprintf("%s %s %s %d%%\n", status, dim.Render(truncate(item.Name, 20)), bar, int(percent)))
		_ = id
	}

	return sb.String()
}
