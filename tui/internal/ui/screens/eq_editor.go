package screens

import (
	"encoding/json"
	"fmt"
	"math"
	"strings"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// ── EQ types (mirror runtime/src/dsp/config.rs) ───────────────────────────

type EqFilterType string

const (
	EqFilterTypePeak      EqFilterType = "peak"
	EqFilterTypeLowShelf  EqFilterType = "low_shelf"
	EqFilterTypeHighShelf EqFilterType = "high_shelf"
	EqFilterTypeLowPass   EqFilterType = "low_pass"
	EqFilterTypeHighPass  EqFilterType = "high_pass"
	EqFilterTypeNotch     EqFilterType = "notch"
)

var eqFilterTypes = []EqFilterType{
	EqFilterTypePeak, EqFilterTypeLowShelf, EqFilterTypeHighShelf,
	EqFilterTypeLowPass, EqFilterTypeHighPass, EqFilterTypeNotch,
}

func (f EqFilterType) String() string {
	switch f {
	case EqFilterTypePeak:
		return "Peak"
	case EqFilterTypeLowShelf:
		return "LowShelf"
	case EqFilterTypeHighShelf:
		return "HighShelf"
	case EqFilterTypeLowPass:
		return "LowPass"
	case EqFilterTypeHighPass:
		return "HighPass"
	case EqFilterTypeNotch:
		return "Notch"
	}
	return string(f)
}

// hasGain returns false for filter types where gain is not applicable.
func (f EqFilterType) hasGain() bool {
	return f != EqFilterTypeLowPass && f != EqFilterTypeHighPass && f != EqFilterTypeNotch
}

// EqBand represents a single parametric EQ band.
type EqBand struct {
	Enabled    bool         `json:"enabled"`
	FilterType EqFilterType `json:"filter_type"`
	Freq       float64      `json:"freq"`
	GainDB     float64      `json:"gain_db"`
	Q          float64      `json:"q"`
}

// ── Biquad coefficient + magnitude computation ────────────────────────────

type biquadCoeffs struct{ b0, b1, b2, a1, a2 float64 }

func computeBiquadCoeffs(band EqBand, sampleRate float64) biquadCoeffs {
	w0 := 2 * math.Pi * band.Freq / sampleRate
	sinW := math.Sin(w0)
	cosW := math.Cos(w0)
	alpha := sinW / (2 * band.Q)

	var b0, b1, b2, a0, a1, a2 float64
	switch band.FilterType {
	case EqFilterTypePeak:
		a := math.Pow(10, band.GainDB/40)
		b0 = 1 + alpha*a
		b1 = -2 * cosW
		b2 = 1 - alpha*a
		a0 = 1 + alpha/a
		a1 = -2 * cosW
		a2 = 1 - alpha/a
	case EqFilterTypeLowShelf:
		a := math.Pow(10, band.GainDB/40)
		sqrtA := math.Sqrt(a)
		alphaS := sinW / 2 * math.Sqrt((a+1/a)*(1/band.Q-1)+2)
		b0 = a * ((a + 1) - (a-1)*cosW + 2*sqrtA*alphaS)
		b1 = 2 * a * ((a - 1) - (a+1)*cosW)
		b2 = a * ((a + 1) - (a-1)*cosW - 2*sqrtA*alphaS)
		a0 = (a + 1) + (a-1)*cosW + 2*sqrtA*alphaS
		a1 = -2 * ((a - 1) + (a+1)*cosW)
		a2 = (a + 1) + (a-1)*cosW - 2*sqrtA*alphaS
	case EqFilterTypeHighShelf:
		a := math.Pow(10, band.GainDB/40)
		sqrtA := math.Sqrt(a)
		alphaS := sinW / 2 * math.Sqrt((a+1/a)*(1/band.Q-1)+2)
		b0 = a * ((a + 1) + (a-1)*cosW + 2*sqrtA*alphaS)
		b1 = -2 * a * ((a - 1) + (a+1)*cosW)
		b2 = a * ((a + 1) + (a-1)*cosW - 2*sqrtA*alphaS)
		a0 = (a + 1) - (a-1)*cosW + 2*sqrtA*alphaS
		a1 = 2 * ((a - 1) - (a+1)*cosW)
		a2 = (a + 1) - (a-1)*cosW - 2*sqrtA*alphaS
	case EqFilterTypeLowPass:
		b0 = (1 - cosW) / 2
		b1 = 1 - cosW
		b2 = (1 - cosW) / 2
		a0 = 1 + alpha
		a1 = -2 * cosW
		a2 = 1 - alpha
	case EqFilterTypeHighPass:
		b0 = (1 + cosW) / 2
		b1 = -(1 + cosW)
		b2 = (1 + cosW) / 2
		a0 = 1 + alpha
		a1 = -2 * cosW
		a2 = 1 - alpha
	case EqFilterTypeNotch:
		b0 = 1
		b1 = -2 * cosW
		b2 = 1
		a0 = 1 + alpha
		a1 = -2 * cosW
		a2 = 1 - alpha
	}
	return biquadCoeffs{b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0}
}

func biquadMagnitudeDB(c biquadCoeffs, omega float64) float64 {
	cos1, sin1 := math.Cos(omega), math.Sin(omega)
	cos2, sin2 := math.Cos(2*omega), math.Sin(2*omega)
	numRe := c.b0 + c.b1*cos1 + c.b2*cos2
	numIm := c.b1*sin1 + c.b2*sin2
	denRe := 1.0 + c.a1*cos1 + c.a2*cos2
	denIm := c.a1*sin1 + c.a2*sin2
	ratio := (numRe*numRe + numIm*numIm) / (denRe*denRe + denIm*denIm)
	if ratio <= 0 {
		return -100.0
	}
	return 20 * math.Log10(math.Sqrt(ratio))
}

// combinedMagnitudeDB sums dB contributions of all enabled bands at freq.
func combinedMagnitudeDB(bands []EqBand, sampleRate, freqHz float64) float64 {
	omega := 2 * math.Pi * freqHz / sampleRate
	total := 0.0
	for _, b := range bands {
		if !b.Enabled {
			continue
		}
		c := computeBiquadCoeffs(b, sampleRate)
		total += biquadMagnitudeDB(c, omega)
	}
	return total
}

// ComputeCurveRow maps a frequency column to a terminal row for the braille curve.
// Returns the 0-indexed row (0 = top). col is 0-indexed within totalCols.
// height is the curve zone height in braille cells (each = 4 subpixels tall).
func ComputeCurveRow(bands []EqBand, sampleRate float64, totalCols, height, col int) int {
	if totalCols <= 1 {
		return height / 2
	}
	// Map column to frequency (log scale)
	t := float64(col) / float64(totalCols-1)
	freq := 20.0 * math.Pow(1000.0, t) // 20Hz at t=0, 20000Hz at t=1
	db := combinedMagnitudeDB(bands, sampleRate, freq)
	db = math.Max(-20, math.Min(20, db))
	// Map +20dB → row 0, -20dB → row height-1
	row := int((1.0 - (db+20)/40.0) * float64(height-1))
	return row
}

// ── Braille renderer ──────────────────────────────────────────────────────

// Braille Unicode 2×4 subpixel bit positions:
//
//	col%2=0: bits 0(row0), 1(row1), 2(row2), 6(row3)
//	col%2=1: bits 3(row0), 4(row1), 5(row2), 7(row3)
var brailleBit = [2][4]byte{
	{0, 1, 2, 6},
	{3, 4, 5, 7},
}

// renderBrailleCurve renders the frequency response curve into a string of
// braille characters. width and height are in terminal cells (each cell = 2×4 subpixels).
func renderBrailleCurve(bands []EqBand, sampleRate float64, width, height int) string {
	// cells[row][col] accumulates subpixel bits
	cells := make([][]byte, height)
	for i := range cells {
		cells[i] = make([]byte, width)
	}

	// Number of frequency samples = 2 * width (matching braille subpixel columns)
	nSamples := 2 * width
	// Total subpixel rows = 4 * height
	nRows := 4 * height
	// Centre subpixel row = nRows/2 (0dB line)
	centreSubRow := nRows / 2

	for px := 0; px < nSamples; px++ {
		t := float64(px) / float64(nSamples-1)
		freq := 20.0 * math.Pow(1000.0, t)
		db := combinedMagnitudeDB(bands, sampleRate, freq)
		db = math.Max(-20, math.Min(20, db))
		// Map dB to subpixel row: +20dB → 0, -20dB → nRows-1
		subRow := int((1.0 - (db+20)/40.0) * float64(nRows-1))
		// Map subpixel to cell
		cellCol := px / 2
		cellRow := subRow / 4
		if cellCol >= width || cellRow >= height {
			continue
		}
		bitIdx := brailleBit[px%2][subRow%4]
		cells[cellRow][cellCol] |= 1 << bitIdx
	}

	// Render 0dB reference line (overwrites with ─)
	refCellRow := centreSubRow / 4

	var sb strings.Builder
	for r := 0; r < height; r++ {
		if r > 0 {
			sb.WriteByte('\n')
		}
		for c := 0; c < width; c++ {
			if r == refCellRow && cells[r][c] == 0 {
				sb.WriteRune('─')
			} else {
				sb.WriteRune(rune(0x2800 + int(cells[r][c])))
			}
		}
	}
	return sb.String()
}

// ── EqEditorModel ─────────────────────────────────────────────────────────

// eqField enumerates which column is active in the band table.
type eqField int

const (
	eqFieldType eqField = iota
	eqFieldFreq
	eqFieldGain
	eqFieldQ
)

// EqEditorModel is the full-screen parametric EQ editor screen.
// Implements screen.Screen.
type EqEditorModel struct {
	bands      []EqBand
	cursor     int     // selected band row
	field      eqField // active column
	editing    bool    // inline text input active
	editBuf    string  // current text input buffer
	enabled    bool
	bypass     bool
	sampleRate float64
	width      int
	height     int
	sendFn     func(key string, value interface{}) tea.Cmd // nil-safe
}

// NewEqEditorModel constructs the editor.
// sendFn is called to emit SettingsChangedMsg commands; pass nil in tests.
func NewEqEditorModel(sendFn func(string, interface{}) tea.Cmd, sampleRate float64) *EqEditorModel {
	if sampleRate <= 0 {
		sampleRate = 44100
	}
	return &EqEditorModel{
		bands:      nil,
		enabled:    true,
		sampleRate: sampleRate,
		sendFn:     sendFn,
	}
}

func (m *EqEditorModel) SetSize(w, h int) { m.width = w; m.height = h }

func (m *EqEditorModel) AddBand(b EqBand) { m.bands = append(m.bands, b) }

func (m *EqEditorModel) Init() tea.Cmd { return nil }

func (m *EqEditorModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		return m.handleKey(msg)
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
	}
	return m, nil
}

func (m *EqEditorModel) handleKey(msg tea.KeyMsg) (screen.Screen, tea.Cmd) {
	if m.editing {
		return m.handleEditKey(msg)
	}
	switch msg.String() {
	case "q", "esc":
		return m, tea.Batch(m.commitBands(), func() tea.Msg { return screen.PopMsg{} })
	case "a":
		if len(m.bands) < 10 {
			m.bands = append(m.bands, EqBand{
				Enabled: true, FilterType: EqFilterTypePeak,
				Freq: 1000, GainDB: 0, Q: 1.0,
			})
			m.cursor = len(m.bands) - 1
		}
	case "d":
		if len(m.bands) > 0 {
			m.bands = append(m.bands[:m.cursor], m.bands[m.cursor+1:]...)
			if m.cursor >= len(m.bands) && m.cursor > 0 {
				m.cursor--
			}
			return m, m.commitBands()
		}
	case " ":
		if len(m.bands) > 0 {
			m.bands[m.cursor].Enabled = !m.bands[m.cursor].Enabled
			return m, m.commitBands()
		}
	case "tab":
		m.cursor = (m.cursor + 1) % max(1, len(m.bands))
		return m, m.commitBands()
	case "shift+tab":
		if len(m.bands) > 0 {
			m.cursor = (m.cursor - 1 + len(m.bands)) % len(m.bands)
		}
		return m, m.commitBands()
	case "left":
		m.field = eqField((int(m.field) - 1 + 4) % 4)
		m.skipGainIfNotApplicable(-1)
	case "right":
		m.field = eqField((int(m.field) + 1) % 4)
		m.skipGainIfNotApplicable(+1)
	case "+", "=":
		m.nudge(+1)
	case "-", "_":
		m.nudge(-1)
	case "e":
		if len(m.bands) > 0 && !(m.field == eqFieldGain && !m.bands[m.cursor].FilterType.hasGain()) {
			m.editing = true
			m.editBuf = m.fieldValueString()
		}
	case "b":
		m.bypass = !m.bypass
		return m, m.commitBands()
	}
	return m, nil
}

func (m *EqEditorModel) skipGainIfNotApplicable(dir int) {
	if len(m.bands) == 0 {
		return
	}
	if m.field == eqFieldGain && !m.bands[m.cursor].FilterType.hasGain() {
		m.field = eqField((int(m.field) + dir + 4) % 4)
	}
}

func (m *EqEditorModel) nudge(dir int) {
	if len(m.bands) == 0 {
		return
	}
	b := &m.bands[m.cursor]
	switch m.field {
	case eqFieldType:
		idx := 0
		for i, t := range eqFilterTypes {
			if t == b.FilterType {
				idx = i
				break
			}
		}
		idx = (idx + dir + len(eqFilterTypes)) % len(eqFilterTypes)
		b.FilterType = eqFilterTypes[idx]
	case eqFieldFreq:
		if dir > 0 {
			b.Freq *= 1.05
		} else {
			b.Freq /= 1.05
		}
		b.Freq = math.Max(20, math.Min(20000, b.Freq))
	case eqFieldGain:
		if b.FilterType.hasGain() {
			b.GainDB = math.Max(-20, math.Min(20, b.GainDB+float64(dir)*0.5))
		}
	case eqFieldQ:
		b.Q = math.Max(0.1, math.Min(10.0, b.Q+float64(dir)*0.05))
	}
}

func (m *EqEditorModel) fieldValueString() string {
	if len(m.bands) == 0 {
		return ""
	}
	b := m.bands[m.cursor]
	switch m.field {
	case eqFieldType:
		return string(b.FilterType)
	case eqFieldFreq:
		return fmt.Sprintf("%.0f", b.Freq)
	case eqFieldGain:
		return fmt.Sprintf("%.1f", b.GainDB)
	case eqFieldQ:
		return fmt.Sprintf("%.2f", b.Q)
	}
	return ""
}

func (m *EqEditorModel) handleEditKey(msg tea.KeyMsg) (screen.Screen, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.editing = false
		m.editBuf = ""
	case "enter":
		m.commitEdit()
		m.editing = false
		m.editBuf = ""
		return m, m.commitBands()
	case "backspace":
		if len(m.editBuf) > 0 {
			_, sz := utf8.DecodeLastRuneInString(m.editBuf)
			m.editBuf = m.editBuf[:len(m.editBuf)-sz]
		}
	default:
		// tea.KeyMsg is an interface; msg.String() returns the key text (e.g. "3", ".", "-").
		s := msg.String()
		if len(s) == 1 && (s[0] >= '0' && s[0] <= '9' || s[0] == '.' || s[0] == '-') {
			m.editBuf += s
		}
	}
	return m, nil
}

func (m *EqEditorModel) commitEdit() {
	if len(m.bands) == 0 || m.editBuf == "" {
		return
	}
	b := &m.bands[m.cursor]
	var v float64
	if _, err := fmt.Sscanf(m.editBuf, "%f", &v); err != nil {
		return
	}
	switch m.field {
	case eqFieldFreq:
		b.Freq = math.Max(20, math.Min(20000, v))
	case eqFieldGain:
		if b.FilterType.hasGain() {
			b.GainDB = math.Max(-20, math.Min(20, v))
		}
	case eqFieldQ:
		b.Q = math.Max(0.1, math.Min(10.0, v))
	}
}

func (m *EqEditorModel) commitBands() tea.Cmd {
	if m.sendFn == nil {
		return nil
	}
	data, err := json.Marshal(m.bands)
	if err != nil {
		return nil
	}
	cmds := []tea.Cmd{
		m.sendFn("dsp.eq_bands", string(data)),
		m.sendFn("dsp.eq_enabled", m.enabled),
		m.sendFn("dsp.eq_bypass", m.bypass),
	}
	return tea.Batch(cmds...)
}

// Note: max() is a Go 1.21+ built-in — do NOT define it locally.

func (m *EqEditorModel) View() tea.View {
	if m.width == 0 {
		return tea.NewView("  Parametric EQ\n")
	}

	accent := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())
	dimmed := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	selected := lipgloss.NewStyle().Foreground(theme.T.Accent()).Reverse(true)

	var sb strings.Builder

	// ── Header ────────────────────────────────────────────────────────────
	ena := "enabled"
	if !m.enabled {
		ena = "disabled"
	}
	byp := "off"
	if m.bypass {
		byp = "on"
	}
	sb.WriteString(accent.Render("  Parametric EQ") + "  " +
		normal.Render("["+ena+"]") + "  " +
		normal.Render("[bypass: "+byp+"]") + "\n")
	sb.WriteString(strings.Repeat("─", m.width) + "\n")

	// ── Braille curve ─────────────────────────────────────────────────────
	curveHeight := (m.height - 10) / 4 // approx 40% of height in braille rows
	if curveHeight < 2 {
		curveHeight = 2
	}
	curveWidth := m.width - 8 // leave room for dB labels
	if curveWidth < 1 { curveWidth = 1 }

	activeBands := make([]EqBand, 0, len(m.bands))
	for _, b := range m.bands {
		if b.Enabled {
			activeBands = append(activeBands, b)
		}
	}

	curve := renderBrailleCurve(activeBands, m.sampleRate, curveWidth, curveHeight)
	lines := strings.Split(curve, "\n")
	for i, line := range lines {
		label := "      "
		if i == 0 {
			label = "+20dB "
		}
		if i == curveHeight/2 {
			label = "  0dB "
		}
		if i == curveHeight-1 {
			label = "-20dB "
		}
		sb.WriteString(dimmed.Render(label) + normal.Render(line) + "\n")
	}
	sb.WriteString(dimmed.Render("      ") +
		dimmed.Render("20Hz") +
		strings.Repeat(" ", (curveWidth-12)/2) +
		dimmed.Render("1kHz") +
		strings.Repeat(" ", (curveWidth-12)/2) +
		dimmed.Render("20kHz") + "\n")
	sb.WriteString(strings.Repeat("─", m.width) + "\n")

	// ── Band table ────────────────────────────────────────────────────────
	header := fmt.Sprintf("  %-3s %-3s %-10s %-9s %-7s %-6s\n",
		"#", "on", "type", "freq", "gain", "Q")
	sb.WriteString(dimmed.Render(header))

	for i, b := range m.bands {
		onStr := "✗"
		if b.Enabled {
			onStr = "✓"
		}

		gainStr := "---"
		if b.FilterType.hasGain() {
			gainStr = fmt.Sprintf("%+.1f", b.GainDB)
		}
		freqStr := fmt.Sprintf("%.0f Hz", b.Freq)
		qStr := fmt.Sprintf("%.2f", b.Q)
		typeStr := b.FilterType.String()

		// Highlight active field on selected row
		if i == m.cursor {
			fields := []string{typeStr, freqStr, gainStr, qStr}
			for fi, fs := range fields {
				if eqField(fi) == m.field {
					if m.editing {
						fields[fi] = "[" + m.editBuf + "_]"
					} else {
						fields[fi] = selected.Render(fs)
					}
				}
			}
			typeStr, freqStr, gainStr, qStr = fields[0], fields[1], fields[2], fields[3]
		}

		row := fmt.Sprintf("  %-3d %-3s %-10s %-9s %-7s %-6s\n",
			i+1, onStr, typeStr, freqStr, gainStr, qStr)

		style := normal
		if i == m.cursor {
			style = accent
		}
		sb.WriteString(style.Render(row))
	}

	if len(m.bands) == 0 {
		sb.WriteString(dimmed.Render("  (no bands — press 'a' to add)\n"))
	}

	addHint := "a add"
	if len(m.bands) >= 10 {
		addHint = dimmed.Render("a add")
	}
	sb.WriteString(strings.Repeat("─", m.width) + "\n")
	sb.WriteString(dimmed.Render(
		"  "+addHint+"  d del  space toggle  tab next  +/- nudge  e edit\n"+
			"  b bypass  q close\n"))

	return tea.NewView(sb.String())
}
