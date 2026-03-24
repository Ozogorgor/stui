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
	tabSignal
	tabCorrection
	tabStereo
	tabFormat
)

var audioTabNames = []string{
	"Output",
	"Signal",
	"Correction",
	"Stereo",
	"Format",
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

// audioHeader returns a non-interactive section label rendered as a sub-heading.
func audioHeader(label string) *settingItem {
	return &settingItem{label: label, kind: settingInfo}
}

// firstSelectableIdx returns the index of the first non-header item in a tab.
func firstSelectableIdx(items []*settingItem) int {
	for i, item := range items {
		if item.kind != settingInfo {
			return i
		}
	}
	return 0
}

func NewAudioSettingsModel() AudioSettingsModel {
	m := AudioSettingsModel{
		tab: tabOutput,
		settingItems: map[audioTab][]*settingItem{

			// ── Output ──────────────────────────────────────────────────────────
			// Hardware sink, sample rate, and backend-specific parameters.
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
					description: "Processing buffer size in samples — smaller = lower latency, larger = more stable",
				},
				{
					label:       "ALSA Device",
					key:         "dsp.alsa_device",
					kind:        settingString,
					strVal:      "hw:0,0",
					description: "ALSA hardware device string (e.g. hw:0,0) — only used when output target is alsa",
				},
				{
					label:       "PipeWire Role",
					key:         "dsp.pipewire_role",
					kind:        settingChoice,
					choiceVals:  []string{"Music", "Production"},
					choiceIdx:   0,
					description: "PipeWire stream role — Production requests bypass of OS resampler",
				},
			},

			// ── Signal ──────────────────────────────────────────────────────────
			// Pipeline master switch, DC offset filter (first in chain),
			// and resampler settings.
			tabSignal: {
				{
					label:       "Enable DSP",
					key:         "dsp.enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Master switch — enables the entire DSP processing pipeline",
				},
				audioHeader("DC Offset Filter"),
				{
					label:       "DC Offset Filter",
					key:         "dsp.dc_offset_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Remove DC bias and very low frequency drift before further processing",
				},
				{
					label:       "DC Cutoff (Hz)",
					key:         "dsp.dc_offset_cutoff_hz",
					kind:        settingInt,
					intVal:      10,
					minVal:      1,
					maxVal:      100,
					description: "DC high-pass cutoff frequency — 5-20 Hz removes DC, 80 Hz also removes rumble",
				},
				audioHeader("Resampler"),
				{
					label:       "Resampling",
					key:         "dsp.resample_enabled",
					kind:        settingBool,
					boolVal:     true,
					description: "Enable sample rate conversion from input rate to output rate",
				},
				{
					label:       "Input Rate",
					key:         "dsp.input_sample_rate",
					kind:        settingChoice,
					choiceVals:  []string{"44100", "48000", "88200", "96000"},
					choiceIdx:   0,
					description: "Expected source sample rate — set to match your media",
				},
				{
					label:       "Upsample Ratio",
					key:         "dsp.upsample_ratio",
					kind:        settingChoice,
					choiceVals:  []string{"1", "2", "4", "8", "16"},
					choiceIdx:   2,
					description: "Upsampling multiplier applied on top of the input rate",
				},
				{
					label:       "Filter Type",
					key:         "dsp.filter_type",
					kind:        settingChoice,
					choiceVals:  []string{"fast", "slow", "synchronous"},
					choiceIdx:   2,
					description: "Resampling filter — fast (low latency), slow (steep roll-off), synchronous (phase-linear)",
				},
			},

			// ── Correction ──────────────────────────────────────────────────────
			// Convolution room/speaker correction and LUFS loudness normalization.
			tabCorrection: {
				audioHeader("Convolution"),
				{
					label:       "Enable",
					key:         "dsp.convolution_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Apply convolution — room correction or speaker EQ via impulse response",
				},
				{
					label:       "Bypass",
					key:         "dsp.convolution_bypass",
					kind:        settingBool,
					boolVal:     true,
					description: "Bypass convolution without disabling it — useful for quick A/B comparison",
				},
				{
					label:       "Filter File",
					key:         "dsp.convolution_filter_path",
					kind:        settingPath,
					strVal:      "",
					description: "Path to impulse response WAV file",
				},
				audioHeader("LUFS Normalization"),
				{
					label:       "Normalize",
					key:         "dsp.lufs_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable integrated loudness normalization (ITU-R BS.1770-4)",
				},
				{
					label:       "Target LUFS",
					key:         "dsp.lufs_target",
					kind:        settingFloat,
					floatVal:    -14.0,
					description: "Target integrated loudness — -14 (streaming), -16 (YouTube), -23 (broadcast EBU R128)",
				},
				{
					label:       "Max Gain (dB)",
					key:         "dsp.lufs_max_gain_db",
					kind:        settingFloat,
					floatVal:    12.0,
					description: "Maximum gain the normalizer will apply — prevents over-amplification of quiet content",
				},
			},

			// ── Stereo ──────────────────────────────────────────────────────────
			// Crossfeed (headphone psychoacoustics) and Mid/Side (stereo image).
			tabStereo: {
				audioHeader("Crossfeed"),
				{
					label:       "Enable",
					key:         "dsp.crossfeed_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable headphone crossfeed to reduce ear fatigue on hard-panned recordings",
				},
				{
					label:       "Auto-detect",
					key:         "dsp.crossfeed_auto",
					kind:        settingBool,
					boolVal:     false,
					description: "Automatically enable crossfeed when headphones are detected",
				},
				{
					label:       "Feed Level",
					key:         "dsp.crossfeed_feed_level",
					kind:        settingFloat,
					floatVal:    0.45,
					description: "Crossfeed blend amount — 0.0 = none, 0.45 = natural, 0.9 = maximum",
				},
				{
					label:       "Cutoff (Hz)",
					key:         "dsp.crossfeed_cutoff_hz",
					kind:        settingInt,
					intVal:      700,
					minVal:      300,
					maxVal:      700,
					description: "Crossfeed lowpass cutoff — higher = more natural, lower = stronger front-center effect",
				},
				audioHeader("Mid/Side"),
				{
					label:       "Enable M/S",
					key:         "dsp.ms_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable Mid/Side processing for independent control of center and stereo content",
				},
				{
					label:       "Width",
					key:         "dsp.ms_width",
					kind:        settingFloat,
					floatVal:    1.0,
					description: "Stereo width — 0.0 = mono, 1.0 = unchanged, 2.0 = maximum width",
				},
				{
					label:       "Mid Gain",
					key:         "dsp.ms_mid_gain",
					kind:        settingFloat,
					floatVal:    1.0,
					description: "Gain on center (mid) channel — reduces lead vocals and bass at < 1.0",
				},
				{
					label:       "Side Gain",
					key:         "dsp.ms_side_gain",
					kind:        settingFloat,
					floatVal:    1.0,
					description: "Gain on stereo difference (side) channel — controls ambience and stereo width",
				},
			},

			// ── Format ──────────────────────────────────────────────────────────
			// DSD conversion and dither — final output format stages.
			tabFormat: {
				audioHeader("DSD"),
				{
					label:       "DSD→PCM",
					key:         "dsp.dsd_to_pcm_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Convert DSD bitstream input to PCM for DSP processing",
				},
				{
					label:       "Output Mode",
					key:         "dsp.output_mode",
					kind:        settingChoice,
					choiceVals:  []string{"pcm", "dsd", "dsd_to_pcm"},
					choiceIdx:   0,
					description: "Output encoding format — pcm (standard), dsd (native bitstream), dsd_to_pcm",
				},
				{
					label:       "DSD Rate",
					key:         "dsp.dsd_output_rate",
					kind:        settingChoice,
					choiceVals:  []string{"88200", "176400", "352800", "705600"},
					choiceIdx:   2,
					description: "DSD to PCM output rate in Hz — higher = more bandwidth preserved",
				},
				audioHeader("Dither"),
				{
					label:       "Enable",
					key:         "dsp.dither_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable TPDF dither — adds shaped noise to reduce quantization distortion",
				},
				{
					label:       "Auto-detect",
					key:         "dsp.dither_auto",
					kind:        settingBool,
					boolVal:     false,
					description: "Auto-enable dither when output is ALSA at 16-bit",
				},
				{
					label:       "Bit Depth",
					key:         "dsp.dither_bit_depth",
					kind:        settingChoice,
					choiceVals:  []string{"8", "16", "20", "24", "32"},
					choiceIdx:   1,
					description: "Output bit depth — dither noise floor is calibrated to this depth",
				},
				{
					label:       "Noise Shaping",
					key:         "dsp.dither_noise_shaping",
					kind:        settingChoice,
					choiceVals:  []string{"none", "lipshitz", "fweighted", "modified_e_weighted", "improved_e_weighted", "shibata", "low_shibata", "high_shibata", "gesemann"},
					choiceIdx:   0,
					description: "Noise shaping algorithm — pushes quantization noise to less audible frequencies",
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
				items := m.settingItems[m.tab]
				m.selectedIdx = firstSelectableIdx(items)
			}
		case "right", "l":
			if m.tab < audioTab(len(audioTabNames)-1) {
				m.tab++
				items := m.settingItems[m.tab]
				m.selectedIdx = firstSelectableIdx(items)
			}
		case "up", "k":
			items := m.settingItems[m.tab]
			next := m.selectedIdx - 1
			for next >= 0 && items[next].kind == settingInfo {
				next--
			}
			if next >= 0 {
				m.selectedIdx = next
			}
		case "down", "j":
			items := m.settingItems[m.tab]
			next := m.selectedIdx + 1
			for next < len(items) && items[next].kind == settingInfo {
				next++
			}
			if next < len(items) {
				m.selectedIdx = next
			}
		case "enter":
			item := m.getCurrentItem()
			if item == nil || item.kind == settingInfo {
				return m, nil
			}
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
		case "+", "=":
			item := m.getCurrentItem()
			if item != nil && item.kind != settingInfo {
				item.adjust(+1)
				return m, settingChangedCmd(item)
			}
		case "-", "_":
			item := m.getCurrentItem()
			if item != nil && item.kind != settingInfo {
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

	rightW := m.width - 24
	if rightW < 30 {
		rightW = 30
	}

	normalStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	valStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())
	sectionStyle := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Bold(true)

	var itemLines []string
	lineIdxOfSelected := -1

	for i, item := range items {
		if item.kind == settingInfo {
			itemLines = append(itemLines, "")
			itemLines = append(itemLines, sectionStyle.Render("  "+item.label))
			continue
		}

		if i == m.selectedIdx {
			lineIdxOfSelected = len(itemLines)
		}

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

	if m.editing && lineIdxOfSelected >= 0 {
		item := m.getCurrentItem()
		if item != nil && item.kind == settingPath {
			itemLines[lineIdxOfSelected] = normalStyle.Render("  "+item.label+" ") + m.editInput.View()
		}
	}

	itemsPanel := lipgloss.NewStyle().
		Width(rightW).
		PaddingLeft(2).
		Render(strings.Join(itemLines, "\n"))

	// Description of the currently selected item shown as a help line.
	descLine := ""
	if item := m.getCurrentItem(); item != nil && item.description != "" {
		desc := item.description
		maxDescW := m.width - 4
		if maxDescW > 0 && len(desc) > maxDescW {
			desc = desc[:maxDescW-1] + "…"
		}
		descLine = lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			PaddingLeft(2).
			Render(desc) + "\n"
	}

	footer := hintBar("←→ tabs", "↑↓ navigate", "enter toggle", "+/- adjust", "esc back")

	return tea.NewView(header + "\n\n" + tabBar + "\n\n" + itemsPanel + "\n\n" + descLine + footer + "\n")
}
