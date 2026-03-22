// internal/ui/screens/common.go
package screens

import (
	"strings"

	"charm.land/lipgloss/v2"
	"github.com/stui/stui/pkg/theme"
)

// hintBar renders a standardised footer hint line.
// Each argument is a pre-formatted "key action" token, e.g. "enter play", "esc back".
// Tokens are joined with 3-space separators and wrapped in dim styling.
// A fresh lipgloss.Style is created per call, consistent with the theme architecture
// (theme.go: "Styles are NOT stored as globals").
func hintBar(hints ...string) string {
	s := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return "  " + s.Render(strings.Join(hints, "   "))
}
