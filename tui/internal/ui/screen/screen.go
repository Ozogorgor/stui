package screen

// screen.go — Screen interface and navigation primitives.

import tea "github.com/charmbracelet/bubbletea"

// Screen is the contract every stui screen must satisfy.
type Screen interface {
	Init() tea.Cmd
	Update(msg tea.Msg) (Screen, tea.Cmd)
	View() string
}

// TransitionMsg is sent as a Cmd to tell RootModel to swap screens.
type TransitionMsg struct {
	Next     Screen
	PushBack bool // if true, current screen is pushed onto the history stack
}

// PopMsg is sent by a screen to signal it should be popped from the stack.
type PopMsg struct{}

// TransitionCmd returns a Cmd that replaces the active screen with next.
// If pushBack is true the current screen is saved so ESC can return to it.
func TransitionCmd(next Screen, pushBack bool) tea.Cmd {
	return func() tea.Msg {
		return TransitionMsg{Next: next, PushBack: pushBack}
	}
}
