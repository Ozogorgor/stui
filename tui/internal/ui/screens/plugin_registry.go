package screens

// plugin_registry.go — Browse the plugin registry and install plugins.
//
// Layout:
//
//   ┌─────────────────────────────────────────────────────────────┐
//   │  🔌  Plugin Registry                                        │
//   ├─────────────────────────────────────────────────────────────┤
//   │  Loading registry…                                          │
//   │                                ── or after load ──          │
//   │  torrentio-rpc   1.2.0   rpc   Torrentio stream provider   │
//   │  subtitle-sync   0.4.1   rpc   OpenSubtitles integration   │
//   │▶ youtube-rpc     0.9.0   rpc   YouTube via yt-dlp [✓]     │
//   │                                                             │
//   ├─────────────────────────────────────────────────────────────┤
//   │  ↑↓ navigate   enter install   esc back                    │
//   └─────────────────────────────────────────────────────────────┘

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

// OpenPluginRegistryMsg triggers navigation to the plugin registry screen.
type OpenPluginRegistryMsg struct{}

// PluginRegistryScreen browses the registry and installs plugins.
type PluginRegistryScreen struct {
	Dims
	client      *ipc.Client
	entries     []ipc.RegistryEntry
	failedRepos []string
	cursor      int
	loading     bool
	installing  bool   // a download+install is in progress
	status      string // last status/error message


	spinner components.Spinner
}

func NewPluginRegistryScreen(client *ipc.Client) *PluginRegistryScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return &PluginRegistryScreen{
		client:  client,
		loading: true,
		spinner: *components.NewSpinner("fetching index…", dimStyle),
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m *PluginRegistryScreen) Init() tea.Cmd {
	m.spinner.Start()
	return func() tea.Msg {
		m.client.BrowseRegistry()
		return nil
	}
}

func (m *PluginRegistryScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case spinner.TickMsg:
		m.spinner.Update(msg)
		return m, nil

	case tea.WindowSizeMsg:
		m.setWindowSize(msg)

	case ipc.RegistryBrowseResultMsg:
		m.loading = false
		m.spinner.Stop()
		if msg.Err != nil {
			m.status = "Error: " + msg.Err.Error()
			return m, nil
		}
		m.entries = msg.Entries
		m.failedRepos = msg.FailedRepos
		if len(m.entries) == 0 {
			m.status = "No plugins found in any registry."
		} else if len(m.failedRepos) > 0 {
			m.status = fmt.Sprintf("Loaded %d plugin(s); %d repo(s) unreachable.", len(m.entries), len(m.failedRepos))
		} else {
			m.status = fmt.Sprintf("%d plugin(s) available.", len(m.entries))
		}

	case ipc.PluginInstallResultMsg:
		m.installing = false
		if msg.Err != nil {
			m.status = "Install failed: " + msg.Err.Error()
		} else {
			m.status = fmt.Sprintf("'%s' v%s installed — reloading…", msg.Name, msg.Version)
			// Mark as installed in the local list
			for i := range m.entries {
				if m.entries[i].Name == msg.Name {
					m.entries[i].Installed = true
					break
				}
			}
		}

	case tea.KeyPressMsg:
		if m.loading || m.installing {
			return m, nil
		}
		switch msg.String() {
		case "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }
		case "up", "k":
			if m.cursor > 0 {
				m.cursor--
			}
		case "down", "j":
			if m.cursor < len(m.entries)-1 {
				m.cursor++
			}
		case "enter", " ":
			if len(m.entries) > 0 {
				e := m.entries[m.cursor]
				if e.Installed {
					m.status = fmt.Sprintf("'%s' is already installed.", e.Name)
				} else {
					m.installing = true
					m.status = fmt.Sprintf("Installing '%s' v%s…", e.Name, e.Version)
					m.client.InstallPlugin(e.Name, e.Version, e.BinaryURL, e.Checksum)
				}
			}
		case "r":
			// Refresh the index
			m.loading = true
			m.status = ""
			m.entries = nil
			m.failedRepos = nil
			m.cursor = 0
			return m, func() tea.Msg {
				m.client.BrowseRegistry()
				return nil
			}
		}
	}

	return m, nil
}

func (m *PluginRegistryScreen) View() tea.View {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	greenStyle := lipgloss.NewStyle().Foreground(theme.T.Green())
	warnStyle := lipgloss.NewStyle().Foreground(theme.T.Yellow())

	header := accentStyle.Render("🔌  Plugin Registry")

	if m.loading {
		return tea.NewView(header + "\n\n  " + m.spinner.View() + "\n")
	}

	var sb strings.Builder
	sb.WriteString(header + "\n\n")

	if len(m.entries) == 0 {
		sb.WriteString(dimStyle.Render("  No plugins found.") + "\n")
	} else {
		// Column header
		hdr := fmt.Sprintf("  %-22s %-8s %-7s %s", "Name", "Version", "Type", "Description")
		sb.WriteString(dimStyle.Render(hdr) + "\n")
		sb.WriteString(dimStyle.Render("  "+strings.Repeat("─", m.rowWidth()-2)) + "\n")

		for i, e := range m.entries {
			isSelected := i == m.cursor

			prefix := "  "
			nameStyle := textStyle
			if isSelected {
				prefix = "▶ "
				nameStyle = accentStyle
			}

			name := fmt.Sprintf("%-22s", truncate(e.Name, 22))
			ver := fmt.Sprintf("%-8s", truncate(e.Version, 8))
			ptype := fmt.Sprintf("%-7s", truncate(e.PluginType, 7))
			desc := truncate(e.Description, m.descWidth())

			installed := ""
			if e.Installed {
				installed = " " + greenStyle.Render("[✓ installed]")
			}

			line := prefix +
				nameStyle.Render(name) + "  " +
				dimStyle.Render(ver) + "  " +
				dimStyle.Render(ptype) + "  " +
				textStyle.Render(desc) +
				installed

			if isSelected && m.installing {
				line += "  " + warnStyle.Render("installing…")
			}

			sb.WriteString(line + "\n")
		}
	}

	// Status line
	if m.status != "" {
		sb.WriteString("\n  " + warnStyle.Render(m.status) + "\n")
	}

	// Footer
	sb.WriteString("\n")
	var hint string
	if m.installing {
		hint = dimStyle.Render("  Installing, please wait…")
	} else {
		hint = hintBar("↑↓ navigate", "enter install", "r refresh", "esc back")
	}
	sb.WriteString(hint + "\n")

	return tea.NewView(sb.String())
}

func (m *PluginRegistryScreen) rowWidth() int {
	if m.width > 10 {
		return m.width
	}
	return 80
}

func (m *PluginRegistryScreen) descWidth() int {
	// 22 (name) + 2 + 8 (ver) + 2 + 7 (type) + 2 + prefix(2) = 45 chars used
	w := m.rowWidth() - 45
	if w < 10 {
		w = 10
	}
	if w > 60 {
		w = 60
	}
	return w
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	if max <= 1 {
		return "…"
	}
	return s[:max-1] + "…"
}
