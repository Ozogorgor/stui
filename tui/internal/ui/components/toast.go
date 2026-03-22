package components

// toast.go — transient plugin notification overlay.
//
// Toasts appear bottom-right, above the status bar, and auto-dismiss
// after a short display period driven by a Bubble Tea tick command.

import (
	"time"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

const toastDuration = 4 * time.Second

// Toast holds a single active notification.
type Toast struct {
	message string
	isError bool
}

// ToastDismissMsg is sent by the auto-dismiss timer.
type ToastDismissMsg struct{}

// ShowToast creates a Toast and starts the dismiss timer.
func ShowToast(message string, isError bool) (Toast, tea.Cmd) {
	t := Toast{message: message, isError: isError}
	cmd := tea.Tick(toastDuration, func(_ time.Time) tea.Msg {
		return ToastDismissMsg{}
	})
	return t, cmd
}

// RenderToast renders the toast overlay at the bottom-right of the screen.
// Returns an empty string if there's no active toast.
func RenderToast(t *Toast, termWidth, termHeight int) string {
	if t == nil || t.message == "" {
		return ""
	}

	maxW := 48
	msg := Truncate(t.message, maxW-6)

	var style lipgloss.Style
	if t.isError {
		style = lipgloss.NewStyle().
			Background(theme.T.Red()).
			Foreground(lipgloss.Color("#ffffff")).
			Bold(true).
			Padding(0, 2).
			BorderStyle(lipgloss.RoundedBorder()).
			BorderForeground(lipgloss.Color("#ff6666")).
			BorderBackground(theme.T.Red())
	} else {
		style = lipgloss.NewStyle().
			Background(theme.T.Accent()).
			Foreground(lipgloss.Color("#ffffff")).
			Bold(true).
			Padding(0, 2).
			BorderStyle(lipgloss.RoundedBorder()).
			BorderForeground(theme.T.Neon()).
			BorderBackground(theme.T.Accent())
	}

	icon := "✦ "
	if t.isError {
		icon = "✖ "
	}

	rendered := style.Render(icon + msg)
	return rendered
}
