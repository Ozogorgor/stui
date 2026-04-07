package screens

// plugin_repos.go — Plugin repository manager screen.
//
// Layout:
//
//   ┌─────────────────────────────────────────────────────────┐
//   │  🧩  Plugin Repositories                               │
//   ├─────────────────────────────────────────────────────────┤
//   │                                                         │
//   │  ▶ https://plugins.stui.dev  (built-in)                │
//   │    https://github.com/alice/stui-plugins               │
//   │    https://example.com/my-repo                         │
//   │                                                         │
//   │  [ Add repo URL… ]                                      │
//   │                                                         │
//   ├─────────────────────────────────────────────────────────┤
//   │  ↑↓ navigate   a add   d delete   enter confirm   esc  │
//   └─────────────────────────────────────────────────────────┘

import (
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// OpenPluginReposMsg is emitted by SettingsModel to open this screen.
type OpenPluginReposMsg struct{}

const builtinRepo = "https://plugins.stui.dev"

// PluginReposScreen manages the list of plugin repository URLs.
type PluginReposScreen struct {
	Dims
	client  *ipc.Client
	repos   []string // all repos; repos[0] is always the built-in one
	cursor  int      // row cursor (0 = first repo, len(repos) = add-row)
	loading bool
	status  string

	// Add-mode: user is typing a new URL
	adding bool
	addBuf string


	spinner components.Spinner
}

func NewPluginReposScreen(client *ipc.Client) *PluginReposScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return &PluginReposScreen{
		client:  client,
		loading: true,
		spinner: *components.NewSpinner("loading…", dimStyle),
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m *PluginReposScreen) Init() tea.Cmd {
	m.spinner.Start()
	return func() tea.Msg {
		m.client.GetPluginRepos()
		return nil
	}
}

func (m *PluginReposScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case spinner.TickMsg:
		_, cmd := m.spinner.Update(msg)
		return m, cmd

	case tea.WindowSizeMsg:
		m.setWindowSize(msg)

	case ipc.PluginReposResultMsg:
		m.loading = false
		m.spinner.Stop()
		if msg.Err != nil {
			m.status = "Error: " + msg.Err.Error()
			// Fall back to showing just the built-in repo
			m.repos = []string{builtinRepo}
			return m, nil
		}
		m.repos = msg.Repos
		// Ensure the built-in repo is always first
		if len(m.repos) == 0 || m.repos[0] != builtinRepo {
			m.repos = append([]string{builtinRepo}, m.repos...)
		}

	case tea.KeyPressMsg:
		if m.loading {
			return m, nil
		}
		if m.adding {
			return m.updateAdding(msg)
		}
		switch msg.String() {
		case "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }

		case "up", "k":
			if m.cursor > 0 {
				m.cursor--
			}

		case "down", "j":
			// cursor can go to len(repos) = add row, len(repos)+1 = browse-registry row
			if m.cursor < len(m.repos)+1 {
				m.cursor++
			}

		case "a":
			if m.cursor == len(m.repos) {
				// Cursor is on the add-row
				m.adding = true
				m.addBuf = ""
				m.status = "Type URL, Enter to add, Esc to cancel"
			}
		case "enter":
			if m.cursor == len(m.repos) {
				// add-row
				m.adding = true
				m.addBuf = ""
				m.status = "Type URL, Enter to add, Esc to cancel"
			} else if m.cursor == len(m.repos)+1 {
				// browse-registry row
				return m, func() tea.Msg { return OpenPluginRegistryMsg{} }
			}

		case "d", "backspace":
			if m.cursor > 0 && m.cursor < len(m.repos) {
				// Never delete the built-in repo (cursor 0)
				m.repos = append(m.repos[:m.cursor], m.repos[m.cursor+1:]...)
				if m.cursor >= len(m.repos) {
					m.cursor = len(m.repos) - 1
				}
				m.client.SetPluginRepos(m.repos)
				m.status = "Repo removed."
			} else if m.cursor == 0 {
				m.status = "The built-in repo cannot be removed."
			}
		}
	}

	return m, nil
}

func (m *PluginReposScreen) updateAdding(msg tea.KeyPressMsg) (screen.Screen, tea.Cmd) {
	switch msg.Code {
	case tea.KeyEsc:
		m.adding = false
		m.addBuf = ""
		m.status = ""

	case tea.KeyEnter:
		url := strings.TrimSpace(m.addBuf)
		if url == "" {
			m.adding = false
			m.status = ""
			return m, nil
		}
		// Reject duplicates
		for _, r := range m.repos {
			if r == url {
				m.status = "Already in list."
				m.adding = false
				m.addBuf = ""
				return m, nil
			}
		}
		m.repos = append(m.repos, url)
		m.cursor = len(m.repos) - 1
		m.client.SetPluginRepos(m.repos)
		m.adding = false
		m.addBuf = ""
		m.status = "Repo added."

	case tea.KeyBackspace:
		if len(m.addBuf) > 0 {
			m.addBuf = m.addBuf[:len(m.addBuf)-1]
		}

	default:
		if len(msg.Text) > 0 {
			m.addBuf += msg.Text
		}
	}
	return m, nil
}

// ── View ──────────────────────────────────────────────────────────────────────

func (m *PluginReposScreen) View() tea.View {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	warnStyle := lipgloss.NewStyle().Foreground(theme.T.Accent())

	header := accentStyle.Render("🧩  Plugin Repositories")

	if m.loading {
		return tea.NewView(header + "\n\n  " + m.spinner.View() + "\n")
	}

	var lines []string

	for i, repo := range m.repos {
		prefix := "  "
		isSelected := i == m.cursor && !m.adding

		var lineStyle lipgloss.Style
		if isSelected {
			prefix = "▶ "
			lineStyle = accentStyle
		} else {
			lineStyle = textStyle
		}

		suffix := ""
		if i == 0 {
			suffix = dimStyle.Render("  (built-in)")
		}

		lines = append(lines, lineStyle.Render(prefix+repo)+suffix)
	}

	// "Add" row
	addRowIdx := len(m.repos)
	if m.adding {
		// Show input box
		display := m.addBuf
		if display == "" {
			display = dimStyle.Render("_")
		}
		boxStyle := lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(theme.T.Accent()).
			Width(m.addInputWidth()).
			Padding(0, 1)
		lines = append(lines, "")
		lines = append(lines, accentStyle.Render("  Add repo:"))
		lines = append(lines, "  "+boxStyle.Render(display))
	} else {
		isSelected := m.cursor == addRowIdx
		if isSelected {
			lines = append(lines, "")
			lines = append(lines, accentStyle.Render("▶ [ + Add community repo… ]"))
		} else {
			lines = append(lines, "")
			lines = append(lines, dimStyle.Render("    [ + Add community repo… ]"))
		}
	}

	// "Browse Registry" row
	if !m.adding {
		isSelected := m.cursor == len(m.repos)+1
		if isSelected {
			lines = append(lines, accentStyle.Render("▶ [ 🔌 Browse plugin registry… ]"))
		} else {
			lines = append(lines, dimStyle.Render("    [ 🔌 Browse plugin registry… ]"))
		}
	}

	body := "  " + strings.Join(lines, "\n")

	// Footer
	var hintStr string
	switch {
	case m.adding:
		hintStr = hintBar("Type URL", "enter add", "esc cancel")
	case m.cursor == 0:
		hintStr = hintBar("↑↓ navigate", "a add repo", "esc back")
	case m.cursor == len(m.repos)+1:
		hintStr = hintBar("↑↓ navigate", "enter open registry", "esc back")
	default:
		hintStr = hintBar("↑↓ navigate", "a add", "d delete", "esc back")
	}
	var footer string
	if m.status != "" {
		footer = warnStyle.Render("  "+m.status) + "\n" + hintStr
	} else {
		footer = hintStr
	}

	return tea.NewView(header + "\n\n" + body + "\n\n" + footer + "\n")
}

func (m *PluginReposScreen) addInputWidth() int {
	w := m.width - 10
	if w < 30 {
		w = 30
	}
	if w > 80 {
		w = 80
	}
	return w
}
