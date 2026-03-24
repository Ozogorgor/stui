package screens

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

type audioTab int

const (
	tabOutput audioTab = iota
	tabDSP
	tabDSD
	tabConvolution
	tabCrossfeed
	tabMidSide
	tabDither
)

var audioTabNames = []string{
	"Output",
	"DSP",
	"DSD",
	"Convolution",
	"Crossfeed",
	"Mid/Side",
	"Dither",
}

type AudioSettingsModel struct {
	tab          audioTab
	selectedIdx  int
	width        int
	height       int
	editing      bool
	editInput    textinput.Model
	settingItems map[audioTab][]*settingItem
}

func NewAudioSettingsModel() AudioSettingsModel {
	m := AudioSettingsModel{
		tab: tabOutput,
		settingItems: map[audioTab][]*settingItem{
			tabOutput: {
				{
					label:       "Output Target",
					key:         "dsp.output_target",
					kind:        settingChoice,
					choiceVals:  []string{"pipewire", "alsa", "roon_raat", "mpd"},
					choiceIdx:   0,
					description: "Audio output backend",
				},
				{
					label:       "Sample Rate",
					key:         "dsp.output_sample_rate",
					kind:        settingChoice,
					choiceVals:  []string{"96000", "192000", "384000", "768000"},
					choiceIdx:   1,
					description: "Target output sample rate in Hz",
				},
				{
					label:       "Buffer Size",
					key:         "dsp.buffer_size",
					kind:        settingInt,
					intVal:      4096,
					minVal:      512,
					maxVal:      16384,
					description: "Processing buffer size in samples",
				},
				{
					label:       "ALSA Device",
					key:         "dsp.alsa_device",
					kind:        settingString,
					strVal:      "hw:0,0",
					description: "ALSA hardware device (e.g. hw:0,0)",
				},
				{
					label:       "PipeWire Role",
					key:         "dsp.pipewire_role",
					kind:        settingChoice,
					choiceVals:  []string{"Music", "Production"},
					choiceIdx:   0,
					description: "PipeWire stream role",
				},
			},
			tabDSP: {
				{
					label:       "Enable DSP",
					key:         "dsp.enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable DSP processing pipeline",
				},
				{
					label:       "Resampling",
					key:         "dsp.resample_enabled",
					kind:        settingBool,
					boolVal:     true,
					description: "Enable sample rate conversion",
				},
				{
					label:       "Upsample Ratio",
					key:         "dsp.upsample_ratio",
					kind:        settingChoice,
					choiceVals:  []string{"1", "2", "4", "8", "16"},
					choiceIdx:   2,
					description: "Upsampling multiplier",
				},
				{
					label:       "Filter Type",
					key:         "dsp.filter_type",
					kind:        settingChoice,
					choiceVals:  []string{"fast", "slow", "synchronous"},
					choiceIdx:   2,
					description: "Resampling filter characteristics",
				},
				{
					label:       "Input Rate",
					key:         "dsp.input_sample_rate",
					kind:        settingChoice,
					choiceVals:  []string{"44100", "48000", "88200", "96000"},
					choiceIdx:   1,
					description: "Expected input sample rate",
				},
			},
			tabDSD: {
				{
					label:       "DSD→PCM",
					key:         "dsp.dsd_to_pcm_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Convert DSD to PCM",
				},
				{
					label:       "Output Mode",
					key:         "dsp.output_mode",
					kind:        settingChoice,
					choiceVals:  []string{"pcm", "dsd", "dsd_to_pcm"},
					choiceIdx:   0,
					description: "Audio output format",
				},
				{
					label:       "DSD Rate",
					key:         "dsp.dsd_output_rate",
					kind:        settingChoice,
					choiceVals:  []string{"88200", "176400", "352800", "705600"},
					choiceIdx:   2,
					description: "DSD to PCM output rate",
				},
			},
			tabConvolution: {
				{
					label:       "Enable",
					key:         "dsp.convolution_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable convolution processing",
				},
				{
					label:       "Bypass",
					key:         "dsp.convolution_bypass",
					kind:        settingBool,
					boolVal:     true,
					description: "Bypass convolution filter",
				},
				{
					label:       "Filter File",
					key:         "dsp.convolution_filter_path",
					kind:        settingPath,
					strVal:      "",
					description: "Path to convolution filter WAV file",
				},
			},
			tabCrossfeed: {
				{
					label:       "Enable",
					key:         "dsp.crossfeed_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable headphone crossfeed",
				},
				{
					label:       "Auto-detect",
					key:         "dsp.crossfeed_auto",
					kind:        settingBool,
					boolVal:     false,
					description: "Auto-detect headphones and enable crossfeed",
				},
				{
					label:       "Feed Level",
					key:         "dsp.crossfeed_feed_level",
					kind:        settingFloat,
					floatVal:    0.45,
					description: "Crossfeed blend level (0.0-0.9)",
				},
				{
					label:       "Cutoff (Hz)",
					key:         "dsp.crossfeed_cutoff_hz",
					kind:        settingInt,
					intVal:      700,
					minVal:      300,
					maxVal:      700,
					description: "Crossfeed lowpass cutoff frequency",
				},
				{
					label:       "DC Offset Filter",
					key:         "dsp.dc_offset_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Remove DC offset and very low frequency drift",
				},
				{
					label:       "DC Cutoff (Hz)",
					key:         "dsp.dc_offset_cutoff_hz",
					kind:        settingChoice,
					choiceVals:  []string{"5", "10", "15", "20", "30"},
					choiceIdx:   1,
					description: "DC filter cutoff frequency",
				},
			},
			tabMidSide: {
				{
					label:       "Enable M/S",
					key:         "dsp.ms_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable Mid/Side processing",
				},
				{
					label:       "Width",
					key:         "dsp.ms_width",
					kind:        settingFloat,
					floatVal:    1.0,
					description: "Stereo width (0.0=mono, 1.0=normal, >1=wider)",
				},
				{
					label:       "Mid Gain",
					key:         "dsp.ms_mid_gain",
					kind:        settingFloat,
					floatVal:    1.0,
					description: "Mid (center) channel gain",
				},
				{
					label:       "Side Gain",
					key:         "dsp.ms_side_gain",
					kind:        settingFloat,
					floatVal:    1.0,
					description: "Side channel gain",
				},
			},
			tabDither: {
				{
					label:       "Enable",
					key:         "dsp.dither_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable TPDF dither before output",
				},
				{
					label:       "Auto-detect",
					key:         "dsp.dither_auto",
					kind:        settingBool,
					boolVal:     false,
					description: "Auto-enable when output is ALSA at 16-bit",
				},
				{
					label:       "Bit Depth",
					key:         "dsp.dither_bit_depth",
					kind:        settingChoice,
					choiceVals:  []string{"8", "16", "20", "24", "32"},
					choiceIdx:   1,
					description: "Output bit depth for quantization",
				},
				{
					label:       "Noise Shaping",
					key:         "dsp.dither_noise_shaping",
					kind:        settingChoice,
					choiceVals:  []string{"none", "lipshitz", "fweighted", "modified_e_weighted", "improved_e_weighted", "shibata", "low_shibata", "high_shibata", "gesemann"},
					choiceIdx:   0,
					description: "Noise shaping algorithm",
				},
			},
		},
	}
	return m
}

func (m AudioSettingsModel) Init() tea.Cmd { return nil }

func (m AudioSettingsModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	if m.editing {
		switch msg := msg.(type) {
		case tea.KeyMsg:
			switch msg.String() {
			case "enter":
				item := m.getCurrentItem()
				m.editing = false
				if item != nil && item.kind == settingPath {
					newPath := m.editInput.Value()
					if isValidPath(newPath) {
						item.strVal = newPath
						return m, settingChangedCmd(item)
					}
					return m, nil
				}
				return m, settingChangedCmd(item)
			case "esc":
				m.editing = false
				return m, nil
			default:
				newInput, cmd := m.editInput.Update(msg)
				m.editInput = newInput
				return m, cmd
			}
		}
		return m, nil
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height

	case tea.KeyMsg:
		switch msg.String() {
		case "left", "h":
			if m.tab > 0 {
				m.tab--
				m.selectedIdx = 0
			}
		case "right", "l":
			if m.tab < audioTab(len(audioTabNames)-1) {
				m.tab++
				m.selectedIdx = 0
			}
		case "up", "k":
			if m.selectedIdx > 0 {
				m.selectedIdx--
			}
		case "down", "j":
			if m.selectedIdx < len(m.settingItems[m.tab])-1 {
				m.selectedIdx++
			}
		case "enter":
			item := m.getCurrentItem()
			if item != nil {
				if item.kind == settingPath {
					ti := textinput.New()
					ti.SetValue(item.strVal)
					ti.CursorEnd()
					inputW := m.width - 40
					if inputW < 20 {
						inputW = 20
					}
					ti.SetWidth(inputW)
					ti.CharLimit = 512
					cmd := ti.Focus()
					m.editInput = ti
					m.editing = true
					return m, cmd
				}
				item.toggle()
				return m, settingChangedCmd(item)
			}
		case "+", "=":
			item := m.getCurrentItem()
			if item != nil {
				item.adjust(+1)
				return m, settingChangedCmd(item)
			}
		case "-", "_":
			item := m.getCurrentItem()
			if item != nil {
				item.adjust(-1)
				return m, settingChangedCmd(item)
			}
		case "esc":
			return m, screen.PopCmd()
		}
	}
	return m, nil
}

func (m AudioSettingsModel) getCurrentItem() *settingItem {
	items := m.settingItems[m.tab]
	if m.selectedIdx < len(items) {
		return items[m.selectedIdx]
	}
	return nil
}

func (m AudioSettingsModel) View() tea.View {
	if m.width == 0 {
		return tea.NewView("Audio Settings\n")
	}

	headerStyle := lipgloss.NewStyle().
		Bold(true).
		Foreground(theme.T.Accent()).
		PaddingLeft(2)

	header := headerStyle.Render("🎧 Audio Settings")

	var tabLines []string
	for i, name := range audioTabNames {
		style := theme.T.TabStyle()
		if i == int(m.tab) {
			style = theme.T.TabActiveStyle()
		}
		tabLines = append(tabLines, style.Render(" "+name+" "))
	}
	tabBar := lipgloss.JoinHorizontal(lipgloss.Top, tabLines...)

	items := m.settingItems[m.tab]
	var itemLines []string
	catStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Bold(true)
	itemLines = append(itemLines, catStyle.Render("  "+audioTabNames[m.tab]))
	itemLines = append(itemLines, "")

	normalStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text())
	valStyle := lipgloss.NewStyle().
		Foreground(theme.T.TextDim())

	rightW := m.width - 24
	if rightW < 30 {
		rightW = 30
	}

	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())

	for i, item := range items {
		prefix := "  "
		if i == m.selectedIdx {
			prefix = accentStyle.Render("► ")
		}
		labelW := rightW - 14
		if labelW < 10 {
			labelW = 10
		}
		labelPad := strings.TrimRight(fmt.Sprintf("%-*s", labelW, item.label), " ")

		val := valStyle.Render(item.displayValue())
		line := normalStyle.Render(prefix+labelPad) + val
		itemLines = append(itemLines, line)
	}

	if m.editing {
		item := m.getCurrentItem()
		if item != nil && item.kind == settingPath {
			// +2 for the category header and blank line prepended above
			lineIdx := 2 + m.selectedIdx
			if lineIdx < len(itemLines) {
				itemLines[lineIdx] = normalStyle.Render("  "+item.label+" ") + m.editInput.View()
			}
		}
	}

	itemsPanel := lipgloss.NewStyle().
		Width(rightW).
		PaddingLeft(2).
		Render(strings.Join(itemLines, "\n"))

	footer := hintBar("←→ switch tabs", "enter toggle", "+/- adjust", "esc back")

	return tea.NewView(header + "\n\n" + tabBar + "\n\n" + itemsPanel + "\n\n" + footer + "\n")
}
