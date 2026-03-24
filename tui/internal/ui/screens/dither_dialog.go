package screens

// dither_dialog.go — TPDF dither + noise shaping settings dialog.
//
// Layout (centered):
//
//	┌──────────────── Dither ─────────────────┐
//	│                                         │
//	│  ▶ Auto-detect   [off]                  │
//	│    Enabled       [off]                  │
//	│                                         │
//	│    Bit depth     16                     │
//	│    Noise shaping none                   │
//	│                                         │
//	│  tab next  +/- adjust  q close         │
//	└─────────────────────────────────────────┘
//
// Key bindings:
//
//	tab/shift+tab  — cycle fields 0–3
//	+ / =          — nudge up   (step bit depth, cycle shaping, flip toggles)
//	- / _          — nudge down
//	q / esc        — commit all four IPC keys and close

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// bitDepths is the ordered set of selectable output bit depths.
var bitDepths = []int{8, 16, 20, 24, 32}

// shapingNames maps shapingIdx to the IPC string sent to the runtime.
var shapingNames = []string{
	"none", "lipshitz", "fweighted", "modified_e_weighted",
	"improved_e_weighted", "shibata", "low_shibata", "high_shibata", "gesemann",
}

// DitherDialogModel is the dither settings dialog screen.
type DitherDialogModel struct {
	enabled     bool
	auto        bool
	bitDepthIdx int // index into bitDepths
	shapingIdx  int // index into shapingNames
	field       int // 0=auto, 1=enabled, 2=bitDepth, 3=shaping
	width       int
	height      int
	sendFn      func(key string, value interface{}) tea.Cmd
}

// NewDitherDialogModel constructs the dialog with default values (16-bit, no shaping).
// sendFn may be nil (safe in tests).
func NewDitherDialogModel(sendFn func(key string, value interface{}) tea.Cmd) DitherDialogModel {
	// Default: bit_depth=16 (index 1), shaping=none (index 0)
	return DitherDialogModel{
		bitDepthIdx: 1,
		shapingIdx:  0,
		sendFn:      sendFn,
	}
}

// SetSize stores terminal dimensions for centering.
func (m *DitherDialogModel) SetSize(w, h int) {
	m.width = w
	m.height = h
}

func (m DitherDialogModel) Init() tea.Cmd { return nil }

func (m DitherDialogModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
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
		case "q", "esc":
			return m, m.commit()
		}
	}
	return m, nil
}

func (m *DitherDialogModel) nudge(dir int) {
	switch m.field {
	case 0:
		m.auto = dir > 0
	case 1:
		m.enabled = dir > 0
	case 2:
		idx := m.bitDepthIdx + dir
		if idx < 0 {
			idx = 0
		} else if idx >= len(bitDepths) {
			idx = len(bitDepths) - 1
		}
		m.bitDepthIdx = idx
	case 3:
		n := len(shapingNames)
		m.shapingIdx = ((m.shapingIdx + dir) % n + n) % n
	}
}

func (m DitherDialogModel) commit() tea.Cmd {
	if m.sendFn == nil {
		return screen.PopCmd()
	}
	return tea.Batch(
		m.sendFn("dsp.dither_enabled", m.enabled),
		m.sendFn("dsp.dither_auto", m.auto),
		m.sendFn("dsp.dither_bit_depth", bitDepths[m.bitDepthIdx]),
		m.sendFn("dsp.dither_noise_shaping", shapingNames[m.shapingIdx]),
		screen.PopCmd(),
	)
}

func (m DitherDialogModel) View() tea.View {
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
		autoNote = "  (auto: active)"
	}

	lines := []string{
		"",
		fmt.Sprintf("  %s Auto-detect   [%s]", cursor(0), boolStr(m.auto)),
		fmt.Sprintf("  %s Enabled       [%s]%s", cursor(1), boolStr(m.enabled), autoNote),
		"",
		fmt.Sprintf("  %s Bit depth     %d", cursor(2), bitDepths[m.bitDepthIdx]),
		fmt.Sprintf("  %s Noise shaping %s", cursor(3), shapingNames[m.shapingIdx]),
		"",
		hintBar("tab next", "+/- adjust", "q close"),
	}

	body := lipgloss.JoinVertical(lipgloss.Left, lines...)

	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(th.Border).
		Padding(0, 2).
		Width(54).
		Render(lipgloss.NewStyle().
			Foreground(th.Text).
			Bold(true).
			Render("  Dither") + "\n" + body)

	content := lipgloss.Place(m.width, m.height,
		lipgloss.Center, lipgloss.Center, box)

	return tea.View{Content: content}
}
