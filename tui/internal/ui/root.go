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
//   return m, screen.TransitionCmd(screens.NewHelpScreen(), true)
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
	active         screen.Screen
	history        []screen.Screen // previous screens (stack); ESC pops the top
	overlay        screen.Screen   // non-nil while a popup overlay is open
	overlayHistory []screen.Screen // stack of overlays "below" the active one
	width          int
	height         int
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
		// If a transition fires from within an overlay, swap the overlay.
		// When PushBack is requested, save the current overlay so PopMsg
		// can return to it (e.g. Settings → DSP Settings → backspace
		// returns to Settings).
		if r.overlay != nil {
			if t.PushBack {
				r.overlayHistory = append(r.overlayHistory, r.overlay)
			}
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
		// ESC / backspace pops back to the previous overlay if one was
		// pushed (Settings → DSP), otherwise closes the overlay entirely.
		if key, ok := msg.(tea.KeyPressMsg); ok {
			s := key.String()
			if s == "esc" || s == "backspace" {
				if n := len(r.overlayHistory); n > 0 {
					prev := r.overlayHistory[n-1]
					r.overlayHistory = r.overlayHistory[:n-1]
					r.overlay = prev
					return r, nil
				}
				r.overlay = nil
				return r, nil
			}
		}
		// PopMsg from overlay screen: pop to previous overlay or close.
		if _, ok := msg.(screen.PopMsg); ok {
			if n := len(r.overlayHistory); n > 0 {
				prev := r.overlayHistory[n-1]
				r.overlayHistory = r.overlayHistory[:n-1]
				r.overlay = prev
				return r, nil
			}
			r.overlay = nil
			return r, nil
		}
		// ctrl+c still quits even when overlay is up.
		if key, ok := msg.(tea.KeyPressMsg); ok && key.String() == "ctrl+c" {
			return r, tea.Quit
		}
		// Mouse events come in raw terminal coordinates, but the overlay's
		// View() doesn't know it's been centered + wrapped in an extra
		// border by the View composite below. Translate the mouse to
		// popup-local coordinates before forwarding so click hit-tests
		// line up with what the user sees.
		if mm, ok := msg.(tea.MouseMsg); ok {
			msg = r.translateOverlayMouse(mm)
		}

		// User input (keys + mouse) is routed to the overlay only — the
		// overlay owns focus while open.
		if _, isKey := msg.(tea.KeyPressMsg); isKey {
			next, cmd := r.overlay.Update(msg)
			r.overlay = next
			return r, cmd
		}
		if _, isMouse := msg.(tea.MouseMsg); isMouse {
			next, cmd := r.overlay.Update(msg)
			r.overlay = next
			return r, cmd
		}

		// Everything else — runtime lifecycle events (`runtimeStartedMsg`,
		// `RuntimeErrorMsg`), IPC responses (`GridUpdateMsg`,
		// `PluginListMsg`, `CatalogStaleMsg`, player / MPD / DSP events),
		// spinner ticks, background timers — fans out to BOTH the active
		// screen and the overlay. The Model in ui.go holds the long-lived
		// state (client handle, mediaCache, grids, mpdNowPlaying, history);
		// starving it of these messages while any overlay is open
		// indefinitely deferred `m.client = msg.client` and other critical
		// assignments. Previous behaviour was an explicit allowlist which
		// was brittle — every new IPC message type needed opt-in. Fan-by-
		// default closes that hole.
		//
		// `fromIPC` is the envelope `listenIPC` uses to wrap every IPC
		// message before handing it off (see ui.go). The active screen
		// MUST see the envelope — its handler unwraps the inner message
		// AND re-arms `listenIPC` so the next IPC response can flow. The
		// overlay cannot do that re-arm (it has no `m.client`), and its
		// handlers type-switch on the unwrapped concrete type
		// (`ipc.PluginListMsg`, `ipc.RegistryBrowseResultMsg`, …), so
		// forwarding the `fromIPC` wrapper to the overlay would silently
		// drop every IPC response. Send the active screen the wrapper
		// (so it unwraps + re-subscribes) and the overlay the unwrapped
		// inner message directly.
		activeNext, activeCmd := r.active.Update(msg)
		r.active = activeNext
		overlayMsg := msg
		if f, ok := msg.(fromIPC); ok {
			overlayMsg = f.Msg
		}
		overlayNext, overlayCmd := r.overlay.Update(overlayMsg)
		r.overlay = overlayNext
		return r, tea.Batch(activeCmd, overlayCmd)
	}

	// ── Global keys ────────────────────────────────────────────────────
	if key, ok := msg.(tea.KeyPressMsg); ok {
		switch key.String() {
		case "ctrl+c":
			return r, tea.Quit
		// `q` is NOT handled here. The inner screen (ui.go::handleKey) owns
		// it so the search-bar's Focus state can veto the quit when the user
		// is typing a query. A duplicated root-level `case "q":` used to
		// fire first and bypass that check, leaking "queen"/"quit"/"quiet"
		// queries into an accidental quit.
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

// translateOverlayMouse converts a raw terminal mouse event into one whose
// X/Y are local to the overlay's content area. The composite View centers a
// boxed popup of size (popupW+2, popupH+2) inside the terminal — so the
// content origin is at (centerX+1, centerY+1).
func (r RootModel) translateOverlayMouse(msg tea.MouseMsg) tea.MouseMsg {
	if r.width <= 0 || r.height <= 0 {
		return msg
	}
	pw, ph := overlayPopupSize(r.width, r.height)
	boxedW := pw + 2 // +2 for the wrapper RoundedBorder
	boxedH := ph + 2
	leftX := (r.width - boxedW) / 2
	if leftX < 0 {
		leftX = 0
	}
	topY := (r.height - boxedH) / 2
	if topY < 0 {
		topY = 0
	}
	// +1 for the wrapper border so (0,0) maps to first content cell.
	dx := leftX + 1
	dy := topY + 1

	m := msg.Mouse()
	m.X -= dx
	m.Y -= dy

	switch msg.(type) {
	case tea.MouseClickMsg:
		return tea.MouseClickMsg(m)
	case tea.MouseReleaseMsg:
		return tea.MouseReleaseMsg(m)
	case tea.MouseWheelMsg:
		return tea.MouseWheelMsg(m)
	case tea.MouseMotionMsg:
		return tea.MouseMotionMsg(m)
	}
	return msg
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
		// Force popup content to exactly the allocated overlay dimensions so
		// lipgloss.Place centering matches translateOverlayMouse's math.
		// Without this, overlays that render fewer lines than allocated cause
		// the visual position and mouse-hit math to diverge.
		pw, ph := overlayPopupSize(r.width, r.height)
		lines := strings.Split(popup, "\n")
		for len(lines) < ph {
			lines = append(lines, strings.Repeat(" ", pw))
		}
		if len(lines) > ph {
			lines = lines[:ph]
		}
		popup = strings.Join(lines, "\n")
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
