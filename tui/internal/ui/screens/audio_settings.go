package screens

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
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
	numTabs
)

var audioTabNames = []string{
	"Output",
	"Signal",
	"Correction",
	"Stereo",
	"Format",
}

// Built-in DSP profiles available in the UI
var dspProfiles = []string{
	// Music profiles
	"Music: Default",
	"Music: Jazz",
	"Music: Classical",
	"Music: Rock",
	"Music: Electronic",
	"Music: Pop",
	"Music: Hip-Hop",
	"Music: Acoustic",
	// Movie profiles
	"Movies: Default",
	"Movies: Action",
	"Movies: Drama",
	"Movies: Comedy",
	"Movies: Horror",
	"Movies: Sci-Fi",
	"Movies: Animation",
	// Other
	"Night Mode",
	"Podcast",
}

// profileIDs maps display names to backend IDs
var profileIDs = map[string]string{
	"Music: Default":    "music_default",
	"Music: Jazz":       "music_jazz",
	"Music: Classical":  "music_classical",
	"Music: Rock":       "music_rock",
	"Music: Electronic": "music_electronic",
	"Music: Pop":        "music_pop",
	"Music: Hip-Hop":    "music_hiphop",
	"Music: Acoustic":   "music_acoustic",
	"Movies: Default":   "movies_default",
	"Movies: Action":    "movies_action",
	"Movies: Drama":     "movies_drama",
	"Movies: Comedy":    "movies_comedy",
	"Movies: Horror":    "movies_horror",
	"Movies: Sci-Fi":    "movies_scifi",
	"Movies: Animation": "movies_animation",
	"Night Mode":        "night_mode",
	"Podcast":           "podcast",
}

type AudioSettingsModel struct {
	Dims
	tab            audioTab
	selectedIdx    int
	editing        bool
	editInput      textinput.Model
	settingItems   map[audioTab][]*settingItem
	currentProfile string
	customProfiles []string // loaded from runtime via IPC
	profileLoaded  bool
	client         *ipc.Client
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

// Helper to find setting item index by key
func findSettingIdx(items []*settingItem, key string) int {
	for i, item := range items {
		if item.key == key {
			return i
		}
	}
	return -1
}

// stripProfilePrefix removes a "Saved: " or "Loaded: " prefix from a profile
// display name to recover the bare built-in profile name for lookups.
func stripProfilePrefix(name string) string {
	if s := strings.TrimPrefix(name, "Saved: "); s != name {
		return s
	}
	return strings.TrimPrefix(name, "Loaded: ")
}

// Helper to sync profile between currentProfile string and dropdown
func (m *AudioSettingsModel) syncProfileDropdown() {
	currentForLookup := stripProfilePrefix(m.currentProfile)
	for i, p := range dspProfiles {
		if currentForLookup == p {
			if idx := findSettingIdx(m.settingItems[tabOutput], "dsp.profile"); idx >= 0 {
				m.settingItems[tabOutput][idx].choiceIdx = i
			}
			return
		}
	}
	// Custom profile not in built-in list - leave dropdown as-is (will show "—")
	// Note: m.currentProfile contains the custom profile name for display purposes
}

func NewAudioSettingsModel(client *ipc.Client) AudioSettingsModel {
	// Derive backend IDs from the canonical dspProfiles slice so the two
	// never diverge — any profile missing from profileIDs gets an empty ID.
	profileChoiceVals := make([]string, len(dspProfiles))
	for i, name := range dspProfiles {
		profileChoiceVals[i] = profileIDs[name]
	}

	m := AudioSettingsModel{
		tab:            tabOutput,
		currentProfile: dspProfiles[0],
		customProfiles: []string{},
		profileLoaded:  false,
		client:         client,
		settingItems: map[audioTab][]*settingItem{

			// ── Output ──────────────────────────────────────────────────────────
			// Hardware sink, sample rate, and backend-specific parameters.
			tabOutput: {
				{
					label:         "Profile",
					key:           "dsp.profile",
					kind:          settingChoice,
					choiceVals:    profileChoiceVals,
					choiceDisplay: dspProfiles,
					choiceIdx:     0,
					description:   "DSP profile preset — applies optimized settings for different content types",
				},
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
					label:       "Output Mode",
					key:         "dsp.output_mode",
					kind:        settingChoice,
					choiceVals:  []string{"pcm", "dsd", "dsd_to_pcm"},
					choiceIdx:   0,
					description: "Output format: PCM, native DSD, or DSD → PCM",
				},
				{
					label:       "PipeWire Role",
					key:         "dsp.pipewire_role",
					kind:        settingChoice,
					choiceVals:  []string{"Music", "Production"},
					choiceIdx:   0,
					description: "PipeWire stream role — Production bypasses WirePlumber resampling",
				},
				{
					label:       "DSD → PCM",
					key:         "dsp.dsd_to_pcm_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Convert DSD input to PCM (required for most DACs)",
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
				audioHeader("DSD Native"),
				{
					label:       "DSD Mode",
					key:         "dsp.dsd_mode",
					kind:        settingChoice,
					choiceVals:  []string{"off", "dsd64", "dsd128", "dsd256", "dsd512"},
					choiceIdx:   0,
					description: "Native DSD output — off (PCM), dsd64 (2.8MHz), dsd128 (5.6MHz), dsd256 (11.2MHz), dsd512 (22.5MHz)",
				},
				{
					label:       "DSD→PCM",
					key:         "dsp.dsd_to_pcm_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Convert DSD bitstream input to PCM for DSP processing",
				},
				{
					label:       "DSD Rate",
					key:         "dsp.dsd_output_rate",
					kind:        settingChoice,
					choiceVals:  []string{"88200", "176400", "352800", "705600"},
					choiceIdx:   2,
					description: "DSD to PCM output rate in Hz — higher = more bandwidth preserved",
				},
				audioHeader("Output Format"),
				{
					label:       "Output Mode",
					key:         "dsp.output_mode",
					kind:        settingChoice,
					choiceVals:  []string{"pcm", "dsd", "dsd_to_pcm"},
					choiceIdx:   0,
					description: "Output encoding format — pcm (standard), dsd (native bitstream), dsd_to_pcm",
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
					label:         "Noise Shaping",
					key:           "dsp.dither_noise_shaping",
					kind:          settingChoice,
					choiceVals:    []string{"none", "lipshitz", "fweighted", "modified_e_weighted", "improved_e_weighted", "shibata", "low_shibata", "high_shibata", "gesemann", "saw"},
					choiceDisplay: []string{"None", "Lipshitz", "F-Weighted", "Modified E-Weighted", "Improved E-Weighted", "Shibata", "Low Shibata", "High Shibata", "Gesemann", "SAW (experimental)"},
					choiceIdx:     0,
					description:   "Noise shaping algorithm — pushes quantization noise to less audible frequencies",
				},
			},
		},
	}
	return m
}

func (m AudioSettingsModel) Init() tea.Cmd {
	if m.client != nil {
		return func() tea.Msg {
			m.client.ListDspProfiles()
			return nil
		}
	}
	return nil
}

func (m AudioSettingsModel) SetClient(client *ipc.Client) AudioSettingsModel {
	m.client = client
	return m
}

func (m AudioSettingsModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	if m.editing {
		switch msg := msg.(type) {
		case tea.KeyPressMsg:
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
		m.setWindowSize(msg)

	case ipc.DspProfilesListedMsg:
		m.customProfiles = msg.Profiles
		m.profileLoaded = true
		m.syncProfileDropdown()

	case ipc.DspProfileLoadedMsg:
		// Profile loaded successfully - Name contains the profile name
		m.currentProfile = "Loaded: " + msg.Name
		m.syncProfileDropdown()

	case tea.KeyPressMsg:
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
		case "s":
			// Profile save - persist current settings as a custom profile
			if m.client != nil {
				bare := stripProfilePrefix(m.currentProfile)
				if strings.HasPrefix(m.currentProfile, "Saved: ") {
					// Already saved - no action needed
				} else if profileID, ok := profileIDs[bare]; ok {
					m.client.SaveDspProfile(profileID)
					m.currentProfile = "Saved: " + bare
				} else {
					// No matching profile - silently skip (custom profiles handled elsewhere)
				}
			}
			return m, nil
		case "L":
			// Shift+L to cycle profiles - load selected profile from runtime.
			// State (currentProfile + dropdown) is updated only in DspProfileLoadedMsg
			// so the UI reflects a profile that is actually loaded, not just requested.
			if m.client != nil {
				profileIdx := 0
				currentForLookup := stripProfilePrefix(m.currentProfile)
				for i, p := range dspProfiles {
					if currentForLookup == p {
						profileIdx = i
						break
					}
				}
				profileIdx = (profileIdx + 1) % len(dspProfiles)
				if profileID, ok := profileIDs[dspProfiles[profileIdx]]; ok {
					m.client.LoadDspProfile(profileID)
				}
			}
			return m, nil
		case "D":
			// Shift+D to clear profile - reset to first built-in
			m.currentProfile = "Music: Default"
			// Also reset the dropdown
			if idx := findSettingIdx(m.settingItems[tabOutput], "dsp.profile"); idx >= 0 {
				m.settingItems[tabOutput][idx].choiceIdx = 0
			}
			// Load the default profile on the runtime
			if m.client != nil {
				m.client.LoadDspProfile("music_default")
			}
			return m, nil
		case "r", "R":
			// 'r' or 'R' to refresh/load custom profiles from runtime
			if m.client != nil {
				m.client.ListDspProfiles()
			}
			// Note: m.profileLoaded is set in DspProfilesListedMsg handler
			return m, nil
		case "esc", "backspace":
			// Both pop back to Settings (the previous screen on the stack).
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

	// Profile footer - persistent across all tabs
	// Shows the current profile, custom profile count, and save/refresh shortcuts
	profileStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())
	profileLabel := profileStyle.Render("Profile: ")
	profileValue := profileStyle.Render(m.currentProfile)

	// Show custom profile count if any loaded
	customHint := ""
	if len(m.customProfiles) > 0 {
		customHint = lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render(fmt.Sprintf(" (+%d custom)", len(m.customProfiles)))
	}
	saveHint := ""
	if m.client != nil {
		saveHint = lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render(" [S]ave | [R]efresh")
	}

	profileFooter := profileLabel + profileValue + customHint + saveHint

	footer := hintBar("←→ tabs", "↑↓ navigate", "enter toggle", "+/- adjust", "L cycle | D reset", "esc back")

	return tea.NewView(header + "\n\n" + tabBar + "\n\n" + itemsPanel + "\n\n" + descLine + profileFooter + "\n" + footer + "\n")
}
