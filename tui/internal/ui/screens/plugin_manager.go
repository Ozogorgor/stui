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

// pmMenuOption is a single choice in the Enter-on-row action menu
// (installed tab). Label is what the user sees; invoke runs when
// the user picks it.
type pmMenuOption struct {
	label  string
	invoke func() (screen.Screen, tea.Cmd)
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

	// Confirmation prompt. When non-empty, key input routes to the
	// prompt resolver (y/n/esc) instead of normal navigation. One
	// prompt at a time — installed-tab actions (unload/update) are
	// the only callers today.
	confirmPrompt string             // text shown to the user
	confirmAction func() (screen.Screen, tea.Cmd)

	// Pending action menu (Enter on installed row). When non-zero
	// length, key input picks one of these options.
	pendingMenu    []pmMenuOption
	pendingMenuIdx int


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
		{Title: "Type", Width: 14},
		{Title: "Author", Width: 12},
		{Title: "Status", Width: 10},
	}

	availableCols := []table.Column{
		{Title: "Name", Width: 22},
		{Title: "Version", Width: 9},
		{Title: "Type", Width: 14},
		{Title: "Author", Width: 12},
		{Title: "Status", Width: 10},
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
	cmds := []tea.Cmd{
		m.pluginsSpinner.Init(),
		m.registrySpinner.Init(),
	}
	// Only fire the IPC calls when a client is actually wired. Opening the
	// plugin manager before `ClientReadyMsg` has landed (e.g. the user
	// mashes into Settings → Plugin Manager within the first ~50ms of
	// launch, before the runtime subprocess has dialed) means `m.client`
	// is still nil and dereffing it in the goroutine inside ListPlugins
	// segfaults on `atomic.Uint64.Add` against the nil Client pointer.
	if m.client != nil {
		cmds = append(cmds, func() tea.Msg {
			m.client.ListPlugins()
			m.client.BrowseRegistry()
			return nil
		})
	} else {
		m.plLoading = false
		m.regLoading = false
		m.pluginsSpinner.Stop()
		m.registrySpinner.Stop()
		m.status = "Runtime not ready — open Plugin Manager again once stui finishes starting."
	}
	return tea.Batch(cmds...)
}

func (m *PluginManagerScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case spinner.TickMsg:
		_, cmd1 := m.pluginsSpinner.Update(msg)
		_, cmd2 := m.registrySpinner.Update(msg)
		return m, tea.Batch(cmd1, cmd2)

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
			// Clear any lingering transient status (e.g. "installed —
			// refreshing…") now that the refresh actually landed.
			if !m.regLoading {
				m.status = ""
			}
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
			if !m.plLoading {
				m.status = ""
			}
		}
		m.recomputeUpdates()
		m.updateUpdatesTable()

	case ipc.PluginToastMsg:
		// The runtime broadcasts this when its filesystem watcher
		// finishes hot-loading a plugin (install path, manual drop,
		// whatever). Our own `ListPlugins()` request right after an
		// install response races the watcher and usually wins — the
		// Installed tab would show stale data until the user hit `r`.
		// Refreshing on the toast catches the load unconditionally.
		if m.client != nil && !msg.IsError {
			m.plLoading = true
			m.pluginsSpinner.Start()
			m.client.ListPlugins()
			// Registry too, since newly-loaded plugins get moved out
			// of the Available list.
			m.regLoading = true
			m.registrySpinner.Start()
			m.client.BrowseRegistry()
		}

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

	// ── Confirmation prompt active: y/n/esc only ──────────────────────
	if m.confirmPrompt != "" {
		switch key {
		case "y", "enter":
			action := m.confirmAction
			m.confirmPrompt = ""
			m.confirmAction = nil
			if action != nil {
				return action()
			}
			return m, nil
		case "n", "esc":
			m.confirmPrompt = ""
			m.confirmAction = nil
			return m, nil
		}
		return m, nil
	}

	// ── Action menu active (Enter on installed row): up/down/enter/esc ─
	if len(m.pendingMenu) > 0 {
		switch key {
		case "up", "k":
			if m.pendingMenuIdx > 0 {
				m.pendingMenuIdx--
			}
			return m, nil
		case "down", "j":
			if m.pendingMenuIdx < len(m.pendingMenu)-1 {
				m.pendingMenuIdx++
			}
			return m, nil
		case "enter":
			opt := m.pendingMenu[m.pendingMenuIdx]
			m.pendingMenu = nil
			m.pendingMenuIdx = 0
			if opt.invoke != nil {
				return opt.invoke()
			}
			return m, nil
		case "esc":
			m.pendingMenu = nil
			m.pendingMenuIdx = 0
			return m, nil
		}
		return m, nil
	}

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
		m.installedTable.SetCursor(m.plCursor)
	case "down", "j":
		if m.plCursor < len(m.plugins)-1 {
			m.plCursor++
		}
		m.installedTable.SetCursor(m.plCursor)
	case "u", "x":
		m.promptUnloadCursor()
	case "enter":
		m.openInstalledActionMenu()
	case "r":
		m.plLoading = true
		m.status = ""
		m.client.ListPlugins()
	}
	return m, nil
}

// promptUnloadCursor stages a y/n confirmation for uninstalling the
// plugin under the installed-tab cursor. On confirmation the actual
// UnloadPlugin RPC fires (which also deletes the plugin directory
// from disk) and both lists refresh so the row moves from Installed
// → Available without the user hitting `r`.
func (m *PluginManagerScreen) promptUnloadCursor() {
	if len(m.plugins) == 0 || m.plCursor >= len(m.plugins) || m.plLoading || m.client == nil {
		return
	}
	p := m.plugins[m.plCursor]
	m.confirmPrompt = fmt.Sprintf("Uninstall '%s' v%s?  (deletes the plugin from disk)\n\n  [y]es   [n]o", p.Name, p.Version)
	m.confirmAction = func() (screen.Screen, tea.Cmd) {
		m.client.UnloadPlugin(p.ID)
		// Clear the cached row immediately so the UI doesn't show
		// the just-uninstalled plugin while the refresh is in flight.
		m.status = fmt.Sprintf("Uninstalling '%s'…", p.Name)
		m.plLoading = true
		m.regLoading = true
		m.pluginsSpinner.Start()
		m.registrySpinner.Start()
		// The runtime doesn't push a toast on uninstall, so we have
		// to schedule both fetches ourselves. Without BrowseRegistry
		// the Available tab would keep showing Installed=true for
		// the unloaded entry until a manual `r`.
		m.client.ListPlugins()
		m.client.BrowseRegistry()
		return m, tea.Batch(m.pluginsSpinner.Init(), m.registrySpinner.Init())
	}
}

// openInstalledActionMenu builds the Enter-on-row menu. The options
// are context-dependent: an "Update to vX.Y.Z" choice appears only
// when the Updates tab sees a newer version for the cursor's plugin.
func (m *PluginManagerScreen) openInstalledActionMenu() {
	if len(m.plugins) == 0 || m.plCursor >= len(m.plugins) || m.plLoading || m.client == nil {
		return
	}
	p := m.plugins[m.plCursor]

	var opts []pmMenuOption

	// "Update" option first — primary action when available.
	if up, ok := m.findUpdateFor(p); ok {
		entry := up.entry
		opts = append(opts, pmMenuOption{
			label: fmt.Sprintf("Update to v%s", entry.Version),
			invoke: func() (screen.Screen, tea.Cmd) {
				m.installing = true
				m.status = fmt.Sprintf("Updating '%s' to v%s…", entry.Name, entry.Version)
				m.client.InstallPlugin(entry.Name, entry.Version, entry.BinaryURL, entry.Checksum)
				return m, nil
			},
		})
	}
	// Enable/Disable toggle — plugin stays on disk, just stops
	// participating in dispatch. Label flips based on current state
	// so there's always exactly one toggle action in the menu.
	if p.Enabled {
		opts = append(opts, pmMenuOption{
			label: "Disable",
			invoke: func() (screen.Screen, tea.Cmd) {
				m.client.SetPluginEnabled(p.ID, false)
				m.status = fmt.Sprintf("Disabling '%s'…", p.Name)
				m.plLoading = true
				m.pluginsSpinner.Start()
				// Re-arm the spinner's tick loop — Start() just
				// flips the active flag, it doesn't emit a tick.
				return m, m.pluginsSpinner.Init()
			},
		})
	} else {
		opts = append(opts, pmMenuOption{
			label: "Enable",
			invoke: func() (screen.Screen, tea.Cmd) {
				m.client.SetPluginEnabled(p.ID, true)
				m.status = fmt.Sprintf("Enabling '%s'…", p.Name)
				m.plLoading = true
				m.pluginsSpinner.Start()
				return m, m.pluginsSpinner.Init()
			},
		})
	}
	opts = append(opts, pmMenuOption{
		label: "Uninstall",
		invoke: func() (screen.Screen, tea.Cmd) {
			m.promptUnloadCursor()
			return m, nil
		},
	})
	opts = append(opts, pmMenuOption{label: "Cancel"})

	m.pendingMenu = opts
	m.pendingMenuIdx = 0
}

// findUpdateFor returns the registry's newer-version entry for p, if
// any, by consulting the previously-computed updates slice.
func (m *PluginManagerScreen) findUpdateFor(p ipc.PluginInfo) (updateCandidate, bool) {
	for _, u := range m.updates {
		if u.plugin.ID == p.ID {
			return u, true
		}
	}
	return updateCandidate{}, false
}

func (m *PluginManagerScreen) handleAvailableKey(key string) (screen.Screen, tea.Cmd) {
	switch key {
	case "up", "k":
		if m.avCursor > 0 {
			m.avCursor--
		}
		m.availableTable.SetCursor(m.avCursor)
	case "down", "j":
		if m.avCursor < len(m.available)-1 {
			m.avCursor++
		}
		m.availableTable.SetCursor(m.avCursor)
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
		m.updatesTable.SetCursor(m.upCursor)
	case "down", "j":
		if m.upCursor < len(m.updates)-1 {
			m.upCursor++
		}
		m.updatesTable.SetCursor(m.upCursor)
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
	// When a confirmation or action menu is pending, replace the tab
	// body with a centred dialog so the prompt is unmissable. Inline
	// prompts got lost below long plugin lists.
	if m.confirmPrompt != "" || len(m.pendingMenu) > 0 {
		sb.WriteString(m.renderDialog())
	} else {
		switch m.tab {
		case pmInstalled:
			sb.WriteString(m.viewInstalled())
		case pmAvailable:
			sb.WriteString(m.viewAvailable())
		case pmUpdates:
			sb.WriteString(m.viewUpdates())
		}
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
	var sb strings.Builder

	if m.plLoading {
		sb.WriteString("  " + m.pluginsSpinner.View() + "\n")
		return sb.String()
	}
	if len(m.plugins) == 0 {
		sb.WriteString("\n" + theme.T.EmptyStateStyle("📦", "No plugins loaded", "Press 'Tab' to browse available plugins") + "\n")
		return sb.String()
	}

	m.installedTable.SetHeight(pmTableHeight(len(m.plugins), m.height))
	m.installedTable.SetFocused(true)

	tableView := m.installedTable.View()

	sb.WriteString(tableView)
	sb.WriteString("\n")
	if m.plCursor < len(m.plugins) {
		p := m.plugins[m.plCursor]
		sb.WriteString(pluginDetail(p.Description, p.Author, p.Tags))
	}
	sb.WriteString("\n  " + theme.T.KeyHint("↑↓", "navigate") + "  " + theme.T.KeyHint("enter", "actions") + "  " + theme.T.KeyHint("u", "uninstall") + "  " + theme.T.KeyHint("r", "refresh") + "\n")
	return sb.String()
}

// renderDialog returns the centered confirm / menu dialog that floats
// above the tab body when a destructive action or action menu is
// pending. Returned as a standalone region composited by View() over
// the normal tab content — replacing the inline prompt that was
// hard to spot when the installed table ran long.
func (m *PluginManagerScreen) renderDialog() string {
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	warn := lipgloss.NewStyle().Foreground(theme.T.Warn()).Bold(true)

	var body strings.Builder

	if m.confirmPrompt != "" {
		body.WriteString(warn.Render("⚠  Confirm"))
		body.WriteString("\n\n")
		body.WriteString(m.confirmPrompt)
		body.WriteString("\n")
	} else if len(m.pendingMenu) > 0 {
		body.WriteString(acc.Render("Actions"))
		body.WriteString("\n\n")
		for i, opt := range m.pendingMenu {
			if i == m.pendingMenuIdx {
				body.WriteString(acc.Render("▸ " + opt.label))
			} else {
				body.WriteString("  " + dim.Render(opt.label))
			}
			body.WriteString("\n")
		}
		body.WriteString("\n")
		body.WriteString(dim.Render("↑↓ choose · enter confirm · esc cancel"))
	}

	// Border + padding, centred inside the plugin_manager overlay.
	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Background(theme.T.Surface()).
		Padding(1, 3).
		Render(body.String())

	// Reserve roughly the same vertical space the tab body would use
	// so the surrounding chrome (title, tabs, footer) doesn't shift
	// when the dialog opens.
	bodyH := m.height - 6
	if bodyH < 10 {
		bodyH = 10
	}
	bodyW := m.contentWidth() + 2
	if bodyW < 30 {
		bodyW = 30
	}
	return lipgloss.Place(bodyW, bodyH, lipgloss.Center, lipgloss.Center, box)
}


func (m *PluginManagerScreen) viewAvailable() string {
	var sb strings.Builder

	if m.regLoading {
		sb.WriteString("  " + m.registrySpinner.View() + "\n")
		return sb.String()
	}
	if len(m.available) == 0 {
		sb.WriteString("  " + theme.T.SuccessPill("✓ All available plugins are already installed") + "\n")
		if len(m.failedRepos) > 0 {
			sb.WriteString("\n  " + theme.T.WarnPill("⚠ "+fmt.Sprintf("%d repo(s) unreachable", len(m.failedRepos))) + "\n")
		}
		return sb.String()
	}

	m.availableTable.SetHeight(pmTableHeight(len(m.available), m.height))
	m.availableTable.SetFocused(true)

	tableView := m.availableTable.View()

	sb.WriteString(tableView)
	sb.WriteString("\n")
	if m.avCursor < len(m.available) {
		e := m.available[m.avCursor]
		// Registry entries (browse-tab) don't carry manifest tags —
		// the index sources only the manifest fields the registry
		// repo serialises. Pass nil to suppress the tag chip line.
		sb.WriteString(pluginDetail(e.Description, e.Author, nil))
	}
	// Render the unreachable-repos warning BELOW the table so it
	// doesn't push the plugin list downward on every open. It's
	// supplementary info — users care about what they can install,
	// which repos failed is secondary.
	if len(m.failedRepos) > 0 {
		sb.WriteString("  " + theme.T.WarnPill("⚠ "+fmt.Sprintf("%d repo(s) unreachable", len(m.failedRepos))) + "\n")
	}
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

	m.updatesTable.SetHeight(pmTableHeight(len(m.updates), m.height))
	m.updatesTable.SetFocused(true)

	tableView := m.updatesTable.View()

	sb.WriteString(tableView)
	sb.WriteString("\n")
	if m.installing {
		sb.WriteString("  " + theme.T.WarnPill("updating…") + "\n")
	}
	sb.WriteString("\n  " + theme.T.KeyHint("↑↓", "navigate") + "  " + theme.T.KeyHint("u", "update") + "  " + theme.T.KeyHint("r", "refresh") + "\n")
	return sb.String()
}

func (m *PluginManagerScreen) updateInstalledTable() {
	rows := make([][]string, len(m.plugins))
	for i, p := range m.plugins {
		status := p.Status
		if !p.Enabled {
			// Keep the word short to fit the 10-wide Status column
			// and distinct from "loaded" so disabled plugins are
			// obvious at a glance.
			status = "disabled"
		}
		rows[i] = []string{
			truncate(p.Name, 22),
			truncate(p.Version, 9),
			truncate(p.PluginType, 14),
			truncate(p.Author, 12),
			status,
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
			truncate(e.PluginType, 14),
			truncate(e.Author, 12),
			status,
		}
	}
	m.availableTable.SetData(rows)
}

// pluginDetail returns a dim one-liner with description, author, and
// manifest tags for the focused row. Tags surface here so plugin
// authors and users can see at a glance which kinds the plugin
// declared itself for — these tags drive the dynamic-discovery in
// the metadata sources screen (movies/series/anime/music).
func pluginDetail(desc, author string, tags []string) string {
	if desc == "" && author == "" && len(tags) == 0 {
		return ""
	}
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	chip := lipgloss.NewStyle().Foreground(theme.T.Accent())

	line := desc
	if author != "" {
		if line != "" {
			line += "  —  by " + author
		} else {
			line = "by " + author
		}
	}

	// First line: description + author, dim-styled and width-clamped.
	out := "  " + dim.Render(truncate(line, 72))

	// Second line: tag chips, accent-styled, only when present. Done
	// as a separate row so the tag list doesn't get truncated by the
	// 72-char description budget.
	if len(tags) > 0 {
		var chips []string
		for _, t := range tags {
			chips = append(chips, chip.Render("["+t+"]"))
		}
		out += "\n  " + dim.Render("tags: ") + strings.Join(chips, " ")
	}

	return out + "\n"
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

// pmTableHeight returns the table viewport height that shows every row at
// once when the overlay has the space, and caps at the overlay's available
// height minus the fixed chrome (title, tab bar, detail line, hints,
// footer ≈ 10 rows) otherwise.
//
// bubbles/table's `SetHeight(h)` sets the viewport to
// `h - lipgloss.Height(headersView)`. With `BorderBottom(true)` on the
// header style, the bordered header is exactly 2 lines, so
// `viewport_rows = h - 2`. The viewport itself pads its content to the
// declared viewport height with full-width spaces — any leftover budget
// becomes a padding row at the end that anything appended after the table
// (like pluginDetail) lands on, shunted to the far right of the screen.
//
// Returning exactly `rowCount + 2` produces `viewport_rows == rowCount`
// with no padding slack, so pluginDetail renders on its own line below.
//
// Prior behaviour fixed the height at `overlayHeight - 10`, which when the
// overlay was shorter than ~30 rows collapsed the viewport to 1-2 rows and
// forced arrow-key paging through the plugin list even when every plugin
// would comfortably fit on screen.
func pmTableHeight(rowCount, overlayHeight int) int {
	const headerLines = 2
	desired := rowCount + headerLines
	if desired < headerLines+1 {
		desired = headerLines + 1
	}
	maxH := overlayHeight - 10
	if maxH < headerLines+1 {
		return desired
	}
	if desired > maxH {
		return maxH
	}
	return desired
}

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
