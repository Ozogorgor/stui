package components

import (
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
)

type TransitionType int

const (
	TransitionFade TransitionType = iota
	TransitionSlideUp
	TransitionSlideDown
	TransitionSlideLeft
	TransitionSlideRight
)

type TransitionState int

const (
	TransitionEntering TransitionState = iota
	TransitionVisible
	TransitionLeaving
)

type ScreenTransition struct {
	currentView string
	nextView    string
	view        string
	transition  TransitionType
	state       TransitionState
	frame       int
	maxFrames   int
	onComplete  func()
}

type ScreenTransitionOption func(*ScreenTransition)

func WithTransition(t TransitionType) ScreenTransitionOption {
	return func(s *ScreenTransition) {
		s.transition = t
	}
}

func WithOnComplete(fn func()) ScreenTransitionOption {
	return func(s *ScreenTransition) {
		s.onComplete = fn
	}
}

func NewScreenTransition(currentView string, nextView string, opts ...ScreenTransitionOption) *ScreenTransition {
	st := &ScreenTransition{
		currentView: currentView,
		nextView:    nextView,
		view:        currentView,
		transition:  TransitionFade,
		state:       TransitionEntering,
		frame:       0,
		maxFrames:   10,
	}
	for _, opt := range opts {
		opt(st)
	}
	return st
}

func (st *ScreenTransition) Init() tea.Cmd {
	return tea.Tick(50*time.Millisecond, func(time.Time) tea.Msg {
		return TransitionTickMsg{}
	})
}

type TransitionTickMsg struct{}

func (st *ScreenTransition) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	switch msg.(type) {
	case TransitionTickMsg:
		st.frame++
		switch st.state {
		case TransitionEntering:
			if st.frame >= st.maxFrames {
				st.state = TransitionVisible
				st.view = st.nextView
				st.frame = 0
				if st.onComplete != nil {
					st.onComplete()
				}
				return nil, nil
			}
			st.updateTransition()
		case TransitionLeaving:
			if st.frame >= st.maxFrames {
				st.state = TransitionVisible
				st.view = st.nextView
				return nil, nil
			}
			st.updateTransition()
		}
		return nil, tea.Tick(50*time.Millisecond, func(time.Time) tea.Msg {
			return TransitionTickMsg{}
		})
	}
	return msg, nil
}

func (st *ScreenTransition) updateTransition() {
	switch st.transition {
	case TransitionFade:
		st.view = st.crossFade()
	case TransitionSlideUp:
		st.view = st.slideUp()
	case TransitionSlideDown:
		st.view = st.slideDown()
	case TransitionSlideLeft:
		st.view = st.slideLeft()
	case TransitionSlideRight:
		st.view = st.slideRight()
	}
}

func (st *ScreenTransition) crossFade() string {
	if st.state == TransitionEntering {
		progress := float64(st.frame) / float64(st.maxFrames)
		if progress > 0.5 {
			return st.nextView
		}
		return st.currentView
	}
	return st.nextView
}

func (st *ScreenTransition) slideUp() string {
	if st.state == TransitionEntering {
		lines := strings.Split(st.nextView, "\n")
		skipLines := int(float64(len(lines)) * (1 - float64(st.frame)/float64(st.maxFrames)))
		if skipLines < len(lines) {
			return strings.Join(lines[skipLines:], "\n")
		}
	}
	return st.nextView
}

func (st *ScreenTransition) slideDown() string {
	if st.state == TransitionEntering {
		lines := strings.Split(st.nextView, "\n")
		skipLines := int(float64(len(lines)) * float64(st.frame) / float64(st.maxFrames))
		if skipLines < len(lines) {
			return strings.Join(lines[skipLines:], "\n")
		}
	}
	return st.nextView
}

func (st *ScreenTransition) slideLeft() string {
	if st.state == TransitionEntering {
		lines := strings.Split(st.nextView, "\n")
		progress := float64(st.frame) / float64(st.maxFrames)
		for i := range lines {
			skipChars := int(float64(len(lines[i])) * (1 - progress))
			if skipChars < len(lines[i]) {
				lines[i] = lines[i][skipChars:]
			}
		}
		return strings.Join(lines, "\n")
	}
	return st.nextView
}

func (st *ScreenTransition) slideRight() string {
	if st.state == TransitionEntering {
		lines := strings.Split(st.nextView, "\n")
		progress := float64(st.frame) / float64(st.maxFrames)
		for i := range lines {
			skipChars := int(float64(len(lines[i])) * progress)
			if skipChars < len(lines[i]) {
				lines[i] = lines[i][skipChars:]
			}
		}
		return strings.Join(lines, "\n")
	}
	return st.nextView
}

func (st *ScreenTransition) View() string {
	return st.view
}

func (st *ScreenTransition) IsComplete() bool {
	return st.state == TransitionVisible && st.frame == 0
}
