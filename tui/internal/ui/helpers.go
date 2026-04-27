// helpers.go — small cross-cutting utility functions used by the
// ui controller. These have no Bubbletea-flow dependencies and are
// pulled out of ui.go so the package's framework-shaped files stay
// focused on Init/Update/View/keys.

package ui

import (
	"time"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/session"
)

func getIfKey(key, target, value string) *string {
	if key == target {
		return &value
	}
	return nil
}

func (m *Model) providersForTab() []string {
	seen := map[string]bool{}
	var out []string
	for _, e := range m.currentGridEntries() {
		if !seen[e.Provider] {
			seen[e.Provider] = true
			out = append(out, e.Provider)
		}
	}
	return out
}

func (m *Model) switchTab(t state.Tab) {
	m.state.ActiveTab = t
	m.state.Cursor = 0
	m.state.Results = nil
	m.gridCursor = screens.GridCursor{}
	m.cwCursor = 0
	// Set cwFocused if the new tab has in-progress items
	if m.historyStore != nil && cwTabActive(t) &&
		len(cwItems(m.historyStore, t.MediaTabID())) > 0 {
		m.cwFocused = true
	} else {
		m.cwFocused = false
	}
	m.screen = screenGrid
	m.detail = nil
	if !m.state.CurrentStream.IsSet() {
		m.state.CurrentMedia = state.CurrentMedia{}
	}
	m.state.StatusMsg = t.String()
	// Collections is local-only — no runtime grid to load.
	if t != state.TabCollections && len(m.grids[t.MediaTabID()]) == 0 {
		m.state.IsLoading = true
		m.state.LoadingStart = time.Now().Unix()
	}
	// Persist the tab choice immediately (pointer receiver — mutation is visible to caller).
	_ = session.Save(m.sessionPath, session.State{
		LastTab:         t.String(),
		LastMusicSubTab: int(m.musicScreen.ActiveSubTab()),
		QueueURIs:       m.lastQueueURIs,
	})
}

// focusedSearchable returns the screens.Searchable for the currently-active
// screen, or nil if the focused screen does not implement Searchable.
//
// Routing table:
//
//   - TabMusic: delegates to MusicScreen.FocusedSearchable(), which returns
//     the active sub-tab's Searchable (Tasks 6.2/6.3).
//   - TabMovies / TabSeries / TabLibrary: grids adopt Searchable via the
//     gridSearchable adapter (Task 6.4). Results stream back through
//     gridScopeAppliedMsg into Model.Update, which writes to m.grids.
//   - TabCollections: not Searchable.
//
// The root model uses this to gate the `/` keystroke and to route
// ipc.ScopeResultsMsg / ipc.MpdSearchResult to the right screen.
func focusedSearchable(m *Model) screens.Searchable {
	switch m.state.ActiveTab {
	case state.TabMusic:
		return m.musicScreen.FocusedSearchable()
	case state.TabMovies, state.TabSeries, state.TabLibrary:
		return gridSearchable{m: m, tab: m.state.ActiveTab}
	}
	return nil
}

// applyRestoreView calls RestoreView on the active Searchable sub-screen
// (routed through the concrete screen holder) and writes the updated
// screen back into the correct typed field on the model.
// It must be called before modifying Focus / SearchActive so that the
// restore can observe the pre-transition state.
//
// For grid tabs (Movies/Series/Library), the adapter has no persistent
// state of its own — the snapshot lives on Model. Restoring means writing
// the saved snapshot back into m.grids[tab] and clearing bookkeeping.
func (m *Model) applyRestoreView() {
	switch m.state.ActiveTab {
	case state.TabMusic:
		m.musicScreen = m.musicScreen.ApplyRestoreView()
	case state.TabMovies, state.TabSeries, state.TabLibrary:
		tab := m.state.ActiveTab
		if snap, has := m.gridSearchSnapshot[tab]; has {
			m.grids[tab.MediaTabID()] = snap
			delete(m.gridSearchSnapshot, tab)
		}
		delete(m.gridSearchActiveQID, tab)
	}
}

func (m Model) currentGridEntries() []ipc.CatalogEntry {
	if entries, ok := m.grids[m.state.ActiveTab.MediaTabID()]; ok {
		return entries
	}
	return nil
}

// innerWidth returns the usable content width inside MainCardStyle
// (terminal width minus margins, border, and padding: 1+1+1+1+1+1 = 6).
// Floored at 0 to prevent negative dimensions on tiny terminals.
func (m Model) innerWidth() int {
	return max(0, m.state.Width-6)
}

// ── Data conversion helpers ───────────────────────────────────────────────────

func convertSearchToCatalog(items []ipc.MediaEntry) []ipc.CatalogEntry {
	out := make([]ipc.CatalogEntry, 0, len(items))
	for _, item := range items {
		out = append(out, ipc.CatalogEntry{
			ID:       item.ID,
			Title:    item.Title,
			Year:     item.Year,
			Genre:    item.Genre,
			Rating:   item.Rating,
			Provider: item.Provider,
			Tab:      string(item.Tab),
		})
	}
	return out
}

func listResultToCatalogEntry(r state.ResultItem, tab string) ipc.CatalogEntry {
	y, g, rt := r.Year, r.Genre, r.Rating
	return ipc.CatalogEntry{
		ID: r.ID, Title: r.Title,
		Year: &y, Genre: &g, Rating: &rt,
		Provider: r.Provider, Tab: tab,
	}
}

func derefStr(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func truncate(s string, maxLen int) string {
	if maxLen <= 0 {
		return ""
	}
	runes := []rune(s)
	if len(runes) <= maxLen {
		return s
	}
	if maxLen <= 3 {
		return string(runes[:maxLen])
	}
	return string(runes[:maxLen-1]) + "\u2026"
}
