package components

// splash.go — Opening splash: a braille-dot play-button diamond that matches
// the assets/stui_logo_braille_play.svg shape. Dots dance through random
// rainbow colors, then gradually lock to tyrian purple (#9B5DE5) before the
// "STUI" wordmark types in below.

import (
	"math/rand"
	"strings"
	"time"

	"charm.land/bubbles/v2/progress"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type SplashState int

const (
	SplashStateReveal     SplashState = iota // dots dance through random colors
	SplashStateColorCycle                    // gradually lock dots to tyrian purple
	SplashStateWordmark                      // type STUI wordmark
	SplashStateDone
)

// Play-button diamond pattern, extracted from
// assets/stui_logo_braille_play.svg (cx columns 282..390 step 18,
// cy rows 152..296 step 18 — the 9×7 diamond of tyrian-purple circles).
var splashPattern = [9][7]int{
	{0, 0, 0, 1, 0, 0, 0}, // row 0 (y=152): tip
	{0, 0, 1, 1, 1, 0, 0}, // row 1 (y=170)
	{0, 1, 1, 1, 1, 1, 0}, // row 2 (y=188)
	{1, 1, 1, 1, 1, 1, 1}, // row 3 (y=206) widest
	{1, 1, 1, 1, 1, 1, 1}, // row 4 (y=224) widest
	{1, 1, 1, 1, 1, 1, 1}, // row 5 (y=242) widest
	{0, 1, 1, 1, 1, 1, 0}, // row 6 (y=260)
	{0, 0, 1, 1, 1, 0, 0}, // row 7 (y=278)
	{0, 0, 0, 1, 0, 0, 0}, // row 8 (y=296) tip
}

const (
	tyrianPurple = "#9B5DE5" // target color, matches the SVG's rgb(155,93,229)

	splashTickMs       = 80 // animation tick
	splashRevealFrames = 18 // ~1.4s of dancing colors
	splashCycleFrames  = 28 // ~2.2s of lock-in to tyrian purple
	splashWordmarkHold = 22 // ~1.8s after the wordmark finishes
)

// danceColors are the palette that dots randomly flip through during the
// dancing (Reveal) phase before locking to tyrian purple.
var danceColors = []string{
	"#FF6B6B", "#4ECDC4", "#45B7D1", "#96CEB4",
	"#FFEAA7", "#DDA0DD", "#98D8C8", "#F7DC6F",
	"#BB8FCE", "#85C1E9", "#F8B500", "#00CED1",
	"#9B5DE5", // include purple in the mix so transitions feel natural
}

type Splash struct {
	state      SplashState
	frame      int
	charIndex  int
	wordmark   string
	width      int
	height     int
	done       bool
	onComplete func()

	rng       *rand.Rand
	dotColors []string // current color for each lit dot (len = number of 1s)
	locked    []bool   // true once the dot has settled on tyrian purple
	lockOrder []int    // order in which dots lock during ColorCycle

	// Progress bar at the bottom of the splash. The splash dismisses
	// when the boot sequence has progressed enough to drop the user
	// into a useful UI: runtime ready, plugins loaded, the wordmark
	// has typed in, AND either every expected tab grid has arrived OR
	// a grid grace period has elapsed (so an offline boot doesn't
	// hang waiting for live data forever). A hard wall-clock cap
	// (splashHardTimeout) is the last-resort fallback for a hung
	// runtime.
	progress      progress.Model
	startedAt     time.Time
	wordmarkTyped bool // true once the wordmark has fully typed in

	gotRuntime bool // RuntimeReadyMsg observed
	gotPlugins bool // PluginListMsg observed

	// Tabs we're waiting for first GridUpdateMsg from. Populated at
	// construction (default: movies/series/music). When all keys have
	// `true` values OR gridDeadline passes, we consider grid loading
	// "good enough" to dismiss.
	expectedTabs map[string]bool
	gridDeadline time.Time // set when plugins arrive

	// finalized flips true once the dismissal preconditions are met.
	// When that happens we kick the bar to 100% (via pendingCmd) and
	// keep the splash up until the spring finishes animating, so the
	// user always sees the bar visually complete before we hand off
	// to the main UI. Without this the bar dismisses at whatever
	// percent the last milestone reached (often 60-80%) which reads
	// as a half-finished load.
	finalized  bool
	pendingCmd tea.Cmd
}

func NewSplash(width, height int) *Splash {
	return newSplash(width, height, nil)
}

func NewSplashWithCallback(width, height int, onComplete func()) *Splash {
	return newSplash(width, height, onComplete)
}

func newSplash(width, height int, onComplete func()) *Splash {
	s := &Splash{
		state:      SplashStateReveal,
		wordmark:   "STUI",
		width:      width,
		height:     height,
		onComplete: onComplete,
		rng:        rand.New(rand.NewSource(time.Now().UnixNano())),
		startedAt:  time.Now(),
		progress: progress.New(
			progress.WithWidth(splashProgressWidth),
			progress.WithoutPercentage(),
			// Continuous gradient look (no visible half-block split
			// at the fill edge): full-block fill char + a
			// purple-to-pink blend that matches the tyrian-purple
			// dot color the logo settles on. The empty char is a
			// dim block so the unfilled portion reads as a track
			// rather than empty space.
			progress.WithFillCharacters('█', '░'),
			progress.WithColors(
				lipgloss.Color(tyrianPurple),
				lipgloss.Color("#FF7AC6"),
			),
		),
		expectedTabs: map[string]bool{
			"movies": false,
			"series": false,
			"music":  false,
		},
	}
	// Start with a very slow spring so the indeterminate fill creeps
	// up toward splashIndeterminateCap over the runtime-startup wait
	// (which can be 15-20s while WASM plugins compile). Frequency 0.4
	// gives a settling time of ~12s — long enough to fill that window
	// with continuous motion. MarkRuntimeReady swaps in a faster
	// spring (frequency 4, ~1.25s settle) once real milestones start
	// landing so the bar reads as deliberate, snappy progress.
	s.progress.SetSpringOptions(0.4, 1.0)
	s.initDots()
	return s
}

const (
	// splashHardTimeout is a last-resort wall-clock cap on the splash.
	// The runtime can take 15-20s on a cold boot — it loads WASM
	// plugins synchronously before its IPC loop accepts the
	// handshake ping (see runtime.log "IPC loop ready"). 60s gives a
	// generous margin for cold first-time launches with many plugins
	// to compile/cache while still bailing out if something is
	// genuinely stuck.
	splashHardTimeout = 60 * time.Second

	// splashGridGracePeriod is how long we wait for grid-update
	// messages AFTER plugins finish loading. Once plugins are up the
	// runtime starts hydrating grids from cache (instant) or from the
	// network (a few seconds). If we're offline or have empty caches,
	// no grid_update will ever arrive — this grace period keeps the
	// splash from hanging in that case. 4s is enough for a cached
	// boot to complete cleanly while still feeling snappy when the
	// network is slow.
	splashGridGracePeriod = 4 * time.Second

	// splashProgressWidth is the rendered width of the progress bar
	// (cells). Smaller than the logo (14 cols) would feel cramped;
	// larger than ~40 looks bloated on small terms.
	splashProgressWidth = 24

	// splashIndeterminateCap is the target for the bar's "slow
	// creep" before any real milestone lands. The runtime often
	// takes 15+ seconds to respond to its first ping while it
	// compiles WASM plugins, so without this the bar would sit
	// dead-empty for the entire startup wait. Capped well below
	// splashPctRuntime (0.20) so MarkRuntimeReady visibly snaps
	// the bar forward rather than starting from "already past".
	splashIndeterminateCap = 0.15
)

// Progress bar percentages for each milestone. Five beats so the bar
// fills steadily through the boot sequence rather than jumping in two
// large steps.
const (
	splashPctRuntime = 0.20
	splashPctPlugins = 0.40
	splashPctGrid1   = 0.60
	splashPctGrid2   = 0.80
	splashPctGrid3   = 1.00
)

// MarkRuntimeReady fires when the runtime IPC handshake completes.
// Switches the bar from "slow indeterminate creep" to "snappy
// milestone fill" — see SetSpringOptions in newSplash for the
// rationale.
func (s *Splash) MarkRuntimeReady() tea.Cmd {
	if s.gotRuntime {
		return nil
	}
	s.gotRuntime = true
	s.progress.SetSpringOptions(4, 1.0)
	return s.progress.SetPercent(splashPctRuntime)
}

// MarkPluginsLoaded fires when plugin discovery completes. This also
// arms the grid grace period: from this moment we'll wait at most
// splashGridGracePeriod for grid_update messages before considering
// the boot "done enough" (offline / empty-cache safety net).
func (s *Splash) MarkPluginsLoaded() tea.Cmd {
	if s.gotPlugins {
		return nil
	}
	s.gotPlugins = true
	s.gridDeadline = time.Now().Add(splashGridGracePeriod)
	return s.progress.SetPercent(splashPctPlugins)
}

// MarkGridReady fires when a grid_update arrives for one of the
// expected tabs. The percentage advances based on how many distinct
// tabs we've now seen. Tabs we're not tracking (e.g. "books" later)
// are ignored. Idempotent per tab.
func (s *Splash) MarkGridReady(tab string) tea.Cmd {
	if seen, expected := s.expectedTabs[tab]; !expected || seen {
		return nil
	}
	s.expectedTabs[tab] = true

	count := 0
	for _, seen := range s.expectedTabs {
		if seen {
			count++
		}
	}
	switch count {
	case 1:
		return s.progress.SetPercent(splashPctGrid1)
	case 2:
		return s.progress.SetPercent(splashPctGrid2)
	default:
		return s.progress.SetPercent(splashPctGrid3)
	}
}

func (s *Splash) initDots() {
	dotCount := 0
	for _, row := range splashPattern {
		for _, v := range row {
			if v == 1 {
				dotCount++
			}
		}
	}

	s.dotColors = make([]string, dotCount)
	s.locked = make([]bool, dotCount)
	s.lockOrder = make([]int, dotCount)
	for i := range s.dotColors {
		s.dotColors[i] = danceColors[s.rng.Intn(len(danceColors))]
		s.lockOrder[i] = i
	}
	s.rng.Shuffle(len(s.lockOrder), func(i, j int) {
		s.lockOrder[i], s.lockOrder[j] = s.lockOrder[j], s.lockOrder[i]
	})
}

type splashTickMsg struct{}

func SplashTickCmd() tea.Cmd {
	return tea.Tick(splashTickMs*time.Millisecond, func(time.Time) tea.Msg {
		return splashTickMsg{}
	})
}

// Init kicks off both the splash's own animation tick AND the
// indeterminate progress-bar creep. The creep targets
// splashIndeterminateCap and, paired with the slow spring set in
// newSplash, fills the bar gradually over the runtime-startup wait
// so the user sees continuous motion while WASM plugins compile.
func (s *Splash) Init() tea.Cmd {
	return tea.Batch(SplashTickCmd(), s.progress.SetPercent(splashIndeterminateCap))
}

func (s *Splash) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	if s.done {
		return msg, nil
	}
	switch msg := msg.(type) {
	case splashTickMsg:
		s.advance()
		if s.done {
			if s.onComplete != nil {
				s.onComplete()
			}
			return nil, nil
		}
		// advance() may have queued a SetPercent(1.0) cmd to snap
		// the bar to its final state — flush it here alongside the
		// next tick so the spring picks it up.
		cmds := []tea.Cmd{SplashTickCmd()}
		if s.pendingCmd != nil {
			cmds = append(cmds, s.pendingCmd)
			s.pendingCmd = nil
		}
		return nil, tea.Batch(cmds...)
	case tea.WindowSizeMsg:
		s.width = msg.Width
		s.height = msg.Height
		return msg, nil
	case progress.FrameMsg:
		// Forward the progress-bar's own animation frames so the
		// fill smoothly slides between SetPercent calls.
		var cmd tea.Cmd
		s.progress, cmd = s.progress.Update(msg)
		return nil, cmd
	}
	return msg, nil
}

// advance runs one animation frame.
func (s *Splash) advance() {
	s.frame++

	// During Reveal and early ColorCycle, re-shuffle the colors on every dot
	// that isn't locked — this is the "dancing" rainbow effect.
	if s.state == SplashStateReveal || s.state == SplashStateColorCycle {
		for i := range s.dotColors {
			if s.locked[i] {
				continue
			}
			s.dotColors[i] = danceColors[s.rng.Intn(len(danceColors))]
		}
	}

	switch s.state {
	case SplashStateReveal:
		if s.frame >= splashRevealFrames {
			s.state = SplashStateColorCycle
			s.frame = 0
		}
	case SplashStateColorCycle:
		// Progressively lock dots to tyrian purple based on frame progress.
		progress := float64(s.frame) / float64(splashCycleFrames)
		if progress > 1 {
			progress = 1
		}
		target := int(float64(len(s.dotColors)) * progress)
		for i := 0; i < target && i < len(s.lockOrder); i++ {
			idx := s.lockOrder[i]
			s.locked[idx] = true
			s.dotColors[idx] = tyrianPurple
		}
		if s.frame >= splashCycleFrames {
			// Force every remaining dot to purple.
			for i := range s.dotColors {
				s.locked[i] = true
				s.dotColors[i] = tyrianPurple
			}
			s.state = SplashStateWordmark
			s.frame = 0
			s.charIndex = 0
		}
	case SplashStateWordmark:
		if s.charIndex < len(s.wordmark) && s.frame%3 == 0 {
			s.charIndex++
		}
		// Wordmark "typed in" means the letters are all visible AND we
		// held briefly so the user can register them. Beyond this we
		// stay in SplashStateWordmark indefinitely — the wordmark is
		// the natural resting state while we wait for milestones.
		if s.charIndex >= len(s.wordmark) && s.frame >= splashWordmarkHold {
			s.wordmarkTyped = true
		}
	}

	// Dismissal is a two-stage process:
	//
	//   1. Determine "ready to wrap up" — the boot sequence has gone
	//      far enough that we no longer need to keep the user on the
	//      splash. Required: wordmark typed, runtime ready, plugins
	//      loaded. Then either every expected tab grid arrived OR
	//      the grid grace period elapsed (offline / empty-cache
	//      fallback). The grace period only starts ticking from
	//      MarkPluginsLoaded so an offline first boot still gets a
	//      few seconds of network attempt.
	//
	//   2. Once "ready to wrap up", flip s.finalized=true and snap
	//      the bar's target to 100% (via pendingCmd, picked up by
	//      Update on the next tick). The splash then waits for the
	//      spring to finish animating before setting s.done=true,
	//      so the user always sees the bar visually complete.
	//
	// A hard wall-clock cap (splashHardTimeout) is the last-resort
	// fallback for a stuck IPC handshake — it bypasses the
	// fill-to-100 wait so a genuinely hung boot doesn't strand the
	// user forever.
	if time.Since(s.startedAt) > splashHardTimeout {
		s.done = true
		return
	}
	if !s.finalized {
		if !s.wordmarkTyped || !s.gotRuntime || !s.gotPlugins {
			return
		}
		allTabsIn := true
		for _, seen := range s.expectedTabs {
			if !seen {
				allTabsIn = false
				break
			}
		}
		gracePassed := !s.gridDeadline.IsZero() && time.Now().After(s.gridDeadline)
		if !allTabsIn && !gracePassed {
			return
		}
		s.finalized = true
		if s.progress.Percent() < 1.0 {
			s.pendingCmd = s.progress.SetPercent(1.0)
		}
	}
	if !s.progress.IsAnimating() {
		s.done = true
	}
}

// View renders the splash screen centered in the available area.
func (s *Splash) View() tea.View {
	if s.done {
		return tea.NewView("")
	}

	bg := lipgloss.NewStyle().Background(theme.T.Bg())

	logoLines := s.buildBrailleLogo()

	// Reserve 2 blank lines + wordmark line below the logo.
	totalH := len(logoLines) + 2
	var wordmarkLine string
	if s.state >= SplashStateWordmark && s.charIndex > 0 {
		wmStyle := lipgloss.NewStyle().
			Foreground(lipgloss.Color(tyrianPurple)).
			Bold(true)
		letters := s.wordmark[:s.charIndex]
		// Pad out to the logo's visual width (7 cols × 2 = 14).
		spaced := strings.Join(strings.Split(letters, ""), " ")
		wordmarkLine = wmStyle.Render(spaced)
	}

	centerY := (s.height - totalH) / 2
	if centerY < 0 {
		centerY = 0
	}

	logoVisualW := 14 // 7 cols × 2 braille chars per col

	var sb strings.Builder
	for i := 0; i < centerY; i++ {
		sb.WriteString(bg.Render(strings.Repeat(" ", s.width)) + "\n")
	}
	for _, line := range logoLines {
		pad := (s.width - logoVisualW) / 2
		if pad < 0 {
			pad = 0
		}
		sb.WriteString(bg.Render(strings.Repeat(" ", pad)))
		sb.WriteString(line)
		sb.WriteString("\n")
	}
	// Blank spacer line under the logo.
	sb.WriteString(bg.Render(strings.Repeat(" ", s.width)) + "\n")
	// Wordmark line (centered).
	if wordmarkLine != "" {
		wmW := stripANSIWidth(wordmarkLine)
		pad := (s.width - wmW) / 2
		if pad < 0 {
			pad = 0
		}
		sb.WriteString(bg.Render(strings.Repeat(" ", pad)))
		sb.WriteString(wordmarkLine)
		sb.WriteString("\n")
	} else {
		sb.WriteString(bg.Render(strings.Repeat(" ", s.width)) + "\n")
	}

	// Progress bar — one blank-line spacer then the bar centered
	// under the wordmark. The bar fills as IPC milestones are
	// reached (runtime ready / plugins loaded / first grid),
	// reaching 100% when the animation naturally completes.
	sb.WriteString(bg.Render(strings.Repeat(" ", s.width)) + "\n")
	progBar := s.progress.View()
	progW := stripANSIWidth(progBar)
	pad := (s.width - progW) / 2
	if pad < 0 {
		pad = 0
	}
	sb.WriteString(bg.Render(strings.Repeat(" ", pad)))
	sb.WriteString(progBar)
	sb.WriteString("\n")

	v := tea.NewView(sb.String())
	v.AltScreen = true
	return v
}

// buildBrailleLogo renders the diamond pattern with each "on" position as a
// 2-character-wide block of ⣿ braille dots, and "off" positions as 2 spaces.
func (s *Splash) buildBrailleLogo() []string {
	lines := make([]string, 0, len(splashPattern))
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	dotIdx := 0
	for _, row := range splashPattern {
		var line strings.Builder
		for _, v := range row {
			if v == 1 {
				color := s.dotColors[dotIdx]
				style := lipgloss.NewStyle().Foreground(lipgloss.Color(color))
				line.WriteString(style.Render("⣿⣿"))
				dotIdx++
			} else {
				line.WriteString(dimStyle.Render("  "))
			}
		}
		lines = append(lines, line.String())
	}
	return lines
}

func (s *Splash) IsDone() bool { return s.done }

// stripANSIWidth returns the visible character width of s, ignoring ANSI
// escape sequences.
func stripANSIWidth(s string) int {
	n := 0
	inEscape := false
	for _, r := range s {
		if r == '\x1b' {
			inEscape = true
			continue
		}
		if inEscape {
			if r == 'm' {
				inEscape = false
			}
			continue
		}
		n++
	}
	return n
}

func stripANSI(s string) string {
	var result strings.Builder
	inEscape := false
	for _, r := range s {
		if r == '\x1b' {
			inEscape = true
			continue
		}
		if inEscape {
			if r == 'm' {
				inEscape = false
			}
			continue
		}
		result.WriteRune(r)
	}
	return result.String()
}

// RenderBrailleLogo renders a tiny static logo for inline use elsewhere.
func RenderBrailleLogo(width int, style lipgloss.Style) string {
	lines := []string{
		" ⣠⣾⣦⣀ ",
		" ⠻⣿⡿⠃ ",
		"  ⠈⠁  ",
	}
	var sb strings.Builder
	for _, line := range lines {
		padding := (width - len(stripANSI(line))) / 2
		if padding < 0 {
			padding = 0
		}
		sb.WriteString(strings.Repeat(" ", padding))
		sb.WriteString(style.Render(line))
		sb.WriteString("\n")
	}
	return sb.String()
}
