package screens

// crossfeed_dialog.go — BS2B headphone crossfeed settings dialog.
//
// Layout (centered):
//
//	┌─────────────── Crossfeed ───────────────┐
//	│                                         │
//	│  Auto-detect   [off]                    │
//	│  Enabled       [off]                    │
//	│                                         │
//	│  Feed level    0.45                     │
//	│  Cutoff        700 Hz                   │
//	│                                         │
//	│  Presets:  [Default]  [Cmoy]  [Jmeier] │
//	│                                         │
//	│  tab next  +/- nudge  p preset  q close │
//	└─────────────────────────────────────────┘
//
// Key bindings:
//
//	tab/shift+tab  — cycle fields 0–3
//	+ / =          — nudge up   (feed ±0.05, cutoff ±10 Hz, toggles flip)
//	- / _          — nudge down
//	p              — cycle presets (Default → Cmoy → Jmeier → Default)
//	q / esc        — commit all four IPC keys and close

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

type crossfeedPreset struct {
	name      string
	feedLevel float64
	cutoffHz  float64
}

var crossfeedPresets = []crossfeedPreset{
	{"Default", 0.45, 700},
	{"Cmoy", 0.65, 700},
	{"Jmeier", 0.90, 650},
}

// CrossfeedDialogModel is the crossfeed settings dialog screen.
type CrossfeedDialogModel struct {
	enabled   bool
	auto      bool
	feedLevel float64 // 0.0–0.9
	cutoffHz  float64 // 300–700
	field     int     // 0=auto, 1=enabled, 2=feed, 3=cutoff
	presetIdx int     // 0=Default, 1=Cmoy, 2=Jmeier
	width     int
	height    int
	sendFn    func(key string, value interface{}) tea.Cmd
}

// NewCrossfeedDialogModel constructs the dialog with Default preset values.
// sendFn may be nil (safe in tests).
func NewCrossfeedDialogModel(sendFn func(key string, value interface{}) tea.Cmd) CrossfeedDialogModel {
	return CrossfeedDialogModel{
		feedLevel: crossfeedPresets[0].feedLevel,
		cutoffHz:  crossfeedPresets[0].cutoffHz,
		sendFn:    sendFn,
	}
}

// SetSize stores the terminal dimensions for centering the dialog.
func (m *CrossfeedDialogModel) SetSize(w, h int) {
	m.width = w
	m.height = h
}

func (m CrossfeedDialogModel) Init() tea.Cmd { return nil }

func (m CrossfeedDialogModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyPressMsg:
		switch msg.String() {
		case "tab":
			m.field = (m.field + 1) % 4
		case "shift+tab":
			m.field = (m.field + 3) % 4
		case "+", "=":
			m.nudge(+1)
		case "-", "_":
			m.nudge(-1)
		case "p":
			m.presetIdx = (m.presetIdx + 1) % len(crossfeedPresets)
			p := crossfeedPresets[m.presetIdx]
			m.feedLevel = p.feedLevel
			m.cutoffHz = p.cutoffHz
		case "q", "esc":
			return m, m.commit()
		}
	}
	return m, nil
}

func (m *CrossfeedDialogModel) nudge(dir int) {
	switch m.field {
	case 0:
		m.auto = dir > 0
	case 1:
		m.enabled = dir > 0
	case 2:
		m.feedLevel = clampF64(m.feedLevel+float64(dir)*0.05, 0.0, 0.9)
	case 3:
		m.cutoffHz = clampF64(m.cutoffHz+float64(dir)*10, 300, 700)
	}
}

func clampF64(v, lo, hi float64) float64 {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

func (m CrossfeedDialogModel) commit() tea.Cmd {
	if m.sendFn == nil {
		return screen.PopCmd()
	}
	return tea.Batch(
		m.sendFn("dsp.crossfeed_enabled", m.enabled),
		m.sendFn("dsp.crossfeed_auto", m.auto),
		m.sendFn("dsp.crossfeed_feed_level", m.feedLevel),
		m.sendFn("dsp.crossfeed_cutoff_hz", m.cutoffHz),
		screen.PopCmd(),
	)
}

func (m CrossfeedDialogModel) View() tea.View {
	th := theme.T.P()

	boolStr := func(v bool) string {
		if v {
			return "on "
		}
		return "off"
	}

	cursor := func(i int) string {
		if m.field == i {
			return lipgloss.NewStyle().Foreground(th.Accent).Render("▶")
		}
		return " "
	}

	autoNote := ""
	if m.auto {
		autoNote = "  (auto: detected)"
	}

	lines := []string{
		"",
		fmt.Sprintf("  %s Auto-detect   [%s]", cursor(0), boolStr(m.auto)),
		fmt.Sprintf("  %s Enabled       [%s]%s", cursor(1), boolStr(m.enabled), autoNote),
		"",
		fmt.Sprintf("  %s Feed level    %.2f", cursor(2), m.feedLevel),
		fmt.Sprintf("  %s Cutoff        %.0f Hz", cursor(3), m.cutoffHz),
		"",
	}

	presetLine := "  Presets: "
	for i, p := range crossfeedPresets {
		label := fmt.Sprintf("[%s]", p.name)
		if i == m.presetIdx {
			label = lipgloss.NewStyle().Foreground(th.Accent).Render(label)
		}
		presetLine += " " + label
	}
	lines = append(lines, presetLine, "")
	lines = append(lines, hintBar("tab next", "+/- nudge", "p preset", "q close"))

	body := lipgloss.JoinVertical(lipgloss.Left, lines...)

	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(th.Border).
		Padding(0, 2).
		Width(54).
		Render(lipgloss.NewStyle().
			Foreground(th.Text).
			Bold(true).
			Render("  Crossfeed") + "\n" + body)

	content := lipgloss.Place(m.width, m.height,
		lipgloss.Center, lipgloss.Center, box)

	return tea.View{Content: content}
}
