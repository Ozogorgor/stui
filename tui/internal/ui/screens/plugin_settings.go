package screens

// plugin_settings.go — Provider API-key configuration screen.
//
// Layout (two panels, left = provider list, right = fields for selected provider):
//
//   ┌─────────────────────────────────────────────────┐
//   │  🔑  Provider Settings                          │
//   ├──────────────────┬──────────────────────────────┤
//   │ Providers        │ TMDB                         │
//   │                  │                              │
//   │ ● TMDB     ✓    │  API Key                     │
//   │   OMDB     ✗    │  ┌────────────────────────┐  │
//   │ ● Last.fm  ✓    │  │ ••••••••••••••••••     │  │
//   │ ○ IMDB          │  └────────────────────────┘  │
//   │ ○ AniList       │  Get one free at             │
//   │ ○ Jikan (MAL)   │  themoviedb.org/settings/api │
//   │ ○ MusicBrainz   │                              │
//   ├──────────────────┴──────────────────────────────┤
//   │  ↑↓ navigate   tab switch panel   enter save   │
//   └─────────────────────────────────────────────────┘

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// ── Messages ──────────────────────────────────────────────────────────────────

// OpenPluginSettingsMsg is emitted by SettingsModel to request this screen.
type OpenPluginSettingsMsg struct{}

// ── PluginSettingsScreen ──────────────────────────────────────────────────────

// PluginSettingsScreen lets the user configure API keys for providers that
// require them. Keys are sent to the runtime via SetConfig and persisted.
type PluginSettingsScreen struct {
	Dims
	client    *ipc.Client
	providers []ipc.ProviderSchema

	// focus: false = left panel (provider list), true = right panel (fields)
	rightFocus  bool
	provCursor  int // selected provider index
	fieldCursor int // selected field index within provider

	// Persisted buffer, one string per field per provider. Pre-populated
	// from ProviderField.Value when the runtime's GetProviderSettings
	// reply lands. The active textinput (when editing) copies its value
	// back into this slice on save; otherwise the view reads directly
	// from here.
	inputs [][]string

	// Active text input while editing. Non-nil iff editing is in progress.
	// Wraps bubbles/v2 textinput for full cursor movement, word-wise
	// edits, delete-forward, home/end, bracketed paste, etc. The
	// wrapper's Update method mutates its inner state on value-receiver
	// Update returns — keep working with the same pointer.
	editInput *components.StyledTextInput

	loading bool
	status  string // transient feedback ("Saved!", "Error: ...")

	spinner components.Spinner
}

// NewPluginSettingsScreen creates the screen and immediately requests provider data.
func NewPluginSettingsScreen(client *ipc.Client) *PluginSettingsScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return &PluginSettingsScreen{
		client:  client,
		loading: true,
		spinner: *components.NewSpinner("loading…", dimStyle),
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m *PluginSettingsScreen) Init() tea.Cmd {
	m.spinner.Start()
	return func() tea.Msg {
		m.client.GetProviderSettings()
		return nil
	}
}

func (m *PluginSettingsScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case spinner.TickMsg:
		_, cmd := m.spinner.Update(msg)
		return m, cmd

	case tea.WindowSizeMsg:
		m.setWindowSize(msg)

	// Bracketed-paste events arrive here when the user hits ctrl+v /
	// shift+insert / right-click-paste in a modern terminal. They are a
	// top-level `tea.PasteMsg`, NOT wrapped in `tea.KeyPressMsg`, so the
	// key-path below never sees them. Strip newlines (pasting multi-line
	// content into a one-line URL/API-key field would break the layout),
	// then forward to the active text input which inserts at cursor.
	case tea.PasteMsg:
		if m.editInput != nil {
			content := strings.ReplaceAll(msg.Content, "\n", "")
			content = strings.ReplaceAll(content, "\r", "")
			cmd := m.editInput.Update(tea.PasteMsg{Content: content})
			return m, cmd
		}

	// ── Provider data arrived ──────────────────────────────────────────────
	case ipc.ProviderSettingsResultMsg:
		m.loading = false
		m.spinner.Stop()
		if msg.Err != nil {
			m.status = fmt.Sprintf("Error loading providers: %v", msg.Err)
			return m, nil
		}
		m.providers = msg.Providers
		// Build input buffers, pre-filled with existing config values
		m.inputs = make([][]string, len(m.providers))
		for i, p := range m.providers {
			m.inputs[i] = make([]string, len(p.Fields))
			for j, f := range p.Fields {
				m.inputs[i][j] = f.Value
			}
		}
		m.provCursor = 0
		m.fieldCursor = 0

	// ── Keyboard ──────────────────────────────────────────────────────────
	case tea.KeyPressMsg:
		if m.loading {
			return m, nil
		}

		// Editing mode: capture all keys through the text input.
		if m.editInput != nil {
			return m.updateEditing(msg)
		}

		switch msg.String() {
		case "esc":
			// Pop back to settings
			return m, func() tea.Msg { return screen.PopMsg{} }

		case "tab":
			if len(m.providers) > 0 && len(m.currentProvider().Fields) > 0 {
				m.rightFocus = !m.rightFocus
				m.fieldCursor = 0
			}

		case "up", "k":
			if !m.rightFocus {
				if m.provCursor > 0 {
					m.provCursor--
					m.fieldCursor = 0
				}
			} else {
				if m.fieldCursor > 0 {
					m.fieldCursor--
				}
			}

		case "down", "j":
			if !m.rightFocus {
				if m.provCursor < len(m.providers)-1 {
					m.provCursor++
					m.fieldCursor = 0
				}
			} else {
				p := m.currentProvider()
				if m.fieldCursor < len(p.Fields)-1 {
					m.fieldCursor++
				}
			}

		case "enter":
			// From left panel: jump straight into editing the first field
			// of the selected provider. One keystroke to start configuring.
			if len(m.currentProvider().Fields) > 0 {
				if !m.rightFocus {
					m.rightFocus = true
					m.fieldCursor = 0
				}
				cmd := m.beginEdit()
				return m, cmd
			}

		case "left", "h":
			m.rightFocus = false
		}
	}

	return m, nil
}

// beginEdit creates a fresh textinput for the current field pre-populated
// with the stored value, focuses it, and returns the blink cmd. The
// caller must return the cmd so the cursor animates.
func (m *PluginSettingsScreen) beginEdit() tea.Cmd {
	p := m.currentProvider()
	if m.fieldCursor >= len(p.Fields) {
		return nil
	}
	field := p.Fields[m.fieldCursor]

	style := components.TextInputStyleDefault
	if field.Masked {
		style = components.TextInputStylePassword
	}
	ti := components.NewStyledTextInput(style, "")
	ti.SetValue(m.inputs[m.provCursor][m.fieldCursor])
	ti.CursorEnd()
	ti.SetCharLimit(512)
	// Box content width minus the rounded border and 1-cell padding on
	// each side. Keep in sync with the box dimensions in View().
	boxInner := m.width - 22 - 10
	if boxInner < 20 {
		boxInner = 20
	}
	ti.SetWidth(boxInner)
	blinkCmd := ti.Focus()

	m.editInput = ti
	m.status = fmt.Sprintf("Editing %s — Enter saves, Tab next, Esc cancels", field.Label)
	return blinkCmd
}

// updateEditing handles keys while a field is active. Explicit cases
// (esc/enter/tab/shift+tab) handle the screen's own semantics; every
// other key — arrows, home/end, ctrl+a/e/w/u, backspace, delete, printable
// text, bracketed-paste fallback — is forwarded to the textinput which
// handles cursor movement and edits natively.
func (m *PluginSettingsScreen) updateEditing(msg tea.KeyPressMsg) (screen.Screen, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.editInput = nil
		m.status = ""
		return m, nil

	case "enter":
		m.saveCurrentField()
		return m, nil

	case "tab":
		// Save current field and advance to the next one, staying in
		// edit mode so the user can chain "type URL → tab → type key →
		// enter to save" without re-pressing enter between each field.
		m.saveCurrentField()
		p := m.currentProvider()
		if m.fieldCursor < len(p.Fields)-1 {
			m.fieldCursor++
			return m, m.beginEdit()
		}
		return m, nil

	case "shift+tab":
		m.saveCurrentField()
		if m.fieldCursor > 0 {
			m.fieldCursor--
			return m, m.beginEdit()
		}
		return m, nil
	}

	// Everything else: forward to textinput. It mutates state via its
	// pointer receivers inside StyledTextInput.Update.
	cmd := m.editInput.Update(msg)
	return m, cmd
}

// saveCurrentField reads the active textinput's value, persists it via
// SetConfig, mirrors it into the local buffer, and drops out of edit
// mode. Shared by Enter (save + exit) and Tab (save + advance).
func (m *PluginSettingsScreen) saveCurrentField() {
	p := m.currentProvider()
	if m.fieldCursor >= len(p.Fields) || m.editInput == nil {
		m.editInput = nil
		return
	}
	field := p.Fields[m.fieldCursor]
	val := m.editInput.Value()
	m.inputs[m.provCursor][m.fieldCursor] = val
	m.client.SetConfig(field.Key, val)
	m.editInput = nil
	m.status = fmt.Sprintf("Saved %s.%s", p.Name, field.Label)
	// Local mirror for immediate Configured-indicator feedback.
	m.providers[m.provCursor].Fields[m.fieldCursor].Configured = val != ""
}

func (m *PluginSettingsScreen) currentProvider() ipc.ProviderSchema {
	if len(m.providers) == 0 || m.provCursor >= len(m.providers) {
		return ipc.ProviderSchema{}
	}
	return m.providers[m.provCursor]
}

// ── View ──────────────────────────────────────────────────────────────────────

func (m *PluginSettingsScreen) View() tea.View {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())

	header := accentStyle.Render("🔑  Provider Settings")

	if m.loading {
		return tea.NewView(header + "\n\n  " + m.spinner.View() + "\n")
	}

	if len(m.providers) == 0 {
		return tea.NewView(header + "\n\n" + dimStyle.Render("  No providers found.") + "\n")
	}

	// ── Left panel: provider list ──────────────────────────────────────────
	leftW := 22
	var leftLines []string
	for i, p := range m.providers {
		var indicator string
		if p.Active {
			indicator = accentStyle.Render("●")
		} else {
			indicator = dimStyle.Render("○")
		}

		var configMark string
		if len(p.Fields) > 0 {
			allConfigured := true
			for _, f := range p.Fields {
				if !f.Configured {
					allConfigured = false
					break
				}
			}
			if allConfigured {
				configMark = accentStyle.Render(" ✓")
			} else {
				configMark = dimStyle.Render(" ✗")
			}
		}

		prefix := "  "
		if i == m.provCursor {
			prefix = "▶ "
		}

		var nameStyle lipgloss.Style
		if i == m.provCursor && !m.rightFocus {
			nameStyle = accentStyle
		} else if i == m.provCursor {
			nameStyle = textStyle
		} else {
			nameStyle = dimStyle
		}

		line := indicator + " " + nameStyle.Render(prefix+p.Name) + configMark
		leftLines = append(leftLines, line)
	}
	leftPanel := lipgloss.NewStyle().Width(leftW).PaddingLeft(1).
		Render(strings.Join(leftLines, "\n"))

	// ── Right panel: fields for selected provider ──────────────────────────
	rightW := m.width - leftW - 6
	if rightW < 24 {
		rightW = 24
	}

	p := m.currentProvider()
	var rightLines []string
	rightLines = append(rightLines, accentStyle.Render("  "+p.Name))
	rightLines = append(rightLines, dimStyle.Render("  "+p.Description))
	rightLines = append(rightLines, "")

	if len(p.Fields) == 0 {
		rightLines = append(rightLines, dimStyle.Render("  No configuration required."))
	} else {
		for fi, field := range p.Fields {
			focused := m.rightFocus && fi == m.fieldCursor

			// Label row
			var labelSt lipgloss.Style
			if focused {
				labelSt = accentStyle
			} else {
				labelSt = textStyle
			}
			prefix := "  "
			if focused {
				prefix = "▶ "
			}
			rightLines = append(rightLines, labelSt.Render(prefix+field.Label))

			// Input box
			rawVal := m.inputs[m.provCursor][fi]
			var display string
			switch {
			case m.editInput != nil && focused:
				// Active edit: let the textinput render its own cursor +
				// text (masked automatically when style is Password). This
				// gives the user a real blinking cursor, full arrow-key
				// editing, home/end, delete, etc.
				display = m.editInput.View()
			case field.Masked:
				// Not editing a masked field: hide the stored value
				// behind 20 bullets if configured, else show (not set).
				if field.Configured {
					display = strings.Repeat("•", 20)
				} else {
					display = dimStyle.Render("(not set)")
				}
			default:
				// Not editing a non-masked field: show the plaintext
				// value, or (not set) placeholder when empty.
				if rawVal == "" && !field.Configured {
					display = dimStyle.Render("(not set)")
				} else {
					display = rawVal
				}
			}

			// MarginLeft instead of a leading "  " prefix: the box is
			// multi-line (top border, content, bottom border), and
			// string concatenation only indents the first line, leaving
			// the middle + bottom flush-left — visually the top cap
			// looks pushed right. MarginLeft indents every line.
			boxStyle := lipgloss.NewStyle().
				Border(lipgloss.RoundedBorder()).
				Width(rightW-6).
				Padding(0, 1).
				MarginLeft(2)
			if focused {
				boxStyle = boxStyle.BorderForeground(theme.T.Accent())
			} else {
				boxStyle = boxStyle.BorderForeground(theme.T.TextDim())
			}

			rightLines = append(rightLines, boxStyle.Render(display))

			// Hint
			if field.Hint != "" {
				rightLines = append(rightLines, dimStyle.Render("  "+field.Hint))
			}
			rightLines = append(rightLines, "")
		}
	}

	rightPanel := lipgloss.NewStyle().Width(rightW).PaddingLeft(2).
		Render(strings.Join(rightLines, "\n"))

	// ── Join ──────────────────────────────────────────────────────────────
	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, rightPanel)

	// ── Footer ────────────────────────────────────────────────────────────
	var hintStr string
	if m.editInput != nil {
		hintStr = hintBar("type to edit", "enter save", "tab next field", "esc cancel")
	} else {
		hintStr = hintBar("↑↓ navigate", "tab switch panel", "enter edit", "esc back/cancel")
	}
	var footer string
	if m.status != "" {
		footer = accentStyle.Render("  "+m.status) + "\n" + hintStr
	} else {
		footer = hintStr
	}

	return tea.NewView(header + "\n\n" + body + "\n\n" + footer + "\n")
}
