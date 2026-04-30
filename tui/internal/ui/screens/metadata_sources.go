package screens

// metadata_sources.go — Editable per-kind metadata source list.
//
// The runtime maintains four priority lists (one per TUI tab kind:
// movies/series/anime/music) plus a parallel set of disabled lists.
// On the detail card, the metadata fan-out walks priority → discovered
// (manifest-tagged for the kind) → minus disabled. THIS screen lets
// the user see who's contributing to each kind and disable plugins
// they don't want.
//
// Flow:
//   - Open with the Music tab selected by default (most volatile).
//     Press tab/shift-tab or 1-4 to switch kinds; the screen re-queries
//     the runtime for that kind's plugin list each time.
//   - j/k navigates rows. The cursor moves through priority entries,
//     then discovered, then disabled — all in one combined list.
//   - d disables the cursor plugin (moves it into the disabled list,
//     out of priority+discovered). e re-enables (removes from disabled).
//   - Each toggle emits a SettingsChangedMsg with the new disabled
//     list, which the root model forwards to the runtime via
//     SetConfig (key = "metadata_sources.<kind>_disabled"). Persisted
//     to runtime.toml; effective immediately for the next detail-card
//     open.
//
//   esc/q  close

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// OpenMetadataSourcesMsg is emitted by the settings screen to open this view.
type OpenMetadataSourcesMsg struct{}

var metadataKinds = []string{"music", "movies", "series", "anime"}

// rowStatus tags a plugin row so the renderer can pick the right
// visual treatment (priority chip / discovered chip / strikethrough
// for disabled).
type rowStatus int

const (
	rowPriority rowStatus = iota
	rowDiscovered
	rowDisabled
)

type metaSourceRow struct {
	plugin string
	status rowStatus
	// Position in the priority list (1-based) when status == rowPriority.
	// Used to render the "1." / "2." prefix without recomputing.
	priorityIdx int
}

// ── Screen ────────────────────────────────────────────────────────────────────

type MetadataSourcesScreen struct {
	Dims
	client *ipc.Client

	// Currently selected kind. Drives the IPC query and the row list
	// below. Starts at "music" so user-installed rating plugins (the
	// most common third-party kind) get attention first; tab/1-4
	// switches.
	kind string

	// Snapshot from the runtime — refreshed every time `kind` changes
	// or a toggle commits (so the screen reflects the new state).
	priority   []string
	discovered []string
	disabled   []string

	cursor int
	rows   []metaSourceRow

	// loading=true while a MetadataPluginsForKind round-trip is in
	// flight. Suppresses the row list to avoid showing stale data
	// during the brief gap.
	loading bool
	err     string
}

// NewMetadataSourcesScreen builds the editor and queues the initial
// query for the default kind. The IPC client is required — without it
// the screen has nothing to fetch and shows an error placeholder.
func NewMetadataSourcesScreen(client *ipc.Client) MetadataSourcesScreen {
	return MetadataSourcesScreen{
		client:  client,
		kind:    "music",
		loading: true,
	}
}

// Init kicks off the first MetadataPluginsForKind query so the screen
// has data to render after the framework calls View().
func (m MetadataSourcesScreen) Init() tea.Cmd {
	if m.client == nil {
		return nil
	}
	kind := m.kind
	client := m.client
	return func() tea.Msg {
		client.MetadataPluginsForKind(kind)
		return nil
	}
}

func (m MetadataSourcesScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.setWindowSize(msg)
		return m, nil

	case ipc.MetadataPluginsForKindMsg:
		// Only consume responses for the currently selected kind —
		// tab-switches in flight could deliver stale data otherwise.
		if msg.Kind != m.kind {
			return m, nil
		}
		m.loading = false
		if msg.Err != nil {
			m.err = msg.Err.Error()
			m.priority = nil
			m.discovered = nil
			m.disabled = nil
			m.rows = nil
			return m, nil
		}
		m.err = ""
		m.priority = msg.Priority
		m.discovered = msg.Discovered
		m.disabled = msg.Disabled
		m.rebuildRows()
		if m.cursor >= len(m.rows) {
			m.cursor = max(0, len(m.rows)-1)
		}
		return m, nil

	case tea.KeyPressMsg:
		switch msg.String() {
		case "q", "esc":
			return m, screen.PopCmd()
		case "tab":
			return m, m.switchKind(nextKind(m.kind, +1))
		case "shift+tab":
			return m, m.switchKind(nextKind(m.kind, -1))
		case "1":
			return m, m.switchKind("music")
		case "2":
			return m, m.switchKind("movies")
		case "3":
			return m, m.switchKind("series")
		case "4":
			return m, m.switchKind("anime")
		case "j", "down":
			if m.cursor < len(m.rows)-1 {
				m.cursor++
			}
			return m, nil
		case "k", "up":
			if m.cursor > 0 {
				m.cursor--
			}
			return m, nil
		case "J", "shift+down":
			return m.movePriority(+1)
		case "K", "shift+up":
			return m.movePriority(-1)
		case "d":
			return m.toggle(true)
		case "e":
			return m.toggle(false)
		}
	}
	return m, nil
}

func (m MetadataSourcesScreen) View() tea.View {
	neon := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	bold := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	hi := lipgloss.NewStyle().Foreground(theme.T.Bg()).Background(theme.T.Accent())
	off := lipgloss.NewStyle().Foreground(theme.T.TextDim()).Italic(true)
	chip := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	chipPri := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	chipDis := lipgloss.NewStyle().Foreground(theme.T.Red()).Italic(true)

	title := neon.Render("◆  Metadata Sources")
	sub := dim.Render("per-kind plugin fan-out · live-pushed to runtime")

	// Tab bar. The active kind gets a highlight; the others stay dim
	// so the user can see the layout without losing focus.
	var tabs []string
	for _, k := range metadataKinds {
		label := strings.ToUpper(k)
		if k == m.kind {
			tabs = append(tabs, hi.Padding(0, 1).Render(label))
		} else {
			tabs = append(tabs, dim.Padding(0, 1).Render(label))
		}
	}
	tabBar := strings.Join(tabs, "  ")

	intro := dim.Render(
		"Plugins below contribute to the detail-card metadata fan-out\n  " +
			"for the selected kind. Priority items are consulted first,\n  " +
			"then auto-discovered plugins (manifest-tagged), minus disabled.",
	)

	var body string
	switch {
	case m.err != "":
		body = lipgloss.NewStyle().Foreground(theme.T.Red()).Render("  " + m.err)
	case m.loading:
		body = dim.Render("  Loading…")
	case len(m.rows) == 0:
		body = dim.Render("  No plugins contribute to this kind.")
	default:
		var lines []string
		maxNameW := 0
		for _, r := range m.rows {
			if w := lipgloss.Width(r.plugin); w > maxNameW {
				maxNameW = w
			}
		}
		if maxNameW < 18 {
			maxNameW = 18
		}
		for i, r := range m.rows {
			var prefix, status string
			switch r.status {
			case rowPriority:
				prefix = fmt.Sprintf("%2d.", r.priorityIdx)
				status = chipPri.Render("[priority]")
			case rowDiscovered:
				prefix = "  ↪"
				status = chip.Render("[discovered]")
			case rowDisabled:
				prefix = "   "
				status = chipDis.Render("[disabled]")
			}
			nameStyled := lipgloss.NewStyle().Width(maxNameW).Render(r.plugin)
			line := fmt.Sprintf("  %s  %s  %s",
				prefix,
				bold.Render(nameStyled),
				status,
			)
			if r.status == rowDisabled {
				line = off.Render(line)
			}
			if i == m.cursor {
				line = hi.Render(line)
			}
			lines = append(lines, line)
		}
		body = strings.Join(lines, "\n")
	}

	keysHint := dim.Render(
		"↑/↓ select   shift+↑/↓ reorder priority   tab switch kind   d disable   e enable   q close",
	)

	header := lipgloss.JoinHorizontal(lipgloss.Top, title, "   ", sub)
	return tea.NewView("\n  " + header +
		"\n\n  " + tabBar +
		"\n\n  " + intro +
		"\n\n" + body +
		"\n\n  " + keysHint)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

// rebuildRows assembles the combined view from the three lists. Order:
// priority entries (in priority order), discovered (registry order),
// then disabled (alphabetical-ish — runtime returns insertion order).
func (m *MetadataSourcesScreen) rebuildRows() {
	rows := make([]metaSourceRow, 0, len(m.priority)+len(m.discovered)+len(m.disabled))
	for i, p := range m.priority {
		rows = append(rows, metaSourceRow{
			plugin:      p,
			status:      rowPriority,
			priorityIdx: i + 1,
		})
	}
	for _, p := range m.discovered {
		rows = append(rows, metaSourceRow{plugin: p, status: rowDiscovered})
	}
	for _, p := range m.disabled {
		rows = append(rows, metaSourceRow{plugin: p, status: rowDisabled})
	}
	m.rows = rows
}

// switchKind queues an IPC query for the new kind and resets transient
// view state so the user sees a Loading marker rather than stale rows
// during the round-trip.
func (m *MetadataSourcesScreen) switchKind(k string) tea.Cmd {
	if k == m.kind {
		return nil
	}
	m.kind = k
	m.loading = true
	m.err = ""
	m.priority = nil
	m.discovered = nil
	m.disabled = nil
	m.rows = nil
	m.cursor = 0
	if m.client == nil {
		return nil
	}
	client := m.client
	kind := k
	return func() tea.Msg {
		client.MetadataPluginsForKind(kind)
		return nil
	}
}

// toggle moves the cursor plugin into or out of the disabled list and
// emits a SettingsChangedMsg carrying the updated list. The runtime
// applies the change atomically and persists; we re-fetch immediately
// so the screen reflects the post-toggle state without guessing.
//
// `disable=true` adds to disabled (idempotent if already there);
// `disable=false` removes from disabled.
func (m MetadataSourcesScreen) toggle(disable bool) (screen.Screen, tea.Cmd) {
	if m.cursor < 0 || m.cursor >= len(m.rows) {
		return m, nil
	}
	plugin := m.rows[m.cursor].plugin
	updated := make([]string, 0, len(m.disabled)+1)
	for _, d := range m.disabled {
		if d != plugin {
			updated = append(updated, d)
		}
	}
	if disable {
		updated = append(updated, plugin)
	}
	m.disabled = updated
	m.rebuildRows()

	// Snapshot to ship in the SettingsChangedMsg. Key encodes which
	// kind's disabled list to update (matches the server-side
	// `apply_metadata_sources_key` switch).
	snapshot := append([]string(nil), updated...)
	key := fmt.Sprintf("metadata_sources.%s_disabled", m.kind)
	return m, func() tea.Msg {
		return SettingsChangedMsg{Key: key, Value: snapshot}
	}
}

func nextKind(cur string, delta int) string {
	idx := 0
	for i, k := range metadataKinds {
		if k == cur {
			idx = i
			break
		}
	}
	idx = (idx + delta + len(metadataKinds)) % len(metadataKinds)
	return metadataKinds[idx]
}

// movePriority swaps the cursor's plugin with its neighbour in the
// priority list (delta=-1 to move up, +1 to move down). Only valid
// when the cursor sits on a priority row — discovered/disabled rows
// have no fixed order, so the keystroke is a no-op there. Each
// successful move emits a SettingsChangedMsg with the full new
// priority list under key `metadata_sources.<kind>`, which the
// runtime persists to runtime.toml and applies to the next
// detail-card fan-out.
func (m MetadataSourcesScreen) movePriority(delta int) (screen.Screen, tea.Cmd) {
	if m.cursor < 0 || m.cursor >= len(m.rows) {
		return m, nil
	}
	row := m.rows[m.cursor]
	if row.status != rowPriority {
		return m, nil
	}
	// row.priorityIdx is 1-based; convert to 0-based for the slice.
	i := row.priorityIdx - 1
	j := i + delta
	if j < 0 || j >= len(m.priority) {
		return m, nil
	}
	m.priority[i], m.priority[j] = m.priority[j], m.priority[i]
	m.rebuildRows()
	// Cursor follows the moved row so the user can keep nudging.
	m.cursor += delta

	snapshot := append([]string(nil), m.priority...)
	key := fmt.Sprintf("metadata_sources.%s", m.kind)
	return m, func() tea.Msg {
		return SettingsChangedMsg{Key: key, Value: snapshot}
	}
}
