package components

// player.go — "Now Playing" HUD and "Buffering" overlay.
//
// BUFFERING:
//
//  ╭──────────────────────────────────────────────────────────────────────╮
//  │  ⏳  Pre-roll buffer                      3.1 MB/s   ETA 8s         │
//  │  ████████████████░░░░░░░░░░░░░░░░░░░░░░   42%  target 30s video     │
//  ╰──────────────────────────────────────────────────────────────────────╯
//
// NOW PLAYING (full HUD):
//
//  ╭──────────────────────────────────────────────────────────────────────╮
//  │  ▶  Interstellar (2014)                        1:12:32 / 2:49:00    │
//  │  ████████████████████░░░░░░░░░░░░░░░░░░░░░░    43%  cache 94%       │
//  │  1080p HTTP  stream 1/3  │  Sub: English +0.0s  │  Audio: English   │
//  │  space pause  z/Z sub±  a audio  s streams  m mute  ] vol+  q stop  │
//  ╰──────────────────────────────────────────────────────────────────────╯

import (
	"fmt"
	"math"
	"strings"

	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/theme"
)

// NowPlayingState tracks live playback or buffering status.
// Populated from ipc.PlayerStartedMsg / ipc.PlayerProgressMsg events.
type NowPlayingState struct {
	// ── Core playback ─────────────────────────────────────────────────────
	Title        string
	Path         string
	Position     float64
	Duration     float64
	Paused       bool
	CachePercent float64
	HasDuration  bool

	// ── Quality / stream info ─────────────────────────────────────────────
	Quality         string // "1080p", "720p", …
	Protocol        string // "HTTP", "Torrent", …
	ActiveCandidate int    // 0-indexed
	CandidateCount  int    // total available streams

	// ── Track info ────────────────────────────────────────────────────────
	AudioLabel    string  // "English", "Japanese", …
	SubLabel      string  // "English", "Off", …
	SubtitleDelay float64 // seconds
	AudioDelay    float64 // seconds
	Volume        float64 // 0–130
	Muted         bool

	// ── Buffering (pre-roll or stall-guard) ───────────────────────────────
	Buffering       bool
	BufferReason    string  // "initial" | "stall_guard"
	BufferFill      float64 // 0–100
	BufferSpeedMbps float64
	BufferPreRoll   float64 // target buffer in seconds
	BufferEta       float64 // seconds until ready
}

func NewNowPlaying(msg ipc.PlayerStartedMsg) *NowPlayingState {
	return &NowPlayingState{
		Title:       msg.Title,
		Path:        msg.Path,
		Duration:    msg.Duration,
		HasDuration: msg.Duration > 0,
		Volume:      100,
		SubLabel:    "Off",
	}
}

func (n *NowPlayingState) Update(msg ipc.PlayerProgressMsg) {
	n.Position = msg.Position
	if msg.Duration > 0 {
		n.Duration = msg.Duration
		n.HasDuration = true
	}
	n.Paused = msg.Paused
	n.CachePercent = msg.CachePercent
	n.Buffering = false

	// Extended fields (populated when runtime sends them)
	if msg.Volume > 0 {
		n.Volume = msg.Volume
	}
	n.Muted = msg.Muted
	n.SubtitleDelay = msg.SubtitleDelay
	n.AudioDelay = msg.AudioDelay
	if msg.AudioLabel != "" {
		n.AudioLabel = msg.AudioLabel
	}
	if msg.SubLabel != "" {
		n.SubLabel = msg.SubLabel
	}
	if msg.Quality != "" {
		n.Quality = msg.Quality
	}
	if msg.Protocol != "" {
		n.Protocol = msg.Protocol
	}
	n.ActiveCandidate = msg.ActiveCandidate
	n.CandidateCount = msg.CandidateCount
}

// ── Renderer ──────────────────────────────────────────────────────────────────

func RenderNowPlaying(np *NowPlayingState, w int) string {
	if np == nil || w < 20 {
		return ""
	}
	if np.Buffering {
		return renderBuffering(np, w)
	}
	return renderPlaying(np, w)
}

func renderPlaying(np *NowPlayingState, w int) string {
	inner := w - 4
	if inner < 10 {
		inner = 10
	}

	// Styles
	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Width(w - 2)

	titleStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Bold(true)

	dimStyle := lipgloss.NewStyle().
		Foreground(theme.T.TextDim())

	accentStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent())

	warnStyle := lipgloss.NewStyle().
		Foreground(lipgloss.Color("#e5c07b"))

	// ── Row 1: title + position ───────────────────────────────────────────
	icon := "▶"
	if np.Paused {
		icon = "⏸"
	}
	posStr := formatDuration(np.Position)
	durStr := ""
	if np.HasDuration && np.Duration > 0 {
		durStr = " / " + formatDuration(np.Duration)
	}
	timeStr := posStr + durStr

	titleTrunc := Truncate(np.Title, inner-len(timeStr)-4)
	titleRow := icon + "  " + titleStyle.Render(titleTrunc)
	row1 := padBetween(titleRow, dimStyle.Render(timeStr), inner)

	// ── Row 2: progress bar + cache ───────────────────────────────────────
	fraction := 0.0
	if np.HasDuration && np.Duration > 0 {
		fraction = math.Min(np.Position/np.Duration, 1.0)
	}
	barW := inner - 16
	if barW < 4 {
		barW = 4
	}
	bar := renderProgressBar(fraction, barW)
	pct := fmt.Sprintf("%3.0f%%", fraction*100)
	cacheStr := fmt.Sprintf("cache %3.0f%%", np.CachePercent)
	row2 := bar + " " + accentStyle.Render(pct) + "  " + dimStyle.Render(cacheStr)

	// ── Row 3: stream info + track info ───────────────────────────────────
	qualParts := []string{}
	if np.Quality != "" {
		qualParts = append(qualParts, np.Quality)
	}
	if np.Protocol != "" {
		qualParts = append(qualParts, np.Protocol)
	}
	qualStr := strings.Join(qualParts, " ")
	if qualStr == "" {
		qualStr = "—"
	}

	streamStr := ""
	if np.CandidateCount > 1 {
		streamStr = fmt.Sprintf("  stream %d/%d", np.ActiveCandidate+1, np.CandidateCount)
	}

	subStr := "Sub: " + np.SubLabel
	if np.SubtitleDelay != 0 {
		subStr += fmt.Sprintf(" %+.1fs", np.SubtitleDelay)
	}

	audioStr := "Audio: " + np.AudioLabel
	if np.AudioDelay != 0 {
		audioStr += fmt.Sprintf(" %+.1fs", np.AudioDelay)
	}

	volStr := fmt.Sprintf("Vol: %.0f%%", np.Volume)
	if np.Muted {
		volStr = warnStyle.Render("Vol: muted")
	}

	sep := dimStyle.Render("  │  ")
	row3 := dimStyle.Render(qualStr+streamStr) + sep +
		dimStyle.Render(subStr) + sep +
		dimStyle.Render(audioStr) + sep +
		dimStyle.Render(volStr)

	// ── Row 4: keybind hints ──────────────────────────────────────────────
	hints := []string{
		"space pause",
		"←→ seek",
		"z/Z sub±",
		"A pick audio",
		"a cycle audio",
		"s streams",
		"m mute",
		"] vol+",
		"q stop",
	}
	row4 := dimStyle.Render(strings.Join(hints, "  "))

	content := strings.Join([]string{row1, row2, row3, row4}, "\n")
	return boxStyle.Render(content) + "\n"
}

func renderBuffering(np *NowPlayingState, w int) string {
	inner := w - 4
	if inner < 10 {
		inner = 10
	}

	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(lipgloss.Color("#e5c07b")).
		Width(w - 2)

	accentStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#e5c07b"))
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	reason := "Pre-roll buffer"
	if np.BufferReason == "stall_guard" {
		reason = "Buffering (stall)"
	}

	speedStr := fmt.Sprintf("%.1f MB/s", np.BufferSpeedMbps)
	etaStr := ""
	if np.BufferEta > 0 {
		etaStr = fmt.Sprintf("  ETA %ds", int(np.BufferEta))
	}
	row1 := padBetween(
		"⏳  "+accentStyle.Render(reason),
		dimStyle.Render(speedStr+etaStr),
		inner,
	)

	barW := inner - 20
	if barW < 4 {
		barW = 4
	}
	bar := renderProgressBar(np.BufferFill/100.0, barW)
	pct := fmt.Sprintf("%3.0f%%", np.BufferFill)
	target := fmt.Sprintf("target %ds video", int(np.BufferPreRoll))
	row2 := bar + " " + accentStyle.Render(pct) + "  " + dimStyle.Render(target)

	return boxStyle.Render(row1+"\n"+row2) + "\n"
}

// ── Helpers ───────────────────────────────────────────────────────────────────

func renderProgressBar(fraction float64, width int) string {
	if width <= 0 {
		return ""
	}
	filled := int(math.Round(fraction * float64(width)))
	if filled > width {
		filled = width
	}
	fill := strings.Repeat("█", filled)
	empty := strings.Repeat("░", width-filled)
	return lipgloss.NewStyle().Foreground(theme.T.Accent()).Render(fill) +
		lipgloss.NewStyle().Foreground(theme.T.Border()).Render(empty)
}

func formatDuration(secs float64) string {
	if secs < 0 {
		secs = 0
	}
	total := int(secs)
	h := total / 3600
	m := (total % 3600) / 60
	s := total % 60
	if h > 0 {
		return fmt.Sprintf("%d:%02d:%02d", h, m, s)
	}
	return fmt.Sprintf("%d:%02d", m, s)
}

func padBetween(left, right string, width int) string {
	pad := width - lipgloss.Width(left) - lipgloss.Width(right)
	if pad < 1 {
		pad = 1
	}
	return left + strings.Repeat(" ", pad) + right
}

// ── MPD / audio HUD ───────────────────────────────────────────────────────────

// MpdNowPlayingState tracks live audio playback state from MPD.
// Populated from ipc.MpdStatusMsg push events.
type MpdNowPlayingState struct {
	State       string // "play" | "pause" | "stop"
	Title       string
	Artist      string
	Album       string
	Elapsed     float64
	Duration    float64
	Volume      uint32
	Bitrate     uint32 // kbps
	AudioFormat string // raw MPD format "192000:24:2"
	ReplayGain  string // off|track|album|auto
	Crossfade   uint32
	Consume     bool
	Random      bool
	QueueLength uint32
}

// DspState tracks live DSP pipeline state.
// Populated from ipc.DspStatusMsg responses.
type DspState struct {
	Enabled            bool
	OutputSampleRate   uint32
	ResampleEnabled    bool
	DsdToPcmEnabled    bool
	ConvolutionEnabled bool
	ConvolutionBypass  bool
	Active             bool
}

func (d *DspState) Update(msg ipc.DspStatusMsg) {
	d.Enabled = msg.Enabled
	d.OutputSampleRate = msg.OutputSampleRate
	d.ResampleEnabled = msg.ResampleEnabled
	d.DsdToPcmEnabled = msg.DsdToPcmEnabled
	d.ConvolutionEnabled = msg.ConvolutionEnabled
	d.ConvolutionBypass = msg.ConvolutionBypass
	d.Active = msg.Active
}

func (m *MpdNowPlayingState) Update(msg ipc.MpdStatusMsg) {
	m.State = msg.State
	m.Title = msg.SongTitle
	m.Artist = msg.SongArtist
	m.Album = msg.SongAlbum
	m.Elapsed = msg.Elapsed
	m.Duration = msg.Duration
	m.Volume = msg.Volume
	m.Bitrate = msg.Bitrate
	m.AudioFormat = msg.AudioFormat
	m.ReplayGain = msg.ReplayGain
	m.Crossfade = msg.Crossfade
	m.Consume = msg.Consume
	m.Random = msg.Random
	m.QueueLength = msg.QueueLength
}

// formatAudioFormat converts MPD's "samplerate:bits:channels" to a
// human-readable audiophile badge like "FLAC 192kHz / 24-bit / Stereo".
// MPD doesn't tell us the codec here, so we show the numbers.
func formatAudioFormat(raw string, bitrateKbps uint32) string {
	if raw == "" && bitrateKbps == 0 {
		return ""
	}
	parts := strings.SplitN(raw, ":", 3)
	var out []string
	if len(parts) >= 1 && parts[0] != "" && parts[0] != "*" {
		hz := parts[0]
		// Format as kHz if >= 1000
		var n int
		if _, err := fmt.Sscanf(parts[0], "%d", &n); err == nil && n >= 1000 {
			if n%1000 == 0 {
				hz = fmt.Sprintf("%dkHz", n/1000)
			} else {
				hz = fmt.Sprintf("%.1fkHz", float64(n)/1000)
			}
		}
		out = append(out, hz)
	}
	if len(parts) >= 2 && parts[1] != "" && parts[1] != "*" {
		out = append(out, parts[1]+"-bit")
	}
	if len(parts) >= 3 {
		switch parts[2] {
		case "1":
			out = append(out, "Mono")
		case "2":
			out = append(out, "Stereo")
		default:
			if parts[2] != "" && parts[2] != "*" {
				out = append(out, parts[2]+"ch")
			}
		}
	}
	badge := strings.Join(out, " / ")
	if bitrateKbps > 0 {
		if badge != "" {
			badge += fmt.Sprintf("  %dkbps", bitrateKbps)
		} else {
			badge = fmt.Sprintf("%dkbps", bitrateKbps)
		}
	}
	return badge
}

// RenderMpdNowPlaying renders the audiophile-friendly MPD playback HUD.
//
//	╭─────────────────────────────────────────────────────────────────────╮
//	│  ▶  Bohemian Rhapsody                              4:32 / 5:55     │
//	│     Queen · A Night at the Opera                                   │
//	│  ████████████████████░░░░░░░░░░░░░░░░   76%   192kHz / 24-bit     │
//	│  Vol: 85%  RG: album  Crossfade: off  Queue: 12  Random: on       │
//	│  space pause  n next  p prev  r rg  o outputs  +/- vol  q stop    │
//	╰─────────────────────────────────────────────────────────────────────╯
func RenderMpdNowPlaying(m *MpdNowPlayingState, w int) string {
	if m == nil || w < 20 {
		return ""
	}

	inner := w - 4
	if inner < 10 {
		inner = 10
	}

	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Width(w - 2)

	titleStyle := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	artistStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	warnStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#e5c07b"))

	// ── Row 1: state icon + title + elapsed ──────────────────────────────
	icon := "▶"
	switch m.State {
	case "pause":
		icon = "⏸"
	case "stop":
		icon = "⏹"
	}

	posStr := formatDuration(m.Elapsed)
	durStr := ""
	if m.Duration > 0 {
		durStr = " / " + formatDuration(m.Duration)
	}
	timeStr := posStr + durStr

	titleTrunc := Truncate(m.Title, inner-len(timeStr)-4)
	if titleTrunc == "" {
		titleTrunc = "—"
	}
	row1 := padBetween(
		icon+"  "+titleStyle.Render(titleTrunc),
		dimStyle.Render(timeStr),
		inner,
	)

	// ── Row 2: artist · album ─────────────────────────────────────────────
	var artistLine string
	if m.Artist != "" || m.Album != "" {
		sep := ""
		if m.Artist != "" && m.Album != "" {
			sep = " · "
		}
		artistLine = "     " + artistStyle.Render(Truncate(m.Artist+sep+m.Album, inner-5))
	}

	// ── Row 3: progress bar + audio format ───────────────────────────────
	fraction := 0.0
	if m.Duration > 0 {
		fraction = m.Elapsed / m.Duration
		if fraction > 1 {
			fraction = 1
		}
	}
	audioFmt := formatAudioFormat(m.AudioFormat, m.Bitrate)
	barW := inner - len(audioFmt) - 8
	if barW < 4 {
		barW = 4
	}
	bar := renderProgressBar(fraction, barW)
	pct := fmt.Sprintf("%3.0f%%", fraction*100)
	row3 := bar + " " + accentStyle.Render(pct) + "  " + accentStyle.Render(audioFmt)

	// ── Row 4: playback settings ──────────────────────────────────────────
	volStr := fmt.Sprintf("Vol: %d%%", m.Volume)
	rgStr := "RG: " + m.ReplayGain
	xfStr := ""
	if m.Crossfade > 0 {
		xfStr = fmt.Sprintf("  XFade: %ds", m.Crossfade)
	}
	qStr := fmt.Sprintf("  Queue: %d", m.QueueLength)
	randStr := ""
	if m.Random {
		randStr = "  Shuffle"
	}
	consumeStr := ""
	if m.Consume {
		consumeStr = warnStyle.Render("  Consume")
	}
	row4 := dimStyle.Render(volStr+"  "+rgStr+xfStr+qStr+randStr) + consumeStr

	// ── Row 5: keybind hints ──────────────────────────────────────────────
	hints := []string{
		"space pause",
		"n next",
		"p prev",
		"r rg-mode",
		"o outputs",
		"+/- vol",
		"S shuffle",
		"q stop",
	}
	row5 := dimStyle.Render(strings.Join(hints, "  "))

	rows := []string{row1}
	if artistLine != "" {
		rows = append(rows, artistLine)
	}
	rows = append(rows, row3, row4, row5)

	return boxStyle.Render(strings.Join(rows, "\n")) + "\n"
}

// RenderDspStatus renders the DSP pipeline status panel.
//
//	╭─────────────────────────────────────────────────────────────────────╮
//	│  🎛 DSP Pipeline                                    [ON]          │
//	│     Out: 192kHz  ↑4×  DSD→PCM: on  Convolution: on  Bypass: off  │
//	│     d toggle  c convolve  b bypass  r reset                         │
//	╰─────────────────────────────────────────────────────────────────────╯
func RenderDspStatus(d *DspState, w int) string {
	if d == nil || w < 30 {
		return ""
	}

	inner := w - 4
	if inner < 20 {
		inner = 20
	}

	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Width(w - 2)

	titleStyle := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	onStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#98c379"))
	offStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#e06c75"))

	// ── Row 1: title + status ───────────────────────────────────────────
	icon := "🎛"
	statusStr := "OFF"
	statusRendered := offStyle.Render(statusStr)
	if d.Enabled {
		statusStr = "ON"
		statusRendered = onStyle.Render(statusStr)
	}
	row1 := padBetween(
		icon+"  DSP Pipeline",
		statusRendered,
		inner,
	)

	// ── Row 2: details ───────────────────────────────────────────────────
	rateStr := fmt.Sprintf("%dkHz", d.OutputSampleRate/1000)
	upStr := "↑off"
	if d.ResampleEnabled {
		upStr = "↑4×"
	}
	dsdStr := "DSD→PCM: off"
	if d.DsdToPcmEnabled {
		dsdStr = onStyle.Render("DSD→PCM: on")
	} else {
		dsdStr = dimStyle.Render("DSD→PCM: off")
	}
	convStr := "Conv: off"
	if d.ConvolutionEnabled {
		convStr = onStyle.Render("Conv: on")
	} else {
		convStr = dimStyle.Render("Conv: off")
	}
	bpStr := "Bypass: off"
	if d.ConvolutionBypass {
		bpStr = offStyle.Render("Bypass: on")
	} else {
		bpStr = dimStyle.Render("Bypass: off")
	}
	row2 := titleStyle.Render("  " + rateStr + "  " + upStr + "  " + dsdStr + "  " + convStr + "  " + bpStr)

	// ── Row 3: keybind hints ──────────────────────────────────────────────
	hints := []string{
		"d toggle DSP",
		"c convolve",
		"b bypass",
		"r reset",
	}
	row3 := dimStyle.Render(strings.Join(hints, "  "))

	return boxStyle.Render(row1+"\n"+row2+"\n"+row3) + "\n"
}

// RenderSyncOverlay renders a right-aligned pill showing the current
// subtitle or audio delay after the user adjusts it.
//
// Example:  Subtitle delay  +300ms   z +  Z –  X reset
func RenderSyncOverlay(isAudio bool, delaySecs float64, w int) string {
	label := "Subtitle delay"
	hints := "  z +  Z –  X reset"
	if isAudio {
		label = "Audio delay"
		hints = "  ctrl+] +  ctrl+[ –"
	}

	delayMs := int(math.Round(delaySecs * 1000))
	var delayStr string
	switch {
	case delayMs == 0:
		delayStr = "0ms"
	case delayMs > 0:
		delayStr = fmt.Sprintf("+%dms", delayMs)
	default:
		delayStr = fmt.Sprintf("%dms", delayMs)
	}

	bg := lipgloss.NewStyle().Foreground(theme.T.Bg()).Background(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.Bg()).Background(theme.T.Accent())
	pill := bg.Render(" "+label+"  "+delayStr) + dim.Render(hints+" ")

	return lipgloss.NewStyle().Width(w).Align(lipgloss.Right).Render(pill)
}

// RenderSkipPrompt renders the "Skip [Intro/Credits]" overlay prompt.
func RenderSkipPrompt(label string, seekTo float64, w int) string {
	_ = seekTo // seekTo is used by the caller for the actual seek command
	style := lipgloss.NewStyle().
		Foreground(theme.T.Bg()).
		Background(theme.T.Accent()).
		Padding(0, 2).
		Bold(true)
	hint := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	line := style.Render("▶ Skip "+label) + "  " + hint.Render("[i]")
	return lipgloss.NewStyle().Width(w).Align(lipgloss.Right).Render(line)
}

// Truncate shortens s to at most maxLen display cells, appending "…" when
// truncated. It delegates to the bidi package so RTL text is truncated from
// the correct end and display width is measured correctly.
func Truncate(s string, maxLen int) string {
	return bidi.Truncate(s, maxLen)
}
