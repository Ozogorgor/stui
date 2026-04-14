package components

// pathlist.go — A simple modal editor for a list of filesystem paths.
//
// Usage:
//
//	editor := components.NewPathListEditor("Extra music directories", paths)
//	// In Update: forward keys to editor.Update(key); when editor.Done() it
//	// returns the new []string via editor.Paths(). Render via editor.View().

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

// PathListEditor is an immutable value-type modal that lets the user
// add and remove paths. It owns a textinput for adding new entries.
//
// Keys (when not adding):
//
//	j / down   move cursor down
//	k / up     move cursor up
//	a / +      open the add-path input
//	d / x      remove the path under the cursor
//	enter      save and close (emits Done = true)
//	esc        cancel and close (emits Cancelled = true)
//
// Keys (when adding):
//
//	enter      append the typed path and close the input
//	esc        discard the typed path and close the input
type PathListEditor struct {
	Title  string
	paths  []string
	cursor int
	width  int

	adding bool
	input  textinput.Model
}

// NewPathListEditor creates an editor seeded with the given paths.
func NewPathListEditor(title string, paths []string) PathListEditor {
	pl := PathListEditor{
		Title: title,
		paths: append([]string(nil), paths...),
		width: 60,
	}
	return pl
}

// Paths returns the current list (a copy).
func (e PathListEditor) Paths() []string {
	out := make([]string, len(e.paths))
	copy(out, e.paths)
	return out
}

// SetWidth sets the rendering width (the modal box width).
func (e PathListEditor) SetWidth(w int) PathListEditor {
	e.width = w
	if e.input.Width() != 0 {
		e.input.SetWidth(w - 4)
	}
	return e
}

// PathListResult captures the outcome of the editor when it closes.
type PathListResult struct {
	Done      bool     // true if the user confirmed (enter on the list)
	Cancelled bool     // true if the user pressed esc on the list
	Paths     []string // the paths at the moment of close
}

// Update handles a key event. The first return is the new editor state;
// the second is non-nil when the editor is closing (Done or Cancelled).
func (e PathListEditor) Update(msg tea.KeyPressMsg) (PathListEditor, *PathListResult, tea.Cmd) {
	key := msg.String()
	if e.adding {
		switch key {
		case "enter":
			val := strings.TrimSpace(e.input.Value())
			if val != "" {
				e.paths = append(e.paths, val)
				e.cursor = len(e.paths) - 1
			}
			e.adding = false
			e.input.Reset()
			return e, nil, nil
		case "esc":
			e.adding = false
			e.input.Reset()
			return e, nil, nil
		}
		newInput, cmd := e.input.Update(msg)
		e.input = newInput
		return e, nil, cmd
	}

	switch key {
	case "j", "down":
		if e.cursor < len(e.paths)-1 {
			e.cursor++
		}
	case "k", "up":
		if e.cursor > 0 {
			e.cursor--
		}
	case "a", "+":
		e.input = textinput.New()
		e.input.Placeholder = "/path/to/music/folder"
		w := e.width - 4
		if w < 20 {
			w = 20
		}
		e.input.SetWidth(w)
		e.input.CharLimit = 512
		e.input.Focus()
		e.adding = true
	case "d", "x", "delete":
		if e.cursor < len(e.paths) {
			e.paths = append(e.paths[:e.cursor], e.paths[e.cursor+1:]...)
			if e.cursor >= len(e.paths) && e.cursor > 0 {
				e.cursor--
			}
		}
	case "enter":
		return e, &PathListResult{Done: true, Paths: e.Paths()}, nil
	case "esc":
		return e, &PathListResult{Cancelled: true, Paths: e.Paths()}, nil
	}
	return e, nil, nil
}

// View returns the rendered editor box.
func (e PathListEditor) View() string {
	titleStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	cursorStyle := lipgloss.NewStyle().
		Foreground(theme.T.AccentAlt()).
		Bold(true)

	innerW := e.width - 2 // border on each side

	var rows []string
	rows = append(rows, titleStyle.Render(e.Title))
	rows = append(rows, dimStyle.Render(strings.Repeat("─", innerW)))

	if len(e.paths) == 0 {
		rows = append(rows, dimStyle.Render("  (no paths configured)"))
	} else {
		for i, p := range e.paths {
			prefix := "  "
			style := textStyle
			if i == e.cursor && !e.adding {
				prefix = "▶ "
				style = cursorStyle
			}
			line := prefix + p
			if lipgloss.Width(line) > innerW {
				line = line[:innerW-1] + "…"
			}
			rows = append(rows, style.Render(line))
		}
	}

	if e.adding {
		rows = append(rows, "")
		rows = append(rows, dimStyle.Render("  add new path:"))
		rows = append(rows, "  "+e.input.View())
	}

	rows = append(rows, dimStyle.Render(strings.Repeat("─", innerW)))
	if e.adding {
		rows = append(rows, dimStyle.Render(fmt.Sprintf("  %s", "enter add · esc cancel input")))
	} else {
		rows = append(rows, dimStyle.Render("  a add · d remove · enter save · esc cancel"))
	}

	body := strings.Join(rows, "\n")
	return lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1).
		Width(e.width).
		Render(body)
}
