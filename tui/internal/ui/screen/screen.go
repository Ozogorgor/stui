package screen

// screen.go — Screen interface and navigation primitives.

import tea "charm.land/bubbletea/v2"

// Screen is the contract every stui screen must satisfy.
type Screen interface {
	Init() tea.Cmd
	Update(msg tea.Msg) (Screen, tea.Cmd)
	View() tea.View
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

// PopCmd returns a Cmd that sends a PopMsg, telling the root to pop the current screen.
func PopCmd() tea.Cmd {
	return func() tea.Msg { return PopMsg{} }
}

// OpenOverlayMsg tells RootModel to show a screen as a centered popup overlay
// without replacing or stacking the active screen.
type OpenOverlayMsg struct {
	Screen Screen
}

// CloseOverlayMsg tells RootModel to dismiss the current overlay.
type CloseOverlayMsg struct{}

// OpenOverlayCmd returns a Cmd that opens a screen as a popup overlay.
func OpenOverlayCmd(s Screen) tea.Cmd {
	return func() tea.Msg { return OpenOverlayMsg{Screen: s} }
}

// CloseOverlayCmd returns a Cmd that closes the current overlay.
func CloseOverlayCmd() tea.Cmd {
	return func() tea.Msg { return CloseOverlayMsg{} }
}
