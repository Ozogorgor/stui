package ui

// root.go — Screen-based model tree pattern for Bubble Tea.
//
// # Problem
//
// As the TUI grows, a monolithic Update() function balloons into hundreds of
// lines with deeply nested switch statements.  Each new screen multiplies the
// complexity.
//
// # Solution: Screen interfaces
//
// Every screen implements the `screen.Screen` interface (its own Init/Update/View).
// The RootModel holds the active screen and simply delegates all Bubble Tea
// calls to it.  Global keys (quit, nav) are intercepted in RootModel.Update
// before forwarding — keeping every screen's logic isolated.
//
// # Screen transitions
//
// Screens signal a transition by returning a special Cmd:
//
//   return m, screen.TransitionCmd(screens.NewSearchScreen(m.ipc), true)
//
// RootModel catches TransitionCmd and swaps the active screen.
//
// # Navigation history
//
// RootModel maintains a simple screen stack.  ESC or Backspace pops the
// previous screen instead of going to a hard-coded "home".
//
// # Current usage
//
// This file wires the existing Model (in ui.go) as the initial "legacy"
// screen so behaviour is unchanged.  New screens should implement screen.Screen
// directly.  The migration path is incremental — extract screens one at a
// time as they grow beyond ~100 lines.

import (
	tea "charm.land/bubbletea/v2"
	"github.com/stui/stui/internal/ui/screen"
)

// ── RootModel ─────────────────────────────────────────────────────────────────

// RootModel is the top-level Bubble Tea model.
// It owns the screen stack and forwards all tea.Msg calls to the active screen.
//
// Global keys handled here (before forwarding to the active screen):
//
//	ctrl+c, q  — quit
//	ESC        — pop previous screen (if stack is non-empty)
type RootModel struct {
	active  screen.Screen
	history []screen.Screen // previous screens (stack); ESC pops the top
}

// NewRootModel creates a RootModel with `initial` as the active screen.
func NewRootModel(initial screen.Screen) RootModel {
	return RootModel{active: initial}
}

// Init delegates to the active screen.
func (r RootModel) Init() tea.Cmd {
	return r.active.Init()
}

// Update handles global keys then delegates to the active screen.
func (r RootModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	// ── Screen transition (from screen.TransitionCmd) ──────────────────────────
	if t, ok := msg.(screen.TransitionMsg); ok {
		if t.PushBack {
			r.history = append(r.history, r.active)
		}
		r.active = t.Next
		return r, r.active.Init()
	}

	// ── Global keys ────────────────────────────────────────────────────
	if key, ok := msg.(tea.KeyPressMsg); ok {
		switch key.String() {
		case "ctrl+c", "q":
			return r, tea.Quit
		case "esc":
			if len(r.history) > 0 {
				// Pop the previous screen
				prev := r.history[len(r.history)-1]
				r.history = r.history[:len(r.history)-1]
				r.active = prev
				return r, nil
			}
		}
	}

	// ── Pop screen (sent by child screens) ───────────────────────────
	if _, ok := msg.(screen.PopMsg); ok {
		if len(r.history) > 0 {
			prev := r.history[len(r.history)-1]
			r.history = r.history[:len(r.history)-1]
			r.active = prev
			return r, nil
		}
		return r, nil
	}

	// ── Delegate to active screen ──────────────────────────────────────
	next, cmd := r.active.Update(msg)
	r.active = next
	return r, cmd
}

// View delegates to the active screen.
func (r RootModel) View() tea.View {
	return r.active.View()
}

// ── LegacyScreen adapter ──────────────────────────────────────────────────────

// LegacyScreen wraps the existing Model (from ui.go) as a screen.Screen so it
// can be used as the initial screen inside RootModel without any changes to
// ui.go.
//
// Once screens are extracted from ui.go, this adapter can be removed.
type LegacyScreen struct {
	m Model
}

// NewLegacyScreen wraps an existing Model as a screen.Screen.
func NewLegacyScreen(m Model) LegacyScreen {
	return LegacyScreen{m: m}
}

func (s LegacyScreen) Init() tea.Cmd {
	return s.m.Init()
}

func (s LegacyScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	next, cmd := s.m.Update(msg)
	if nextModel, ok := next.(Model); ok {
		return LegacyScreen{m: nextModel}, cmd
	}
	// Fallback: keep current state if the type assertion fails
	return s, cmd
}

func (s LegacyScreen) View() tea.View {
	return s.m.View()
}
