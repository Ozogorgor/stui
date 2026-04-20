package ui

// grid_search.go — Searchable adapter and streaming plumbing for the
// Movies / Series / Library grid tabs.
//
// Grids are rendered inline from Model.grids (map[string][]ipc.CatalogEntry)
// rather than by a dedicated screen type, so there is no sub-model to carry
// Searchable state. Instead, gridSearchable is a thin value type that holds a
// reference back to *Model plus the tab it represents, and dispatches
// Client.Search while wiring the returned channel back through tea.Cmd
// messages that Model.Update handles directly.
//
// Design notes:
//
//   - gridSearchable has a value method-set. focusedSearchable builds one on
//     the fly per call; it does not survive across Update iterations.
//   - The pre-search snapshot lives on Model (gridSearchSnapshot[tab]) so
//     RestoreView can restore grid content after Esc / cleared-query without
//     re-fetching the catalog.
//   - Stale results are dropped by comparing gridSearchActiveQID[tab] with
//     the QueryID carried on each gridScopeAppliedMsg.
//   - The Library tab requests both Movie and Series scopes and accumulates
//     results across both; Movies/Series have a single scope and overwrite.

import (
	"context"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
)

// gridSearchable is the screens.Searchable adapter for the Movies, Series,
// and Library grid tabs. One instance is constructed per focusedSearchable
// call — it carries only a *Model reference and the tab identity.
type gridSearchable struct {
	m   *Model
	tab state.Tab
}

// SearchScopes returns the scope set for the tab: [Movie] for Movies,
// [Series] for Series, [Movie, Series] for Library (which aggregates both
// under a single view).
func (g gridSearchable) SearchScopes() []ipc.SearchScope {
	switch g.tab {
	case state.TabMovies:
		return []ipc.SearchScope{ipc.ScopeMovie}
	case state.TabSeries:
		return []ipc.SearchScope{ipc.ScopeSeries}
	case state.TabLibrary:
		return []ipc.SearchScope{ipc.ScopeMovie, ipc.ScopeSeries}
	}
	return nil
}

// SearchPlaceholder returns the placeholder text for the top-bar input
// while this tab is focused.
func (g gridSearchable) SearchPlaceholder() string {
	switch g.tab {
	case state.TabMovies:
		return "Search movies\u2026"
	case state.TabSeries:
		return "Search series\u2026"
	case state.TabLibrary:
		return "Search library\u2026"
	}
	return "Search\u2026"
}

// StartSearch dispatches a scoped Client.Search and returns the first
// read-from-channel tea.Cmd. The streaming loop is driven by
// gridScopeAppliedMsg → its Followup cmd continues draining the channel
// until it closes (gridSearchClosedMsg).
//
// The first call for a given tab captures a snapshot of the current grid
// entries so RestoreView can restore it later.
func (g gridSearchable) StartSearch(query string) tea.Cmd {
	if query == "" || g.m == nil || g.m.client == nil {
		return nil
	}

	// Capture the pre-search snapshot once per tab search session.
	if g.m.gridSearchSnapshot == nil {
		g.m.gridSearchSnapshot = make(map[state.Tab][]ipc.CatalogEntry)
	}
	if _, has := g.m.gridSearchSnapshot[g.tab]; !has {
		existing := g.m.grids[g.tab.MediaTabID()]
		before := make([]ipc.CatalogEntry, len(existing))
		copy(before, existing)
		g.m.gridSearchSnapshot[g.tab] = before
	}

	scopes := g.SearchScopes()
	qid, ch, err := g.m.client.Search(context.Background(), query, scopes)
	if err != nil {
		return func() tea.Msg { return gridSearchFailedMsg{Tab: g.tab, Err: err} }
	}

	if g.m.gridSearchActiveQID == nil {
		g.m.gridSearchActiveQID = make(map[state.Tab]uint64)
	}
	g.m.gridSearchActiveQID[g.tab] = qid

	// For multi-scope tabs (Library), clear accumulated results so prior
	// scope data from a previous query does not bleed through. Snapshot was
	// already captured above, so RestoreView can still recover.
	g.m.grids[g.tab.MediaTabID()] = nil

	return readNextGridScope(ch, g.tab, qid)
}

// gridScopeAppliedMsg carries one streamed scope result for a grid tab.
// Followup is the tea.Cmd that reads the next message from the underlying
// channel; it is nil only when the channel is observed closed (in which
// case gridSearchClosedMsg is emitted instead).
type gridScopeAppliedMsg struct {
	Tab      state.Tab
	QueryID  uint64
	Scope    ipc.SearchScope
	Entries  []ipc.MediaEntry
	Partial  bool
	Followup tea.Cmd
}

// gridSearchClosedMsg signals the scope-results channel has closed — all
// requested scopes have finalized (partial=false) or the runtime rolled the
// query off the subscription map.
type gridSearchClosedMsg struct {
	Tab     state.Tab
	QueryID uint64
}

// gridSearchFailedMsg carries a dispatch or transport error for a grid
// search. Update surfaces this via the status bar.
type gridSearchFailedMsg struct {
	Tab state.Tab
	Err error
}

// readNextGridScope blocks on the scope-results channel and returns either a
// gridScopeAppliedMsg (with a Followup cmd that will read the next message)
// or a gridSearchClosedMsg when the channel closes. It is the loop driver
// for streaming grid search results.
func readNextGridScope(ch <-chan ipc.ScopeResultsMsg, tab state.Tab, qid uint64) tea.Cmd {
	return func() tea.Msg {
		msg, ok := <-ch
		if !ok {
			return gridSearchClosedMsg{Tab: tab, QueryID: qid}
		}
		return gridScopeAppliedMsg{
			Tab:      tab,
			QueryID:  msg.QueryID,
			Scope:    msg.Scope,
			Entries:  msg.Entries,
			Partial:  msg.Partial,
			Followup: readNextGridScope(ch, tab, qid),
		}
	}
}

// mediaEntriesToCatalog converts a slice of MediaEntry (the wire format
// used by scoped search) into CatalogEntry (the grid storage format).
// Description, PosterURL and imdb-relevant metadata are preserved so the
// grid renderer can show posters and details for search results exactly
// as it would for catalog entries.
func mediaEntriesToCatalog(items []ipc.MediaEntry) []ipc.CatalogEntry {
	out := make([]ipc.CatalogEntry, 0, len(items))
	for _, item := range items {
		out = append(out, ipc.CatalogEntry{
			ID:          item.ID,
			Title:       item.Title,
			Year:        item.Year,
			Genre:       item.Genre,
			Rating:      item.Rating,
			Description: item.Description,
			PosterURL:   item.PosterURL,
			Provider:    item.Provider,
			Tab:         string(item.Tab),
			Kind:        item.Kind,
			Source:      item.Source,
		})
	}
	return out
}

// scopeKind maps a SearchScope to its EntryKind counterpart. The string
// values are already aligned (scope.movie <-> kind.movie etc.) so a direct
// cast works.
func scopeKind(s ipc.SearchScope) ipc.EntryKind {
	return ipc.EntryKind(s)
}
