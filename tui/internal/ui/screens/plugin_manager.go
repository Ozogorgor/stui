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

	"charm.land/bubbles/v2/spinner"
	"charm.land/bubbles/v2/table"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
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
	Dims
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


	// Spinners
	pluginsSpinner  components.Spinner
	registrySpinner components.Spinner

	// Tables
	installedTable *components.SortableTable
	availableTable *components.SortableTable
	updatesTable   *components.SortableTable
}

func NewPluginManagerScreen(client *ipc.Client) *PluginManagerScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	installedCols := []table.Column{
		{Title: "Name", Width: 22},
		{Title: "Version", Width: 9},
		{Title: "Type", Width: 8},
		{Title: "Tags", Width: 15},
		{Title: "Status", Width: 12},
	}

	availableCols := []table.Column{
		{Title: "Name", Width: 22},
		{Title: "Version", Width: 9},
		{Title: "Type", Width: 8},
		{Title: "Status", Width: 15},
	}

	updatesCols := []table.Column{
		{Title: "Name", Width: 22},
		{Title: "Installed", Width: 9},
		{Title: "Latest", Width: 9},
		{Title: "Type", Width: 10},
	}

	return &PluginManagerScreen{
		client:          client,
		plLoading:       true,
		regLoading:      true,
		pluginsSpinner:  *components.NewSpinner("loading installed plugins…", dimStyle),
		registrySpinner: *components.NewSpinner("fetching plugin registry…", dimStyle),
		installedTable:  components.NewSortableTable(installedCols),
		availableTable:  components.NewSortableTable(availableCols),
		updatesTable:    components.NewSortableTable(updatesCols),
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m *PluginManagerScreen) Init() tea.Cmd {
	m.pluginsSpinner.Start()
	m.registrySpinner.Start()
	return func() tea.Msg {
		m.client.ListPlugins()
		m.client.BrowseRegistry()
		return nil
	}
}

func (m *PluginManagerScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case spinner.TickMsg:
		m.pluginsSpinner.Update(msg)
		m.registrySpinner.Update(msg)
		return m, nil

	case tea.WindowSizeMsg:
		m.setWindowSize(msg)

	case ipc.PluginListMsg:
		m.plLoading = false
		m.pluginsSpinner.Stop()
		if msg.Err != nil {
			m.status = "Error loading plugins: " + msg.Err.Error()
		} else {
			m.plugins = msg.Plugins
			m.updateInstalledTable()
		}
		m.recomputeUpdates()

	case ipc.RegistryBrowseResultMsg:
		m.regLoading = false
		m.registrySpinner.Stop()
		if msg.Err != nil {
			m.status = "Registry error: " + msg.Err.Error()
		} else {
			m.registry = msg.Entries
			m.failedRepos = msg.FailedRepos
			m.available = pmFilterUninstalled(msg.Entries)
			m.updateAvailableTable()
		}
		m.recomputeUpdates()
		m.updateUpdatesTable()

	case ipc.PluginInstallResultMsg:
		m.installing = false
		if msg.Err != nil {
			m.status = "Install failed: " + msg.Err.Error()
		} else {
			m.status = fmt.Sprintf("'%s' v%s installed — refreshing…", msg.Name, msg.Version)
			// Refresh both lists to reflect the new state.
			m.plLoading = true
			m.regLoading = true
			m.client.ListPlugins()
			m.client.BrowseRegistry()
		}

	case tea.KeyPressMsg:
		return m.handleKey(msg)
	}
	return m, nil
}

func (m *PluginManagerScreen) handleKey(msg tea.KeyPressMsg) (screen.Screen, tea.Cmd) {
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
		if len(m.plugins) > 0 && m.plCursor < len(m.plugins) && !m.plLoading {
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
		m.plLoading = true
		m.regLoading = true
		m.status = ""
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

func (m *PluginManagerScreen) View() tea.View {
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
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
			sb.WriteString(acc.Render("[" + label + "]"))
		} else {
			sb.WriteString(dim.Render(" " + label + " "))
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
	if m.installing {
		spinnerView := m.pluginsSpinner.View()
		pb := components.NewProgressBar(0.5, 1,
			components.WithWidth(20),
			components.WithShowValue(false),
		)
		installingStyle := lipgloss.NewStyle().Foreground(theme.T.Neon())
		sb.WriteString("\n  " + installingStyle.Render(spinnerView) + " " + pb.View() + "\n")
		if m.status != "" {
			sb.WriteString("  " + dim.Render(m.status) + "\n")
		}
	} else if m.status != "" {
		style := neon
		if strings.HasPrefix(m.status, "Error") || strings.HasPrefix(m.status, "Install failed") {
			style = warn
		}
		sb.WriteString("\n  " + style.Render(m.status) + "\n")
	}

	// ── Footer ────────────────────────────────────────────────────────────
	sb.WriteString("\n" + hintBar("tab/shift+tab switch tab", "R repos", "esc close") + "\n")
	return tea.NewView(sb.String())
}

func (m *PluginManagerScreen) viewInstalled() string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	var sb strings.Builder

	if m.plLoading {
		sb.WriteString("  " + m.pluginsSpinner.View() + "\n")
		return sb.String()
	}
	if len(m.plugins) == 0 {
		sb.WriteString("\n" + theme.T.EmptyStateStyle("📦", "No plugins loaded", "Press 'Tab' to browse available plugins") + "\n")
		return sb.String()
	}

	availH := m.height - 10
	if availH < 1 {
		availH = 20
	}

	m.installedTable.SetHeight(availH)
	m.installedTable.SetFocused(true)

	tableView := m.installedTable.View()

	sb.WriteString(dim.Render(tableView))
	sb.WriteString("\n  " + theme.T.KeyHint("↑↓", "navigate") + "  " + theme.T.KeyHint("u", "unload") + "  " + theme.T.KeyHint("r", "refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) viewAvailable() string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	var sb strings.Builder

	if m.regLoading {
		sb.WriteString("  " + m.registrySpinner.View() + "\n")
		return sb.String()
	}
	if len(m.failedRepos) > 0 {
		sb.WriteString("  " + theme.T.WarnPill("⚠ "+fmt.Sprintf("%d repo(s) unreachable", len(m.failedRepos))) + "\n\n")
	}
	if len(m.available) == 0 {
		sb.WriteString("  " + theme.T.SuccessPill("✓ All available plugins are already installed") + "\n")
		return sb.String()
	}

	availH := m.height - 10
	if availH < 1 {
		availH = 20
	}

	m.availableTable.SetHeight(availH)
	m.availableTable.SetFocused(true)

	tableView := m.availableTable.View()

	sb.WriteString(dim.Render(tableView))
	sb.WriteString("\n  " + theme.T.KeyHint("↑↓", "navigate") + "  " + theme.T.KeyHint("enter", "install") + "  " + theme.T.KeyHint("r", "refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) viewUpdates() string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	var sb strings.Builder

	if m.plLoading || m.regLoading {
		sb.WriteString("  " + dim.Render("Checking for updates…") + "\n")
		return sb.String()
	}
	if len(m.updates) == 0 {
		sb.WriteString("  " + theme.T.SuccessPill("✓ All plugins are up to date") + "\n")
		return sb.String()
	}

	availH := m.height - 10
	if availH < 1 {
		availH = 20
	}

	m.updatesTable.SetHeight(availH)
	m.updatesTable.SetFocused(true)

	tableView := m.updatesTable.View()

	sb.WriteString(dim.Render(tableView))
	if m.installing {
		sb.WriteString("  " + theme.T.WarnPill("updating…") + "\n")
	}
	sb.WriteString("\n  " + theme.T.KeyHint("↑↓", "navigate") + "  " + theme.T.KeyHint("u", "update") + "  " + theme.T.KeyHint("r", "refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) updateInstalledTable() {
	rows := make([][]string, len(m.plugins))
	for i, p := range m.plugins {
		tags := ""
		if len(p.Tags) > 0 {
			tags = strings.Join(p.Tags, ",")
		}
		rows[i] = []string{
			truncate(p.Name, 22),
			truncate(p.Version, 9),
			truncate(p.PluginType, 8),
			truncate(tags, 15),
			p.Status,
		}
	}
	m.installedTable.SetData(rows)
}

func (m *PluginManagerScreen) updateAvailableTable() {
	rows := make([][]string, len(m.available))
	for i, e := range m.available {
		status := "available"
		if e.Installed {
			status = "installed"
		}
		rows[i] = []string{
			truncate(e.Name, 22),
			truncate(e.Version, 9),
			truncate(e.PluginType, 8),
			status,
		}
	}
	m.availableTable.SetData(rows)
}

func (m *PluginManagerScreen) updateUpdatesTable() {
	rows := make([][]string, len(m.updates))
	for i, u := range m.updates {
		rows[i] = []string{
			truncate(u.plugin.Name, 22),
			truncate(u.plugin.Version, 9),
			truncate(u.entry.Version, 9),
			truncate(u.entry.PluginType, 10),
		}
	}
	m.updatesTable.SetData(rows)
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
