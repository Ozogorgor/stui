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
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// overlayPopupSize returns the width and height to give an overlay screen.
// Overlays render at popup dimensions so they don't fill the whole terminal.
func overlayPopupSize(termW, termH int) (w, h int) {
	w = termW - 8
	if w > 120 {
		w = 120
	}
	if w < 60 {
		w = 60
	}
	h = termH - 6
	if h > 42 {
		h = 42
	}
	if h < 20 {
		h = 20
	}
	return w, h
}

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
	overlay screen.Screen   // non-nil while a popup overlay is open
	width   int
	height  int
}

// NewRootModel creates a RootModel with `initial` as the active screen.
func NewRootModel(initial screen.Screen) RootModel {
	return RootModel{active: initial}
}

// Init delegates to the active screen.
func (r RootModel) Init() tea.Cmd {
	return r.active.Init()
}

// SetProgram propagates the program to the active screen.
func (r *RootModel) SetProgram(p *tea.Program) {
	if ls, ok := r.active.(LegacyScreen); ok {
		r.active = ls.SetProgram(p)
	}
}

// Update handles global keys then delegates to the active screen.
func (r RootModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	// Track current terminal size so newly activated screens get it immediately.
	if ws, ok := msg.(tea.WindowSizeMsg); ok {
		r.width = ws.Width
		r.height = ws.Height
		// Forward popup-capped size to overlay if one is open.
		if r.overlay != nil {
			pw, ph := overlayPopupSize(r.width, r.height)
			sized, _ := r.overlay.Update(tea.WindowSizeMsg{Width: pw, Height: ph})
			r.overlay = sized
		}
	}

	// ── Open overlay (from screen.OpenOverlayCmd) ──────────────────────────────
	if o, ok := msg.(screen.OpenOverlayMsg); ok {
		r.overlay = o.Screen
		initCmd := r.overlay.Init()
		if r.width > 0 {
			pw, ph := overlayPopupSize(r.width, r.height)
			sized, sizeCmd := r.overlay.Update(tea.WindowSizeMsg{Width: pw, Height: ph})
			r.overlay = sized
			return r, tea.Batch(initCmd, sizeCmd)
		}
		return r, initCmd
	}

	// ── Close overlay ──────────────────────────────────────────────────────────
	if _, ok := msg.(screen.CloseOverlayMsg); ok {
		r.overlay = nil
		return r, nil
	}

	// ── Screen transition (from screen.TransitionCmd) ──────────────────────────
	if t, ok := msg.(screen.TransitionMsg); ok {
		// If a transition fires from within an overlay, replace the overlay
		// instead of touching the active screen.
		if r.overlay != nil {
			r.overlay = t.Next
			initCmd := r.overlay.Init()
			if r.width > 0 {
				pw, ph := overlayPopupSize(r.width, r.height)
				sized, sizeCmd := r.overlay.Update(tea.WindowSizeMsg{Width: pw, Height: ph})
				r.overlay = sized
				return r, tea.Batch(initCmd, sizeCmd)
			}
			return r, initCmd
		}
		if t.PushBack {
			r.history = append(r.history, r.active)
		}
		r.active = t.Next
		initCmd := r.active.Init()
		// Inject the current window size into the new screen right away so its
		// View() renders at full size instead of falling back to the zero-width stub.
		if r.width > 0 {
			sized, sizeCmd := r.active.Update(tea.WindowSizeMsg{Width: r.width, Height: r.height})
			r.active = sized
			return r, tea.Batch(initCmd, sizeCmd)
		}
		return r, initCmd
	}

	// ── When overlay is open, route input there; ESC/PopMsg close it ──────────
	if r.overlay != nil {
		// Background tick messages must always reach the active screen so that
		// spinner animation chains stay alive while a popup is open.
		if _, ok := msg.(spinner.TickMsg); ok {
			next, cmd := r.active.Update(msg)
			r.active = next
			return r, cmd
		}

		// ESC closes the overlay.
		if key, ok := msg.(tea.KeyPressMsg); ok && key.String() == "esc" {
			r.overlay = nil
			return r, nil
		}
		// PopMsg from overlay screen closes it.
		if _, ok := msg.(screen.PopMsg); ok {
			r.overlay = nil
			return r, nil
		}
		// ctrl+c still quits even when overlay is up.
		if key, ok := msg.(tea.KeyPressMsg); ok && key.String() == "ctrl+c" {
			return r, tea.Quit
		}
		next, cmd := r.overlay.Update(msg)
		r.overlay = next
		return r, cmd
	}

	// ── Global keys ────────────────────────────────────────────────────
	if key, ok := msg.(tea.KeyPressMsg); ok {
		switch key.String() {
		case "ctrl+c":
			return r, tea.Quit
		case "q":
			// Only quit from the root screen; subscreens (search, settings, etc.)
			// must handle or ignore "q" themselves so typed text is not intercepted.
			if len(r.history) == 0 {
				return r, tea.Quit
			}
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

// View delegates to the active screen, always enforcing alt-screen and mouse
// mode so subscreens don't need to declare these themselves.
// When an overlay is open it is composited as a centered popup over the base.
func (r RootModel) View() tea.View {
	v := r.active.View()
	v.AltScreen = true
	v.MouseMode = tea.MouseModeCellMotion

	if r.overlay != nil && r.width > 0 && r.height > 0 {
		ov := r.overlay.View()
		popup := strings.TrimRight(ov.Content, "\n")
		// Wrap in a rounded border box with a solid background so underlying
		// content doesn't bleed through and obscure the popup text.
		boxed := lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(theme.T.Accent()).
			Background(theme.T.Surface()).
			Render(popup)
		v.Content = lipgloss.Place(r.width, r.height, lipgloss.Center, lipgloss.Center, boxed)
	}

	return v
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

func (s LegacyScreen) SetProgram(p *tea.Program) LegacyScreen {
	s.m.SetProgram(p)
	return s
}
