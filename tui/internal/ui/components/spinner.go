package components

import (
	"sync/atomic"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
)

var reducedMotionEnabled atomic.Bool

type Spinner struct {
	model         spinner.Model
	msg           string
	style         lipgloss.Style
	active        bool
	reducedMotion bool
}

func NewSpinner(msg string, style lipgloss.Style) *Spinner {
	s := spinner.New(
		spinner.WithSpinner(spinner.Dot),
		spinner.WithStyle(style),
	)
	return &Spinner{
		model:         s,
		msg:           msg,
		style:         style,
		active:        true,
		reducedMotion: reducedMotionEnabled.Load(),
	}
}

func (s *Spinner) Init() tea.Cmd {
	if !s.active {
		return nil
	}
	if s.reducedMotion || reducedMotionEnabled.Load() {
		return nil
	}
	return func() tea.Msg {
		return s.model.Tick()
	}
}

func (s *Spinner) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	if !s.active {
		return msg, nil
	}
	if s.reducedMotion || reducedMotionEnabled.Load() {
		return msg, nil
	}
	switch msg.(type) {
	case spinner.TickMsg:
		var cmd tea.Cmd
		s.model, cmd = s.model.Update(msg)
		return nil, cmd
	}
	return msg, nil
}

func (s *Spinner) View() string {
	if !s.active {
		return ""
	}
	if s.reducedMotion || reducedMotionEnabled.Load() {
		return "[...] " + s.msg
	}
	return s.model.View() + " " + s.msg
}

func (s *Spinner) Start() {
	s.active = true
}

func (s *Spinner) Stop() {
	s.active = false
}

func (s *Spinner) SetMessage(msg string) {
	s.msg = msg
}

func (s *Spinner) IsActive() bool {
	return s.active
}

func SetReducedMotion(enabled bool) {
	reducedMotionEnabled.Store(enabled)
}
