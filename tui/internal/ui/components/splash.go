package components

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
	SplashStateReveal SplashState = iota
	SplashStateHold
	SplashStateColorCycle
	SplashStateWordmark
	SplashStateDone
)

type Splash struct {
	frame      int
	state      SplashState
	charIndex  int
	wordmark   string
	width      int
	height     int
	done       bool
	onComplete func()
	dotColors  []string // Current colors for each dot
	dotOrder   []int    // Order in which dots transition to final color
	seed       int64    // Random seed for consistent animation
}

func NewSplash(width, height int) *Splash {
	seed := time.Now().UnixNano()

	s := &Splash{
		frame:    0,
		state:    SplashStateReveal,
		wordmark: "STUI",
		width:    width,
		height:   height,
		seed:     seed,
	}
	s.initDotColors()
	return s
}

func NewSplashWithCallback(width, height int, onComplete func()) *Splash {
	seed := time.Now().UnixNano()

	s := &Splash{
		frame:      0,
		state:      SplashStateReveal,
		wordmark:   "STUI",
		width:      width,
		height:     height,
		onComplete: onComplete,
		seed:       seed,
	}
	s.initDotColors()
	return s
}

// initDotColors initializes the color state for each dot.
func (s *Splash) initDotColors() {
	r := rand.New(rand.NewSource(s.seed))

	// Final pattern (triangle / play-button shape).
	finalPattern := [][]bool{
		{false, false, false, true, false, false, false}, // row 1: tip
		{false, false, true, true, true, false, false},   // row 2
		{false, true, true, true, true, true, false},     // row 3
		{true, true, true, true, true, true, true},       // row 4: widest
		{true, true, true, true, true, true, true},       // row 5: widest
		{true, true, true, true, true, true, true},       // row 6
		{false, true, true, true, true, true, false},     // row 7
		{false, false, true, true, true, false, false},   // row 8
		{false, false, false, true, false, false, false}, // row 9: bottom tip
	}

	dotCount := 0
	for _, row := range finalPattern {
		for _, b := range row {
			if b {
				dotCount++
			}
		}
	}

	colors := []string{
		"#FF6B6B", "#4ECDC4", "#45B7D1", "#96CEB4",
		"#FFEAA7", "#DDA0DD", "#98D8C8", "#F7DC6F",
		"#BB8FCE", "#85C1E9", "#F8B500", "#00CED1",
	}

	s.dotColors = make([]string, dotCount)
	s.dotOrder = make([]int, dotCount)

	order := r.Perm(dotCount)
	for i := 0; i < dotCount; i++ {
		s.dotColors[i] = colors[r.Intn(len(colors))]
		s.dotOrder[i] = order[i]
	}

	r.Shuffle(len(s.dotOrder), func(i, j int) {
		s.dotOrder[i], s.dotOrder[j] = s.dotOrder[j], s.dotOrder[i]
	})
}

type splashTickMsg struct{}

func SplashTickCmd() tea.Cmd {
	return tea.Tick(100*time.Millisecond, func(time.Time) tea.Msg {
		return splashTickMsg{}
	})
}

func (s *Splash) Init() tea.Cmd {
	return SplashTickCmd()
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
		return nil, SplashTickCmd()
	case tea.WindowSizeMsg:
		s.width = msg.Width
		s.height = msg.Height
		return msg, nil
	}
	return msg, nil
}

func (s *Splash) advance() {
	s.frame++

	switch s.state {
	case SplashStateReveal:
		if s.frame >= 12 {
			s.state = SplashStateHold
			s.frame = 0
		}
	case SplashStateHold:
		if s.frame >= 6 {
			s.state = SplashStateColorCycle
			s.frame = 0
		}
	case SplashStateColorCycle:
		// Gradually transition dots to tyrian purple over 24 frames.
		if s.frame >= 24 {
			s.state = SplashStateWordmark
			s.frame = 0
			s.charIndex = 0
			for i := range s.dotColors {
				s.dotColors[i] = "#9B5DE5" // tyrian purple
			}
		} else {
			progress := float64(s.frame) / 24.0
			targetCount := int(float64(len(s.dotColors)) * progress)
			for i := 0; i < targetCount && i < len(s.dotColors); i++ {
				dotIdx := s.dotOrder[i]
				s.dotColors[dotIdx] = "#9B5DE5"
			}
		}
	case SplashStateWordmark:
		if s.charIndex < len(s.wordmark) {
			s.charIndex++
		}
		if s.charIndex >= len(s.wordmark) && s.frame >= 20 {
			s.done = true
		}
	}
}

func (s *Splash) View() tea.View {
	if s.done {
		return tea.NewView("")
	}

	bg := lipgloss.NewStyle().Background(theme.T.Bg())

	lines := s.buildBrailleLogo()

	if s.state >= SplashStateWordmark && s.charIndex > 0 {
		wordmark := s.wordmark[:s.charIndex]
		wordmarkStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
		wordmarkLine := wordmarkStyle.Render("  " + wordmark)
		lines = append(lines, wordmarkLine)
	}

	centerY := (s.height - len(lines)) / 2
	if centerY < 0 {
		centerY = 0
	}

	var sb strings.Builder
	for i := 0; i < centerY; i++ {
		sb.WriteString(bg.Render(strings.Repeat(" ", s.width)) + "\n")
	}

	for _, line := range lines {
		padding := (s.width - len(stripANSI(line))) / 2
		if padding < 0 {
			padding = 0
		}
		sb.WriteString(bg.Render(strings.Repeat(" ", padding)))
		sb.WriteString(line)
		sb.WriteString("\n")
	}

	v := tea.NewView(sb.String())
	v.AltScreen = true
	return v
}

func (s *Splash) buildBrailleLogo() []string {
	// Play-button triangle pattern (9 rows, 7 cols).
	pattern := [][]int{
		{0, 0, 0, 1, 0, 0, 0}, // row 0: tip at col 3
		{0, 0, 0, 1, 0, 0, 0}, // row 1: tip
		{0, 0, 1, 1, 1, 0, 0}, // row 2: 3 dots at cols 2,3,4
		{0, 1, 1, 1, 1, 1, 0}, // row 3: 5 dots at cols 1-5
		{1, 1, 1, 1, 1, 1, 1}, // row 4: 7 dots (widest)
		{1, 1, 1, 1, 1, 1, 1}, // row 5: 7 dots (widest)
		{0, 1, 1, 1, 1, 1, 0}, // row 6: 5 dots
		{0, 0, 1, 1, 1, 0, 0}, // row 7: 3 dots
		{0, 0, 0, 1, 0, 0, 0}, // row 8: tip at col 3
	}

	// Flatten pattern to get dot index mapping.
	dotIndexMap := make(map[int]int) // pattern index -> color index
	idx := 0
	for row := 0; row < len(pattern); row++ {
		for col := 0; col < len(pattern[row]); col++ {
			if pattern[row][col] == 1 {
				dotIndexMap[row*7+col] = idx
				idx++
			}
		}
	}

	// Build output lines (scaled 2x horizontally for bigger display).
	var lines []string
	for row := 0; row < len(pattern); row++ {
		var line strings.Builder
		rowPattern := pattern[row]

		for col := 0; col < len(rowPattern); col++ {
			if rowPattern[col] == 1 {
				colorIdx := dotIndexMap[row*7+col]
				color := s.dotColors[colorIdx]
				style := lipgloss.NewStyle().Foreground(lipgloss.Color(color))
				line.WriteString(style.Render("⣿"))
				line.WriteString(style.Render("⣿"))
			} else {
				dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
				line.WriteString(dimStyle.Render("  "))
			}
		}

		lines = append(lines, line.String())
	}

	return lines
}

func (s *Splash) IsDone() bool {
	return s.done
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
