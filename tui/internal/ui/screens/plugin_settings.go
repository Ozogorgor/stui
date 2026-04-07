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

	// One text input value per field per provider:
	// inputs[provIdx][fieldIdx] = typed string
	inputs [][]string

	// Whether each field input is in edit mode (cursor visible, keys captured)
	editing bool

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

		// Editing mode: capture all printable keys + backspace + enter/esc
		if m.editing {
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
			if m.rightFocus && len(m.currentProvider().Fields) > 0 {
				m.editing = true
				m.status = "Type API key, Enter to save, Esc to cancel"
			}

		case "left", "h":
			m.rightFocus = false
		}
	}

	return m, nil
}

// updateEditing handles key input while a field is being edited.
func (m *PluginSettingsScreen) updateEditing(msg tea.KeyPressMsg) (screen.Screen, tea.Cmd) {
	switch msg.Code {
	case tea.KeyEsc:
		m.editing = false
		m.status = ""

	case tea.KeyEnter:
		// Save the value via SetConfig
		p := m.currentProvider()
		if m.fieldCursor < len(p.Fields) {
			field := p.Fields[m.fieldCursor]
			val := m.inputs[m.provCursor][m.fieldCursor]
			m.client.SetConfig(field.Key, val)
			m.editing = false
			m.status = fmt.Sprintf("Saved %s.%s", p.Name, field.Label)
			// Mark the field as configured locally for immediate feedback
			m.providers[m.provCursor].Fields[m.fieldCursor].Configured = val != ""
		}

	case tea.KeyBackspace:
		cur := m.inputs[m.provCursor][m.fieldCursor]
		if len(cur) > 0 {
			m.inputs[m.provCursor][m.fieldCursor] = cur[:len(cur)-1]
		}

	default:
		if len(msg.Text) > 0 {
			m.inputs[m.provCursor][m.fieldCursor] += msg.Text
		}
	}

	return m, nil
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
			if field.Masked && !m.editing {
				if field.Configured {
					display = strings.Repeat("•", 20)
				} else {
					display = dimStyle.Render("(not set)")
				}
			} else if m.editing && focused {
				// Show masked but with length indicator while typing
				display = strings.Repeat("•", len(rawVal))
				if len(rawVal) == 0 {
					display = dimStyle.Render("_")
				}
			} else {
				if rawVal == "" && !field.Configured {
					display = dimStyle.Render("(not set)")
				} else {
					display = rawVal
				}
			}

			boxStyle := lipgloss.NewStyle().
				Border(lipgloss.RoundedBorder()).
				Width(rightW-6).
				Padding(0, 1)
			if focused {
				boxStyle = boxStyle.BorderForeground(theme.T.Accent())
			} else {
				boxStyle = boxStyle.BorderForeground(theme.T.TextDim())
			}

			rightLines = append(rightLines, "  "+boxStyle.Render(display))

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
	if m.editing {
		hintStr = hintBar("Type API key", "enter save", "esc cancel")
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
