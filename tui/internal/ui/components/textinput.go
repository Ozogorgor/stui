package components

import (
	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type TextInputStyle int

const (
	TextInputStyleDefault TextInputStyle = iota
	TextInputStyleSearch
	TextInputStylePath
	TextInputStylePassword
)

type StyledTextInput struct {
	model textinput.Model
	style TextInputStyle
}

func NewStyledTextInput(style TextInputStyle, placeholder string) *StyledTextInput {
	ti := textinput.New()
	ti.Placeholder = placeholder

	switch style {
	case TextInputStyleSearch:
		ti.SetStyles(textinput.Styles{
			Blurred: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
				Prompt:      lipgloss.NewStyle().Foreground(theme.T.AccentAlt()),
			},
			Focused: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
				Prompt:      lipgloss.NewStyle().Foreground(theme.T.Accent()),
			},
			Cursor: textinput.CursorStyle{
				Color: lipgloss.Color("#7c3aed"),
				Blink: true,
			},
		})

	case TextInputStylePath:
		ti.SetStyles(textinput.Styles{
			Blurred: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextDim()),
				Prompt:      lipgloss.NewStyle().Foreground(theme.T.AccentAlt()),
			},
			Focused: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextDim()),
				Prompt:      lipgloss.NewStyle().Foreground(theme.T.Accent()),
			},
			Cursor: textinput.CursorStyle{
				Color: lipgloss.Color("#06b6d4"),
				Blink: true,
			},
		})

	case TextInputStylePassword:
		ti.EchoMode = textinput.EchoPassword
		ti.EchoCharacter = '•'
		ti.SetStyles(textinput.Styles{
			Blurred: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextDim()),
				Prompt:      lipgloss.NewStyle().Foreground(theme.T.Warn()),
			},
			Focused: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextDim()),
				Prompt:      lipgloss.NewStyle().Foreground(theme.T.Warn()),
			},
			Cursor: textinput.CursorStyle{
				Color: lipgloss.Color("#f59e0b"),
				Blink: true,
			},
		})

	default:
		ti.SetStyles(textinput.Styles{
			Blurred: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
			},
			Focused: textinput.StyleState{
				Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
				Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
			},
			Cursor: textinput.CursorStyle{
				Color: lipgloss.Color("#7c3aed"),
				Blink: true,
			},
		})
	}

	return &StyledTextInput{
		model: ti,
		style: style,
	}
}

func (s *StyledTextInput) Model() textinput.Model {
	return s.model
}

func (s *StyledTextInput) SetWidth(width int) {
	s.model.SetWidth(width)
}

func (s *StyledTextInput) SetCharLimit(limit int) {
	s.model.CharLimit = limit
}

func (s *StyledTextInput) SetValue(value string) {
	s.model.SetValue(value)
}

func (s *StyledTextInput) Value() string {
	return s.model.Value()
}

// Focus marks the input active and returns the cmd for cursor blink. Must
// be returned through tea.Cmd so the blink loop runs.
func (s *StyledTextInput) Focus() tea.Cmd {
	return s.model.Focus()
}

func (s *StyledTextInput) Blur() {
	s.model.Blur()
}

func (s *StyledTextInput) Focused() bool {
	return s.model.Focused()
}

// Update forwards the message to the inner textinput.Model. textinput's
// Update returns a new Model by value (value-receiver method) — we
// capture and store it back into `s.model` so state mutations stick.
func (s *StyledTextInput) Update(msg tea.Msg) tea.Cmd {
	var cmd tea.Cmd
	s.model, cmd = s.model.Update(msg)
	return cmd
}

// CursorEnd moves the cursor to the end of the current value.
func (s *StyledTextInput) CursorEnd() {
	s.model.CursorEnd()
}

func (s *StyledTextInput) View() string {
	return s.model.View()
}

type UndoableTextInput struct {
	model      textinput.Model
	history    []string
	historyPos int
	maxHistory int
}

func NewUndoableTextInput(maxHistory int) *UndoableTextInput {
	ti := textinput.New()
	ti.SetStyles(textinput.Styles{
		Blurred: textinput.StyleState{
			Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
			Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
		},
		Focused: textinput.StyleState{
			Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
			Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
		},
		Cursor: textinput.CursorStyle{
			Color: lipgloss.Color("#7c3aed"),
			Blink: true,
		},
	})

	return &UndoableTextInput{
		model:      ti,
		maxHistory: maxHistory,
	}
}

func (u *UndoableTextInput) SaveToHistory() {
	value := u.model.Value()
	if value == "" {
		return
	}
	if len(u.history) == 0 || u.history[len(u.history)-1] != value {
		u.history = append(u.history, value)
		if len(u.history) > u.maxHistory {
			u.history = u.history[1:]
		}
	}
	u.historyPos = len(u.history)
}

func (u *UndoableTextInput) Undo() bool {
	if u.historyPos > 0 {
		u.historyPos--
		u.model.SetValue(u.history[u.historyPos])
		return true
	}
	return false
}

func (u *UndoableTextInput) Redo() bool {
	if u.historyPos < len(u.history)-1 {
		u.historyPos++
		u.model.SetValue(u.history[u.historyPos])
		return true
	}
	return false
}

func (u *UndoableTextInput) Model() textinput.Model {
	return u.model
}

func (u *UndoableTextInput) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	return u.model.Update(msg)
}

func (u *UndoableTextInput) View() string {
	return u.model.View()
}

func (u *UndoableTextInput) SetWidth(width int) {
	u.model.SetWidth(width)
}

func (u *UndoableTextInput) SetValue(value string) {
	u.model.SetValue(value)
}

func (u *UndoableTextInput) Value() string {
	return u.model.Value()
}
