// handlers_catalog.go — Update msg handlers for catalog browsing
// (grid updates, scope results, search results, episodes, detail
// open + metadata) plus their helpers.

package ui

import (
	"fmt"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components/poster"
	"github.com/stui/stui/internal/ui/screens"
)

// handlePostersUpdated handles poster.PostersUpdatedMsg.
func (m Model) handlePostersUpdated(msg poster.PostersUpdatedMsg) (tea.Model, tea.Cmd) {
	// Re-arm the poll so we keep listening. No model-state change —
	// the next View() pass picks up newly-cached posters directly.
	return m, pollPosterRefresh()
}

// handleCatalogStale handles ipc.CatalogStaleMsg.
func (m Model) handleCatalogStale(msg ipc.CatalogStaleMsg) (tea.Model, tea.Cmd) {
	// Runtime couldn't refresh this tab (all providers errored /
	// offline). Surface to the user so they know the grid isn't
	// freshly fetched data — existing cached entries stay on screen
	// because the runtime deliberately didn't overwrite them with
	// an empty result.
	tab := msg.Tab
	if tab == "" {
		tab = "catalog"
	}
	m.state.StatusMsg = fmt.Sprintf("⚠ Offline — showing cached %s", tab)
	return m, nil
}

// handleGridUpdate handles ipc.GridUpdateMsg.
func (m Model) handleGridUpdate(msg ipc.GridUpdateMsg) (tea.Model, tea.Cmd) {
	m.grids[msg.Tab] = msg.Entries
	m.musicScreen, _ = m.musicScreen.Update(msg) // keep Browse catalog fresh
	if msg.Tab == m.state.ActiveTab.MediaTabID() {
		m.state.IsLoading = false
		m.state.LoadingStart = 0
		// The runtime's catalog now skips refresh_tab when the cached
		// grid is still within TTL, so a cache-source GridUpdate is NOT
		// guaranteed to be followed by a live one. Showing "refreshing…"
		// here would leave the footbar stuck on that string when no
		// follow-up arrives. Plain "N titles" is correct in both cases.
		m.state.StatusMsg = fmt.Sprintf("%d titles", len(msg.Entries))
	}
	// Persist live catalog data for offline browsing.
	if msg.Source == "live" && m.mediaCache != nil {
		m.mediaCache.SaveTab(msg.Tab, msg.Entries)
	}
	return m, nil
}

// handleScopeResults handles ipc.ScopeResultsMsg.
func (m Model) handleScopeResults(msg ipc.ScopeResultsMsg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	m.musicScreen, cmd = m.musicScreen.ApplyScopeResults(msg)
	return m, cmd
}

// handleGridScopeApplied handles gridScopeAppliedMsg.
func (m Model) handleGridScopeApplied(msg gridScopeAppliedMsg) (tea.Model, tea.Cmd) {
	activeQID, ok := m.gridSearchActiveQID[msg.Tab]
	if !ok || activeQID != msg.QueryID {
		// Stale — a newer search superseded this one. Continue draining.
		return m, msg.Followup
	}
	converted := mediaEntriesToCatalog(msg.Entries)
	tabID := msg.Tab.MediaTabID()
	if msg.Tab == state.TabLibrary {
		// Library accumulates Movie + Series. Keep any entries from the
		// other scope, replace only the current-scope bucket.
		existing := m.grids[tabID]
		filtered := make([]ipc.CatalogEntry, 0, len(existing)+len(converted))
		targetKind := scopeKind(msg.Scope)
		for _, e := range existing {
			if e.Kind != targetKind {
				filtered = append(filtered, e)
			}
		}
		filtered = append(filtered, converted...)
		m.grids[tabID] = filtered
	} else {
		m.grids[tabID] = converted
	}
	return m, msg.Followup
}

// handleGridSearchClosed handles gridSearchClosedMsg.
func (m Model) handleGridSearchClosed(msg gridSearchClosedMsg) (tea.Model, tea.Cmd) {
	// Channel closed — all requested scopes finalized. No structural
	// change needed; the last gridScopeAppliedMsg already wrote the
	// final entries.
	return m, nil
}

// handleGridSearchFailed handles gridSearchFailedMsg.
func (m Model) handleGridSearchFailed(msg gridSearchFailedMsg) (tea.Model, tea.Cmd) {
	m.state.StatusMsg = fmt.Sprintf("Search error: %v", msg.Err)
	return m, nil
}

// handleSearchResult handles ipc.SearchResultMsg.
func (m Model) handleSearchResult(msg ipc.SearchResultMsg) (tea.Model, tea.Cmd) {
	m.state.IsLoading = false
	m.state.LoadingStart = 0
	if msg.Err != nil {
		m.state.StatusMsg = fmt.Sprintf("Search error: %v", msg.Err)
		return m, nil
	}
	if m.detail != nil && m.detail.PersonMode {
		m.detail.PersonResults = convertSearchToCatalog(msg.Result.Items)
		m.detail.PersonLoading = false
		m.detail.PersonCursor = screens.GridCursor{}
		return m, nil
	}
	return m, nil
}

// handleEpisodesLoaded handles ipc.EpisodesLoadedMsg.
func (m Model) handleEpisodesLoaded(msg ipc.EpisodesLoadedMsg) (tea.Model, tea.Cmd) {
	// Episode data arrived — forwarded automatically to EpisodeScreen via RootModel
	return m, nil
}

// handleCollectionOpenDetail handles screens.CollectionOpenDetailMsg.
func (m Model) handleCollectionOpenDetail(msg screens.CollectionOpenDetailMsg) (tea.Model, tea.Cmd) {
	return m, m.openDetail(msg.Entry)
}

// handleDetailReady handles ipc.DetailReadyMsg.
func (m Model) handleDetailReady(msg ipc.DetailReadyMsg) (tea.Model, tea.Cmd) {
	if m.detail == nil {
		return m, nil
	}
	if msg.Err != nil {
		m.detail.Loading = false
		m.state.StatusMsg = fmt.Sprintf("Detail error: %v", msg.Err)
		return m, nil
	}
	m.detail.Entry = msg.Entry
	m.detail.Loading = false
	m.state.StatusMsg = msg.Entry.Title
	return m, m.sendGetDetailMetadata(msg.Entry)
}

// handleDetailMetadataPartial handles ipc.DetailMetadataPartial.
func (m Model) handleDetailMetadataPartial(msg ipc.DetailMetadataPartial) (tea.Model, tea.Cmd) {
	// Streamed per-verb partial from GetDetailMetadata. Apply to the
	// live detail state if the user hasn't navigated away; mismatched
	// EntryIDs are silently ignored by ApplyMetadataPartial.
	if m.detail != nil {
		m.detail.ApplyMetadataPartial(msg)
	}
	return m, nil
}

// handleSearchDebounceFire handles searchDebounceFireMsg.
func (m Model) handleSearchDebounceFire(msg searchDebounceFireMsg) (tea.Model, tea.Cmd) {
	// Stale token means a newer keystroke has already queued a fresh tick;
	// drop this one without firing.
	if msg.token != m.searchDebounceToken {
		return m, nil
	}
	if s := focusedSearchable(&m); s != nil {
		query := m.search.Value()
		if query != "" {
			return m, s.StartSearch(query)
		}
		m.applyRestoreView()
	}
	return m, nil
}

// ── Detail opening ────────────────────────────────────────────────────────────

func (m *Model) openDetail(entry ipc.CatalogEntry) tea.Cmd {
	detail := ipc.DetailEntry{
		ID:          entry.ID,
		Title:       entry.Title,
		Year:        derefStr(entry.Year),
		Genre:       derefStr(entry.Genre),
		Rating:      derefStr(entry.Rating),
		Description: derefStr(entry.Description),
		PosterURL:   derefStr(entry.PosterURL),
		Provider:    entry.Provider,
		Tab:         entry.Tab,
		ImdbID:      derefStr(entry.ImdbID),
		Providers:   []string{entry.Provider},
	}
	ds := screens.NewDetailState(detail)
	// Populate watch history so the detail screen can show a resume hint.
	if m.historyStore != nil {
		ds.WatchHistory = m.historyStore.Get(entry.ID)
	}
	m.detail = &ds
	m.screen = screenDetail
	m.state.CurrentMedia = state.CurrentMedia{
		ID:       entry.ID,
		Title:    entry.Title,
		Year:     derefStr(entry.Year),
		Genre:    derefStr(entry.Genre),
		Rating:   derefStr(entry.Rating),
		Tab:      m.state.ActiveTab,
		Provider: entry.Provider,
		ImdbID:   derefStr(entry.ImdbID),
	}
	return m.fetchDetailMetadata(detail)
}

// formatDurationHMS converts seconds to a H:MM:SS or M:SS string.
func formatDurationHMS(secs float64) string {
	total := int(secs)
	h := total / 3600
	min := (total % 3600) / 60
	s := total % 60
	if h > 0 {
		return fmt.Sprintf("%d:%02d:%02d", h, min, s)
	}
	return fmt.Sprintf("%d:%02d", min, s)
}

// fetchDetailMetadata synthesises the initial DetailReadyMsg from the
// in-memory catalog entry. Until the runtime gains a dedicated detail
// endpoint (chunk 7+), this populates the overlay with whatever we
// already have; the four-verb fan-out filed by sendGetDetailMetadata
// streams back enrichments (studio, networks, credits, artwork,
// related) as they arrive via DetailMetadataPartial.
func (m *Model) fetchDetailMetadata(entry ipc.DetailEntry) tea.Cmd {
	tabProviders := m.providersForTab()
	return func() tea.Msg {
		if len(tabProviders) > 0 && len(entry.Providers) == 0 {
			entry.Providers = tabProviders
		}
		return ipc.DetailReadyMsg{Entry: entry}
	}
}

// sendGetDetailMetadata fires the runtime's GetDetailMetadata fan-out
// for the currently-open detail entry. Partials stream back out-of-order
// as ipc.DetailMetadataPartial messages and are applied by the ui.go
// Update handler through DetailState.ApplyMetadataPartial.
//
// When the runtime client is nil (offline / tests) the command is a no-op.
func (m *Model) sendGetDetailMetadata(entry ipc.DetailEntry) tea.Cmd {
	if m.client == nil {
		return nil
	}

	// Entries from non-TMDB providers carry a provider-prefixed id
	// like "anilist-199" or "kitsu-6448"; the entry's Provider field
	// may also be a comma-joined list of multiple providers (e.g.
	// "anilist,kitsu" for titles that merged across catalogs).
	//
	// Plugin-side verb handlers strictly require `id_source` to match
	// their own provider name and `id` to be the provider's native
	// id form (usually numeric). So we strip the provider prefix and
	// use *that* provider as the canonical id_source — whichever
	// plugin receives the request then finds its own namespace.
	entryID := entry.ID
	idSource := entry.IDSource
	if idSource == "" {
		// Prefer the prefix on the id itself — it's the most
		// authoritative signal for which catalog owns this entry.
		if prefix, rest, ok := splitProviderPrefix(entry.ID); ok {
			idSource = prefix
			entryID = rest
		} else {
			// Fall back to the first entry in the (possibly
			// comma-separated) provider list.
			idSource = firstProvider(entry.Provider)
		}
	}
	kind := entry.Kind
	if kind == "" {
		kind = entry.Tab
	}
	// Pull title/year/external_ids forward so the runtime's enrich stage
	// can title-search a foreign metadata source (e.g. resolve a kitsu-…
	// entry's AniList equivalent) and route credits/artwork/related
	// per-plugin via their native ids.
	title := entry.Title
	var yearPtr *uint16
	if entry.Year != "" {
		if y, err := strconv.Atoi(entry.Year); err == nil && y > 0 && y < 10000 {
			yu := uint16(y)
			yearPtr = &yu
		}
	}
	externalIDs := entry.ExternalIDs
	client := m.client
	return func() tea.Msg {
		client.GetDetailMetadata(entryID, idSource, kind, title, yearPtr, externalIDs)
		return nil
	}
}

// splitProviderPrefix recognises entry ids of the form
// "<provider>-<native_id>" (e.g. "anilist-199", "kitsu-6448") and
// returns (provider, native_id, true). For unprefixed numeric ids
// (TMDB style, e.g. "83533") returns ("", "", false).
func splitProviderPrefix(id string) (prefix, rest string, ok bool) {
	// Known provider prefixes we emit on the catalog side.
	for _, p := range []string{"anilist-", "kitsu-", "mal-", "tvdb-"} {
		if strings.HasPrefix(id, p) {
			return p[:len(p)-1], id[len(p):], true
		}
	}
	return "", "", false
}

// seasonsList expands a season-count integer into [1, 2, …, n]. Returns
// nil for a zero count so EpisodeScreen falls back to its single-season
// default — providers that don't expose `number_of_seasons` should
// undercount rather than have the UI fabricate seasons.
func seasonsList(count uint32) []int {
	if count == 0 {
		return nil
	}
	out := make([]int, count)
	for i := range out {
		out[i] = i + 1
	}
	return out
}

// firstProvider returns the leading provider name from a
// possibly-comma-joined list like "anilist,kitsu".
func firstProvider(p string) string {
	if i := strings.IndexByte(p, ','); i > 0 {
		return p[:i]
	}
	return p
}

// relatedItemToCatalogEntry reshapes a RelatedItemWire into the
// CatalogEntry shape that openDetail expects. Year/poster pointers are
// passed through where present.
func relatedItemToCatalogEntry(r ipc.RelatedItemWire) ipc.CatalogEntry {
	var yearStr *string
	if r.Year != nil {
		s := fmt.Sprintf("%d", *r.Year)
		yearStr = &s
	}
	return ipc.CatalogEntry{
		ID:        r.ID,
		Title:     r.Title,
		Year:      yearStr,
		PosterURL: r.PosterURL,
		Tab:       r.Kind,
		Source:    r.IDSource,
	}
}
