package components

// splash.go — Opening splash: a braille-dot play-button diamond that matches
// the assets/stui_logo_braille_play.svg shape. Dots dance through random
// rainbow colors, then gradually lock to tyrian purple (#9B5DE5) before the
// "STUI" wordmark types in below.

import (
	"math/rand"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type SplashState int

const (
	SplashStateReveal      SplashState = iota // dots dance through random colors
	SplashStateColorCycle                     // gradually lock dots to tyrian purple
	SplashStateWordmark                       // type STUI wordmark
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
	}
	s.initDots()
	return s
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

func (s *Splash) Init() tea.Cmd { return SplashTickCmd() }

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
		return nil, SplashTickCmd()
	case tea.WindowSizeMsg:
		s.width = msg.Width
		s.height = msg.Height
		return msg, nil
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
		if s.charIndex >= len(s.wordmark) && s.frame >= splashWordmarkHold {
			s.done = true
		}
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
