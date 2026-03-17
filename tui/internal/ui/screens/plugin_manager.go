package screens

// plugin_manager.go — Unified Plugin Manager screen.
//
// Three tabs navigated with [Tab] / [Shift+Tab]:
//
//   [Installed](N)  [Available](N)  [Updates](N)
//
//   Installed — lists plugins currently loaded in the engine.
//     u  unload selected plugin
//     r  refresh list
//
//   Available — registry entries that are NOT yet installed.
//     enter  install selected plugin
//     r  refresh registry
//
//   Updates — installed plugins for which the registry has a newer version.
//     enter/u  update (re-install) selected plugin
//     r  refresh both lists
//
//   Common:
//     R  open Plugin Repositories screen
//     esc / q  close
//
// Opened with:  screen.TransitionCmd(screens.NewPluginManagerScreen(client), true)
// or via 'P' from the main view.

import (
	"fmt"
	"strconv"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// OpenPluginManagerMsg triggers navigation to the plugin manager screen.
type OpenPluginManagerMsg struct{}

// ── Tab enum ──────────────────────────────────────────────────────────────────

type pmTab int

const (
	pmInstalled pmTab = iota
	pmAvailable
	pmUpdates
)

// ── updateCandidate ───────────────────────────────────────────────────────────

type updateCandidate struct {
	plugin ipc.PluginInfo
	entry  ipc.RegistryEntry
}

// ── Screen ────────────────────────────────────────────────────────────────────

// PluginManagerScreen is the unified plugin management hub.
type PluginManagerScreen struct {
	client *ipc.Client
	tab    pmTab // active sub-tab

	// Installed tab
	plugins   []ipc.PluginInfo
	plCursor  int
	plLoading bool

	// Registry data (shared between Available and Updates)
	registry    []ipc.RegistryEntry
	failedRepos []string
	regLoading  bool

	// Available tab — uninstalled registry entries
	available []ipc.RegistryEntry
	avCursor  int

	// Updates tab — installed plugins with a newer registry version
	updates  []updateCandidate
	upCursor int

	// Transient UI state
	installing bool   // an install/update is in flight
	status     string // last status/error message

	width  int
	height int
}

func NewPluginManagerScreen(client *ipc.Client) *PluginManagerScreen {
	return &PluginManagerScreen{
		client:     client,
		plLoading:  true,
		regLoading: true,
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m *PluginManagerScreen) Init() tea.Cmd {
	return func() tea.Msg {
		m.client.ListPlugins()
		m.client.BrowseRegistry()
		return nil
	}
}

func (m *PluginManagerScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		m.width  = msg.Width
		m.height = msg.Height

	case ipc.PluginListMsg:
		m.plLoading = false
		if msg.Err != nil {
			m.status = "Error loading plugins: " + msg.Err.Error()
		} else {
			m.plugins = msg.Plugins
		}
		m.recomputeUpdates()

	case ipc.RegistryBrowseResultMsg:
		m.regLoading = false
		if msg.Err != nil {
			m.status = "Registry error: " + msg.Err.Error()
		} else {
			m.registry    = msg.Entries
			m.failedRepos = msg.FailedRepos
			m.available   = pmFilterUninstalled(msg.Entries)
		}
		m.recomputeUpdates()

	case ipc.PluginInstallResultMsg:
		m.installing = false
		if msg.Err != nil {
			m.status = "Install failed: " + msg.Err.Error()
		} else {
			m.status = fmt.Sprintf("'%s' v%s installed — refreshing…", msg.Name, msg.Version)
			// Refresh both lists to reflect the new state.
			m.plLoading  = true
			m.regLoading = true
			m.client.ListPlugins()
			m.client.BrowseRegistry()
		}

	case tea.KeyMsg:
		return m.handleKey(msg)
	}
	return m, nil
}

func (m *PluginManagerScreen) handleKey(msg tea.KeyMsg) (screen.Screen, tea.Cmd) {
	if m.installing {
		return m, nil
	}
	key := msg.String()

	// Global keys
	switch key {
	case "tab":
		m.tab = (m.tab + 1) % 3
		return m, nil
	case "shift+tab":
		m.tab = (m.tab + 2) % 3
		return m, nil
	case "esc", "q":
		return m, func() tea.Msg { return screen.PopMsg{} }
	case "R":
		return m, func() tea.Msg { return OpenPluginReposMsg{} }
	}

	// Tab-specific
	switch m.tab {
	case pmInstalled:
		return m.handleInstalledKey(key)
	case pmAvailable:
		return m.handleAvailableKey(key)
	case pmUpdates:
		return m.handleUpdatesKey(key)
	}
	return m, nil
}

func (m *PluginManagerScreen) handleInstalledKey(key string) (screen.Screen, tea.Cmd) {
	switch key {
	case "up", "k":
		if m.plCursor > 0 {
			m.plCursor--
		}
	case "down", "j":
		if m.plCursor < len(m.plugins)-1 {
			m.plCursor++
		}
	case "u", "x":
		if len(m.plugins) > 0 && !m.plLoading {
			p := m.plugins[m.plCursor]
			m.client.UnloadPlugin(p.ID)
			m.status = fmt.Sprintf("Unloading '%s'…", p.Name)
			m.plLoading = true
		}
	case "r":
		m.plLoading = true
		m.status = ""
		m.client.ListPlugins()
	}
	return m, nil
}

func (m *PluginManagerScreen) handleAvailableKey(key string) (screen.Screen, tea.Cmd) {
	switch key {
	case "up", "k":
		if m.avCursor > 0 {
			m.avCursor--
		}
	case "down", "j":
		if m.avCursor < len(m.available)-1 {
			m.avCursor++
		}
	case "enter", " ":
		if len(m.available) > 0 && !m.regLoading && !m.installing {
			e := m.available[m.avCursor]
			m.installing = true
			m.status = fmt.Sprintf("Installing '%s' v%s…", e.Name, e.Version)
			m.client.InstallPlugin(e.Name, e.Version, e.BinaryURL, e.Checksum)
		}
	case "r":
		m.regLoading = true
		m.status = ""
		m.client.BrowseRegistry()
	}
	return m, nil
}

func (m *PluginManagerScreen) handleUpdatesKey(key string) (screen.Screen, tea.Cmd) {
	switch key {
	case "up", "k":
		if m.upCursor > 0 {
			m.upCursor--
		}
	case "down", "j":
		if m.upCursor < len(m.updates)-1 {
			m.upCursor++
		}
	case "enter", "u":
		if len(m.updates) > 0 && !m.installing {
			u := m.updates[m.upCursor]
			m.installing = true
			m.status = fmt.Sprintf("Updating '%s' to v%s…", u.entry.Name, u.entry.Version)
			m.client.InstallPlugin(u.entry.Name, u.entry.Version, u.entry.BinaryURL, u.entry.Checksum)
		}
	case "r":
		m.plLoading  = true
		m.regLoading = true
		m.status     = ""
		m.client.ListPlugins()
		m.client.BrowseRegistry()
	}
	return m, nil
}

// recomputeUpdates cross-references installed plugins with the registry to find
// entries where the registry version is newer than what's currently installed.
func (m *PluginManagerScreen) recomputeUpdates() {
	if m.plLoading || m.regLoading {
		return
	}
	m.updates = nil
	for _, p := range m.plugins {
		for _, e := range m.registry {
			if strings.EqualFold(e.Name, p.Name) && pmIsNewer(e.Version, p.Version) {
				m.updates = append(m.updates, updateCandidate{plugin: p, entry: e})
				break
			}
		}
	}
}

// ── View ──────────────────────────────────────────────────────────────────────

func (m *PluginManagerScreen) View() string {
	acc  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim  := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())
	warn := lipgloss.NewStyle().Foreground(theme.T.Warn())

	var sb strings.Builder
	sb.WriteString("\n  " + acc.Render("🔌  Plugin Manager") + "\n\n")

	// ── Tab bar ───────────────────────────────────────────────────────────
	tabLabels := []string{"Installed", "Available", "Updates"}
	tabCounts := []string{
		fmt.Sprintf("(%d)", len(m.plugins)),
		fmt.Sprintf("(%d)", len(m.available)),
		"",
	}
	if len(m.updates) > 0 {
		tabCounts[2] = neon.Render(fmt.Sprintf("(%d ↑)", len(m.updates)))
	} else if !m.plLoading && !m.regLoading {
		tabCounts[2] = dim.Render("(✓)")
	}

	sb.WriteString("  ")
	for i, label := range tabLabels {
		if i == int(m.tab) {
			sb.WriteString(acc.Render("["+label+"]"))
		} else {
			sb.WriteString(dim.Render(" "+label+" "))
		}
		sb.WriteString(dim.Render(tabCounts[i]))
		if i < len(tabLabels)-1 {
			sb.WriteString(dim.Render("  "))
		}
	}
	sb.WriteString("\n")
	sb.WriteString("  " + dim.Render(strings.Repeat("─", m.contentWidth())) + "\n\n")

	// ── Active tab content ────────────────────────────────────────────────
	switch m.tab {
	case pmInstalled:
		sb.WriteString(m.viewInstalled())
	case pmAvailable:
		sb.WriteString(m.viewAvailable())
	case pmUpdates:
		sb.WriteString(m.viewUpdates())
	}

	// ── Status bar ────────────────────────────────────────────────────────
	if m.status != "" {
		style := neon
		if strings.HasPrefix(m.status, "Error") || strings.HasPrefix(m.status, "Install failed") {
			style = warn
		}
		sb.WriteString("\n  " + style.Render(m.status) + "\n")
	}

	// ── Footer ────────────────────────────────────────────────────────────
	sb.WriteString("\n" + hintBar("tab/shift+tab switch tab", "R repos", "esc close") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) viewInstalled() string {
	acc   := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim   := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	text  := lipgloss.NewStyle().Foreground(theme.T.Text())
	green := lipgloss.NewStyle().Foreground(theme.T.Success())
	red   := lipgloss.NewStyle().Foreground(lipgloss.Color("#e06c75"))

	var sb strings.Builder

	if m.plLoading {
		sb.WriteString("  " + dim.Render("Loading installed plugins…") + "\n")
		return sb.String()
	}
	if len(m.plugins) == 0 {
		sb.WriteString("  " + dim.Render("No plugins currently loaded.") + "\n")
		sb.WriteString("  " + dim.Render("Press ") + acc.Render("tab") + dim.Render(" to browse Available plugins.") + "\n")
		return sb.String()
	}

	hdr := fmt.Sprintf("  %-22s %-9s %-8s  Status", "Name", "Version", "Type")
	sb.WriteString(dim.Render(hdr) + "\n")

	for i, p := range m.plugins {
		sel := i == m.plCursor

		prefix := "  "
		nameS  := text.Render(fmt.Sprintf("%-22s", truncate(p.Name, 22)))
		if sel {
			prefix = "▶ "
			nameS  = acc.Render(fmt.Sprintf("%-22s", truncate(p.Name, 22)))
		}

		ver   := dim.Render(fmt.Sprintf("%-9s", truncate(p.Version, 9)))
		ptype := dim.Render(fmt.Sprintf("%-8s", truncate(p.PluginType, 8)))

		var badge string
		switch strings.ToLower(p.Status) {
		case "loaded":
			badge = green.Render("✓ loaded")
		case "failed":
			badge = red.Render("✗ failed")
		case "disabled":
			badge = dim.Render("○ disabled")
		default:
			badge = dim.Render(p.Status)
		}

		sb.WriteString(prefix + nameS + "  " + ver + "  " + ptype + "  " + badge + "\n")
	}

	sb.WriteString("\n  " + dim.Render("[↑↓] navigate  [u] unload  [r] refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) viewAvailable() string {
	acc  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim  := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	text := lipgloss.NewStyle().Foreground(theme.T.Text())
	green := lipgloss.NewStyle().Foreground(theme.T.Success())
	warn := lipgloss.NewStyle().Foreground(theme.T.Warn())

	var sb strings.Builder

	if m.regLoading {
		sb.WriteString("  " + dim.Render("Fetching plugin registry…") + "\n")
		return sb.String()
	}
	if len(m.failedRepos) > 0 {
		sb.WriteString("  " + warn.Render(fmt.Sprintf("⚠  %d repo(s) unreachable", len(m.failedRepos))) + "\n\n")
	}
	if len(m.available) == 0 {
		sb.WriteString("  " + green.Render("✓ All available plugins are already installed.") + "\n")
		return sb.String()
	}

	descW := m.contentWidth() - 2 - 22 - 2 - 9 - 2 - 8 - 2
	if descW < 10 {
		descW = 10
	}
	if descW > 55 {
		descW = 55
	}

	hdr := fmt.Sprintf("  %-22s %-9s %-8s  Description", "Name", "Version", "Type")
	sb.WriteString(dim.Render(hdr) + "\n")

	for i, e := range m.available {
		sel := i == m.avCursor

		prefix := "  "
		nameS  := text.Render(fmt.Sprintf("%-22s", truncate(e.Name, 22)))
		if sel {
			prefix = "▶ "
			nameS  = acc.Render(fmt.Sprintf("%-22s", truncate(e.Name, 22)))
		}

		ver   := dim.Render(fmt.Sprintf("%-9s", truncate(e.Version, 9)))
		ptype := dim.Render(fmt.Sprintf("%-8s", truncate(e.PluginType, 8)))
		desc  := text.Render(truncate(e.Description, descW))

		line := prefix + nameS + "  " + ver + "  " + ptype + "  " + desc
		if sel && m.installing {
			line += "  " + warn.Render("installing…")
		}
		sb.WriteString(line + "\n")
	}

	sb.WriteString("\n  " + dim.Render("[↑↓] navigate  [enter] install  [r] refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) viewUpdates() string {
	acc  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim  := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())
	text := lipgloss.NewStyle().Foreground(theme.T.Text())
	warn := lipgloss.NewStyle().Foreground(theme.T.Warn())

	var sb strings.Builder

	if m.plLoading || m.regLoading {
		sb.WriteString("  " + dim.Render("Checking for updates…") + "\n")
		return sb.String()
	}
	if len(m.updates) == 0 {
		sb.WriteString("  " + acc.Render("✓ All plugins are up to date.") + "\n")
		return sb.String()
	}

	hdr := fmt.Sprintf("  %-22s %-9s → %-9s  Type", "Name", "Installed", "Latest")
	sb.WriteString(dim.Render(hdr) + "\n")

	for i, u := range m.updates {
		sel := i == m.upCursor

		prefix := "  "
		nameS  := text.Render(fmt.Sprintf("%-22s", truncate(u.plugin.Name, 22)))
		if sel {
			prefix = "▶ "
			nameS  = acc.Render(fmt.Sprintf("%-22s", truncate(u.plugin.Name, 22)))
		}

		installed := dim.Render(fmt.Sprintf("%-9s", truncate(u.plugin.Version, 9)))
		latest    := neon.Render(fmt.Sprintf("%-9s", truncate(u.entry.Version, 9)))
		ptype     := dim.Render(truncate(u.entry.PluginType, 8))

		line := prefix + nameS + "  " + installed + "→ " + latest + "  " + ptype
		if sel && m.installing {
			line += "  " + warn.Render("updating…")
		}
		sb.WriteString(line + "\n")
	}

	sb.WriteString("\n  " + dim.Render("[↑↓] navigate  [enter/u] update  [r] refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) contentWidth() int {
	if m.width > 12 {
		return m.width - 4
	}
	return 76
}

// ── Helpers ───────────────────────────────────────────────────────────────────

func pmFilterUninstalled(entries []ipc.RegistryEntry) []ipc.RegistryEntry {
	out := make([]ipc.RegistryEntry, 0, len(entries))
	for _, e := range entries {
		if !e.Installed {
			out = append(out, e)
		}
	}
	return out
}

// pmIsNewer returns true when semver string a is strictly greater than b.
// Falls back to string comparison for non-numeric segments.
func pmIsNewer(a, b string) bool {
	ap := strings.Split(strings.TrimPrefix(a, "v"), ".")
	bp := strings.Split(strings.TrimPrefix(b, "v"), ".")
	for i := 0; i < len(ap) && i < len(bp); i++ {
		ai, errA := strconv.Atoi(ap[i])
		bi, errB := strconv.Atoi(bp[i])
		if errA != nil || errB != nil {
			// Non-numeric: lexicographic
			if ap[i] > bp[i] {
				return true
			}
			if ap[i] < bp[i] {
				return false
			}
			continue
		}
		if ai > bi {
			return true
		}
		if ai < bi {
			return false
		}
	}
	return len(ap) > len(bp)
}
