package components

import (
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
	SplashStatePulse
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
}

func NewSplash(width, height int) *Splash {
	return &Splash{
		frame:    0,
		state:    SplashStateReveal,
		wordmark: "STUI",
		width:    width,
		height:   height,
	}
}

func NewSplashWithCallback(width, height int, onComplete func()) *Splash {
	return &Splash{
		frame:      0,
		state:      SplashStateReveal,
		wordmark:   "STUI",
		width:      width,
		height:     height,
		onComplete: onComplete,
	}
}

type splashTickMsg struct{}

func SplashTickCmd() tea.Cmd {
	return tea.Tick(80*time.Millisecond, func(time.Time) tea.Msg {
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
		if s.frame >= 18 {
			s.state = SplashStateHold
			s.frame = 0
		}
	case SplashStateHold:
		if s.frame >= 8 {
			s.state = SplashStatePulse
			s.frame = 0
		}
	case SplashStatePulse:
		if s.frame >= 8 {
			s.state = SplashStateWordmark
			s.frame = 0
			s.charIndex = 0
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

	accent := lipgloss.NewStyle().Foreground(theme.T.Accent())
	bg := lipgloss.NewStyle().Background(theme.T.Bg())

	lines := s.buildBrailleLogo(accent)

	if s.state >= SplashStateWordmark && s.charIndex > 0 {
		wordmark := s.wordmark[:s.charIndex]
		wordmarkLine := accent.Render("  " + wordmark)
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

func (s *Splash) buildBrailleLogo(style lipgloss.Style) []string {
	var lines []string

	frames := [][]string{
		{"    ", "    ", "    "},
		{" ⣀  ", " ⠉⠁ ", "    "},
		{" ⣠⡀ ", " ⠙⠁ ", "    "},
		{" ⣠⡀ ", " ⠈⠻⠋ ", "    "},
		{" ⢀⣴⣄ ", " ⠈⠻⠋ ", "    "},
		{" ⢀⣴⣄ ", " ⠙⠿⠛⠁ ", "    "},
		{" ⢀⣾⣆ ", " ⠙⢿⠟⠁ ", "    "},
		{" ⣠⣾⣦⡀", " ⠛⢿⠟⠁ ", "    "},
		{" ⣠⣾⣦⡀", " ⠻⣿⡿⠃ ", " ⠈  "},
		{" ⣠⣾⣦⡀", " ⠻⣿⡿⠃ ", " ⠈  "},
		{"     ", "     ", "     "},
		{" ⣠⣾⣦⡀", " ⠻⣿⡿⠃ ", " ⠈  "},
		{"     ", "     ", "     "},
		{" ⣠⣾⣦⡀", " ⠻⣿⡿⠃ ", " ⠈  "},
		{" ⣠⣾⣦⡀", " ⠻⣿⡿⠃ ", " ⠈  "},
	}

	frameIndex := s.frame
	if s.state == SplashStateHold {
		frameIndex = 8
	} else if s.state == SplashStatePulse {
		frameIndex = 10 + (s.frame % 3)
	} else if s.state == SplashStateWordmark {
		frameIndex = 14
	}

	if frameIndex >= len(frames) {
		frameIndex = len(frames) - 1
	}

	frame := frames[frameIndex]
	for _, line := range frame {
		lines = append(lines, style.Render(line))
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
