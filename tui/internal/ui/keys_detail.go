// keys_detail.go — keypress routing inside the detail overlay
// (FocusDetailInfo / Crew / Cast / Provider / Episodes / Related)
// plus the inline collection-picker keys ('c'). Person-mode dispatch
// helper is colocated since it's the only caller.

package ui

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/collections"
	"github.com/stui/stui/pkg/watchhistory"
)

// defaultFocusForTab picks the default cursor zone when the user
// switches into a tab. Description starts on the info zone (so j/k
// scrolls + h/l cycles backdrops); Episodes starts on Seasons (the
// left column of the season picker, so j/k navigates seasons by
// default); Streams starts on the provider row.
func defaultFocusForTab(tab screens.DetailTab) screens.DetailFocus {
	switch tab {
	case screens.DetailTabEpisodes:
		return screens.FocusDetailSeasons
	case screens.DetailTabStreams:
		// Movies' Streams tab now hosts the same streams panel as the
		// per-episode streams column; default focus to the streams
		// list so j/k/Enter behave the same as in the Episodes tab.
		return screens.FocusDetailEpisodeStreams
	default:
		return screens.FocusDetailInfo
	}
}

// maybeLoadEpisodesForTab kicks off a `LoadEpisodes` IPC for the
// season the user just landed on, if it hasn't already been fetched
// AND no request for it is currently in flight. Called from the
// tab-cycle handlers + the `e` keybind so the Episodes tab starts
// populating immediately on entry.
func (m Model) maybeLoadEpisodesForTab(ds *screens.DetailState) tea.Cmd {
	if ds == nil || m.client == nil {
		return nil
	}
	if ds.ActiveTab != screens.DetailTabEpisodes {
		return nil
	}
	season := ds.SeasonCursor + 1
	if ds.EpisodesLoaded[season] {
		return nil
	}
	if ds.EpisodesInFlight[season] {
		// Already waiting on this season — duplicating the request
		// would queue behind the live one in the runtime's
		// supervisor lock and almost certainly trip the TUI's 60 s
		// IPC timeout for the queued copy.
		return nil
	}
	if ds.Episodes == nil {
		ds.Episodes = make(map[int][]ipc.EpisodeEntry)
	}
	if ds.EpisodesLoaded == nil {
		ds.EpisodesLoaded = make(map[int]bool)
	}
	if ds.EpisodesInFlight == nil {
		ds.EpisodesInFlight = make(map[int]bool)
	}
	ds.EpisodesInFlight[season] = true
	seriesID, idSource := episodeLookupTarget(ds)
	client := m.client
	client.LoadEpisodes(seriesID, idSource, season)
	return nil
}

// dispatchFindStreamsForCursor kicks off a `find_streams` IPC for the
// currently-focused streams target. Two modes:
//
//   - Episodes tab: the focused (season, episode) row.
//   - Streams tab (movies): the entry itself, no season/episode.
//
// Both routes share the same handler pipeline on the runtime side and
// the same EpisodeStreams cache on the TUI side (movies use the
// `{0, 0}` sentinel key).
func (m Model) dispatchFindStreamsForCursor(ds *screens.DetailState) tea.Cmd {
	if ds == nil || m.client == nil {
		return nil
	}
	year := uint32(0)
	if ds.Entry.Year != "" {
		fmt.Sscanf(ds.Entry.Year, "%d", &year)
	}
	var yearPtr *uint32
	if year > 0 {
		yearPtr = &year
	}
	// IMDB / TMDB id: convenience field first, external_ids map as
	// backup. Enrich mirrors map → convenience as it lands, but if
	// Enter fires before enrich completed the map is the only source.
	imdb := ds.Entry.ImdbID
	if imdb == "" {
		imdb = ds.Entry.ExternalIDs["imdb"]
	}
	tmdb := ds.Entry.TmdbID
	if tmdb == "" {
		tmdb = ds.Entry.ExternalIDs["tmdb"]
	}
	req := ipc.FindStreamsRequest{
		Title:       ds.Entry.Title,
		Year:        yearPtr,
		ImdbID:      imdb,
		TmdbID:      tmdb,
		ExternalIDs: ds.Entry.ExternalIDs,
	}
	if ds.ActiveTab == screens.DetailTabStreams {
		req.Kind = "Movie"
	} else {
		eps := ds.Episodes[ds.SeasonCursor+1]
		if ds.EpisodeCursor < 0 || ds.EpisodeCursor >= len(eps) {
			return nil
		}
		ep := eps[ds.EpisodeCursor]
		seasonPtr := uint32(ds.SeasonCursor + 1)
		episodePtr := uint32(ep.Episode)
		req.Kind = "Series"
		req.Season = &seasonPtr
		req.Episode = &episodePtr
	}
	client := m.client
	go func() { client.FindStreams(req) }()
	return nil
}

// ── Detail key handler ────────────────────────────────────────────────────────

func (m Model) handleDetailKey(key string) (tea.Model, tea.Cmd) {
	ds := m.detail

	// Collection picker swallows all keys while open
	if ds.CollectionPickerOpen {
		return m.handleCollectionPickerKey(key)
	}

	switch key {
	case "c":
		// Open the inline collection picker
		if !ds.PersonMode && m.collectionsStore != nil {
			ds.CollectionPickerOpen = true
			ds.CollectionPickerCursor = 0
			ds.CollectionPickerNames = m.collectionsStore.Names()
		}
		return m, nil

	case "esc":
		if ds.PersonMode {
			if !ds.PopBreadcrumb() {
				m.screen = screenGrid
				m.detail = nil
				if !m.state.CurrentStream.IsSet() {
					m.state.CurrentMedia = state.CurrentMedia{}
				}
			}
			return m, nil
		}
		m.screen = screenGrid
		m.detail = nil
		if !m.state.CurrentStream.IsSet() {
			m.state.CurrentMedia = state.CurrentMedia{}
		}
		return m, nil

	case "q", "ctrl+c":
		// q stops playback if active; if not, quits the app
		if ds.NowPlaying != nil && m.client != nil {
			m.client.PlayerStop()
			m.nowPlayingEntryID = "" // manual stop — suppress auto-delete
			return m, nil
		}
		if m.client != nil {
			m.client.Stop()
		}
		return m, tea.Quit

	// Tab / shift+tab cycle the top-level detail tabs (Description /
	// Episodes / Streams). Mirrors the Music sub-tab navigation —
	// users coming from the Music screen know the pattern. Within a
	// tab, j/k/h/l drive the cursors that section needs.
	case "tab":
		if ds.PersonMode {
			return m, nil
		}
		ds.CycleTab(+1)
		// Reset focus to a sensible per-tab default so empty zones
		// don't leak into the new tab's behaviour.
		ds.Focus = defaultFocusForTab(ds.ActiveTab)
		return m, m.maybeLoadEpisodesForTab(ds)

	case "shift+tab":
		if ds.PersonMode {
			return m, nil
		}
		ds.CycleTab(-1)
		ds.Focus = defaultFocusForTab(ds.ActiveTab)
		return m, m.maybeLoadEpisodesForTab(ds)

	case "1", "2":
		if ds.PersonMode {
			return m, nil
		}
		// Direct-jump to tab 1 / 2 (matching the music screen's
		// quick-switch). Out-of-range key (e.g. "2" for an entry
		// without an Episodes/Streams tab) is a no-op.
		idx := int(key[0] - '1')
		tabs := ds.AvailableTabs()
		if idx >= 0 && idx < len(tabs) {
			ds.ActiveTab = tabs[idx]
			ds.Focus = defaultFocusForTab(ds.ActiveTab)
			return m, m.maybeLoadEpisodesForTab(ds)
		}
		return m, nil

	case "j", "down":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorDown(ds.PersonCursor, len(ds.PersonResults))
		case ds.Focus == screens.FocusDetailSeasons:
			count := int(ds.Entry.SeasonCount)
			if count <= 0 {
				count = 1
			}
			if ds.SeasonCursor < count-1 {
				ds.SeasonCursor++
				return m, m.maybeLoadEpisodesForTab(ds)
			}
		case ds.Focus == screens.FocusDetailEpisodes:
			eps := ds.Episodes[ds.SeasonCursor+1]
			if ds.EpisodeCursor < len(eps)-1 {
				ds.EpisodeCursor++
			}
		case ds.Focus == screens.FocusDetailEpisodeStreams:
			// Streams column: cursor walks the cached list for the
			// current key (movie or focused episode). Bound below by
			// len-1 so we don't index off the end while partials are
			// still streaming in (the list grows as providers respond).
			key := ds.CurrentStreamsKey()
			if ds.EpisodeStreamCursor < len(ds.EpisodeStreams[key])-1 {
				ds.EpisodeStreamCursor++
			}
		case ds.Focus == screens.FocusDetailInfo:
			ds.InfoScroll++
		case ds.Focus == screens.FocusDetailCrew:
			if ds.Meta.CrewCursor < len(ds.Meta.Credits.Crew)-1 {
				ds.Meta.CrewCursor++
			} else if len(ds.Entry.Cast) > 0 {
				ds.Focus = screens.FocusDetailCast
			}
		case ds.Focus == screens.FocusDetailCast:
			if ds.CastCursor < len(ds.Entry.Cast)-1 {
				ds.CastCursor++
			} else if len(ds.Meta.Related.Items) > 0 {
				ds.Focus = screens.FocusDetailRelated
			}
		case ds.Focus == screens.FocusDetailProvider:
			// Provider row is horizontal — j/k is a no-op (use h/l).
		case ds.Focus == screens.FocusDetailRelated:
			if ds.Meta.RelatedCursor < len(ds.Meta.Related.Items)-1 {
				ds.Meta.RelatedCursor++
			}
		}
		return m, nil

	case "k", "up":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorUp(ds.PersonCursor)
		case ds.Focus == screens.FocusDetailSeasons:
			if ds.SeasonCursor > 0 {
				ds.SeasonCursor--
				return m, m.maybeLoadEpisodesForTab(ds)
			}
		case ds.Focus == screens.FocusDetailEpisodes:
			if ds.EpisodeCursor > 0 {
				ds.EpisodeCursor--
			}
		case ds.Focus == screens.FocusDetailEpisodeStreams:
			if ds.EpisodeStreamCursor > 0 {
				ds.EpisodeStreamCursor--
			}
		case ds.Focus == screens.FocusDetailInfo:
			if ds.InfoScroll > 0 {
				ds.InfoScroll--
			}
		case ds.Focus == screens.FocusDetailCrew:
			if ds.Meta.CrewCursor > 0 {
				ds.Meta.CrewCursor--
			}
		case ds.Focus == screens.FocusDetailCast:
			if ds.CastCursor > 0 {
				ds.CastCursor--
			} else if len(ds.Meta.Credits.Crew) > 0 {
				ds.Focus = screens.FocusDetailCrew
			}
		case ds.Focus == screens.FocusDetailRelated:
			if ds.Meta.RelatedCursor > 0 {
				ds.Meta.RelatedCursor--
			} else if len(ds.Entry.Cast) > 0 {
				ds.Focus = screens.FocusDetailCast
			}
		}
		return m, nil

	case "h", "left":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorLeft(ds.PersonCursor)
		case ds.Focus == screens.FocusDetailEpisodeStreams:
			// Episodes tab: h moves focus from the streams column
			// back to the episode list.
			ds.Focus = screens.FocusDetailEpisodes
		case ds.Focus == screens.FocusDetailEpisodes:
			// Episodes tab: h moves focus from the episode list
			// back to the season picker.
			ds.Focus = screens.FocusDetailSeasons
		case ds.Focus == screens.FocusDetailInfo && len(ds.Meta.Artwork.Backdrops) > 0:
			n := len(ds.Meta.Artwork.Backdrops)
			ds.Meta.ArtworkCursor = (ds.Meta.ArtworkCursor - 1 + n) % n
		case ds.Focus == screens.FocusDetailProvider:
			if ds.ProviderCursor > 0 {
				ds.ProviderCursor--
			}
		}
		return m, nil

	case "l", "right":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorRight(ds.PersonCursor, len(ds.PersonResults))
		case ds.Focus == screens.FocusDetailSeasons:
			// Episodes tab: l moves focus from the season picker to
			// the episode list. The streams column stays empty until
			// the user explicitly presses Enter on a row — that's
			// the trigger for find_streams. Auto-dispatching on
			// every focus shift dispatched stale searches when the
			// user was just navigating, and the responses fought
			// over the streams column.
			if len(ds.Episodes[ds.SeasonCursor+1]) > 0 {
				ds.Focus = screens.FocusDetailEpisodes
			}
		case ds.Focus == screens.FocusDetailEpisodes:
			// Move focus to the streams column. Whatever is cached
			// for the current (season, episode) shows; if nothing,
			// the user presses Enter from the episode list to
			// dispatch a fresh search.
			eps := ds.Episodes[ds.SeasonCursor+1]
			if ds.EpisodeCursor >= 0 && ds.EpisodeCursor < len(eps) {
				ds.Focus = screens.FocusDetailEpisodeStreams
				ds.EpisodeStreamCursor = 0
			}
		case ds.Focus == screens.FocusDetailInfo && len(ds.Meta.Artwork.Backdrops) > 0:
			n := len(ds.Meta.Artwork.Backdrops)
			ds.Meta.ArtworkCursor = (ds.Meta.ArtworkCursor + 1) % n
		case ds.Focus == screens.FocusDetailProvider:
			if ds.ProviderCursor < len(ds.Entry.Providers)-1 {
				ds.ProviderCursor++
			}
		}
		return m, nil

	case "enter":
		switch {
		case ds.PersonMode:
			idx := ds.PersonCursor.Index(components.CardColumns)
			if idx >= 0 && idx < len(ds.PersonResults) {
				ds.PushBreadcrumb(ds.PersonName)
				return m, m.openDetail(ds.PersonResults[idx])
			}

		case ds.Focus == screens.FocusDetailCast:
			member := ds.SelectedCastMember()
			if member == nil {
				return m, nil
			}
			ds.PushBreadcrumb(ds.Entry.Title)
			ds.PersonMode = true
			ds.PersonName = member.Name
			ds.PersonResults = nil
			ds.PersonLoading = true
			ds.PersonCursor = screens.GridCursor{}
			return m, m.dispatchPersonSearch(member.Name)

		case ds.Focus == screens.FocusDetailEpisodes,
			ds.Focus == screens.FocusDetailEpisodeStreams && ds.ActiveTab == screens.DetailTabStreams:
			// Two paths trigger a stream search:
			//   - Episodes tab: Enter on the focused episode row.
			//   - Streams tab (movies): Enter while focused on the
			//     streams panel — there's no row above it to act as
			//     the trigger, so the streams panel itself accepts
			//     the keystroke.
			// Both clear the cached entry for the current key first so
			// a stale timeout / empty-result from a prior dispatch
			// can't shadow the new in-flight request — the renderer
			// falls through to "Searching torrents…" until the
			// runtime replies.
			key := ds.CurrentStreamsKey()
			// Episodes tab guard: bail if no episode is selected.
			if ds.ActiveTab == screens.DetailTabEpisodes {
				eps := ds.Episodes[ds.SeasonCursor+1]
				if ds.EpisodeCursor < 0 || ds.EpisodeCursor >= len(eps) {
					return m, nil
				}
			}
			delete(ds.EpisodeStreams, key)
			delete(ds.EpisodeStreamsLoaded, key)
			delete(ds.EpisodeStreamsError, key)
			if ds.EpisodeStreamsInFlight == nil {
				ds.EpisodeStreamsInFlight = make(map[screens.EpisodeStreamsKey]bool)
			}
			ds.EpisodeStreamsInFlight[key] = true
			ds.EpisodeStreamCursor = 0
			return m, m.dispatchFindStreamsForCursor(ds)

		case ds.Focus == screens.FocusDetailProvider:
			// ▶ Play via selected provider — resume from saved position if available
			provider := ds.SelectedProvider()
			if provider != "" && m.client != nil {
				tab := ipc.MediaTab(m.state.ActiveTab.MediaTabID())
				startPos := 0.0
				if ds.WatchHistory != nil && ds.WatchHistory.Position > 0 && !ds.WatchHistory.Completed {
					startPos = ds.WatchHistory.Position
					m.state.StatusMsg = fmt.Sprintf("Resuming via %s from %s…",
						provider, formatDurationHMS(startPos))
				} else {
					m.state.StatusMsg = fmt.Sprintf("Resolving via %s…", provider)
				}
				m.client.PlayFrom(ds.Entry.ID, provider, ds.Entry.ImdbID, tab, startPos)
				m.nowPlayingEntryID = ds.Entry.ID
				m.historyLastSavedPos = startPos
				season, episode := watchhistory.ParseEpisodeInfo(ds.Entry.Title)
				m.nowPlayingEntry = watchhistory.Entry{
					ID:       ds.Entry.ID,
					Title:    ds.Entry.Title,
					Year:     ds.Entry.Year,
					Tab:      ds.Entry.Tab,
					Provider: provider,
					ImdbID:   ds.Entry.ImdbID,
					Season:   season,
					Episode:  episode,
				}
				// Create/update the history record immediately so progress
				// updates have an entry to upsert into.
				if m.historyStore != nil {
					m.historyStore.Upsert(m.nowPlayingEntry)
				}
			}
			return m, nil

		case ds.Focus == screens.FocusDetailRelated:
			idx := ds.Meta.RelatedCursor
			items := ds.Meta.Related.Items
			if idx >= 0 && idx < len(items) {
				ds.PushBreadcrumb(ds.Entry.Title)
				return m, m.openDetail(relatedItemToCatalogEntry(items[idx]))
			}
		}
		return m, nil

	case "e", "E":
		// Switch to the inline Episodes tab on series cards. The
		// dedicated EpisodeScreen was retired in favor of the tabbed
		// detail layout — clicking `e` is now equivalent to pressing
		// `tab` until you land on Episodes (or just `2`), kept as a
		// muscle-memory shortcut for users used to the old keybind.
		if ds.HasEpisodesTab() {
			ds.ActiveTab = screens.DetailTabEpisodes
			ds.Focus = defaultFocusForTab(ds.ActiveTab)
			return m, m.maybeLoadEpisodesForTab(ds)
		}
		return m, nil

	case "s":
		// Open stream picker for the current item
		if !ds.PersonMode && m.client != nil {
			s := screens.NewStreamPickerScreen(m.client, ds.Entry.Title, ds.Entry.ID, m.state.Settings.BenchmarkStreams)
			return m, screen.TransitionCmd(s, true)
		}
		return m, nil
	}

	return m, nil
}

// ── Collection picker key handler ─────────────────────────────────────────────

func (m Model) handleCollectionPickerKey(key string) (tea.Model, tea.Cmd) {
	ds := m.detail
	switch key {
	case "esc":
		ds.CollectionPickerOpen = false

	case "j", "down":
		if ds.CollectionPickerCursor < len(ds.CollectionPickerNames)-1 {
			ds.CollectionPickerCursor++
		}

	case "k", "up":
		if ds.CollectionPickerCursor > 0 {
			ds.CollectionPickerCursor--
		}

	case "enter":
		if ds.CollectionPickerCursor < len(ds.CollectionPickerNames) && m.collectionsStore != nil {
			collName := ds.CollectionPickerNames[ds.CollectionPickerCursor]
			entry := collections.Entry{
				ID:       ds.Entry.ID,
				Title:    ds.Entry.Title,
				Year:     ds.Entry.Year,
				Tab:      ds.Entry.Tab,
				Provider: ds.Entry.Provider,
				ImdbID:   ds.Entry.ImdbID,
			}
			added := m.collectionsStore.AddTo(collName, entry)
			go func() { _ = m.collectionsStore.Save() }()
			ds.CollectionPickerOpen = false
			if added {
				m.state.StatusMsg = fmt.Sprintf("Added “%s” to %s", ds.Entry.Title, collName)
			} else {
				m.state.StatusMsg = fmt.Sprintf("Already in %s", collName)
			}
		}
	}
	return m, nil
}

// detailFocusOrder returns the visible focus zones in render order,
// skipping zones that have nothing to show (no crew → skip Crew, etc.).
// Drives Tab / Shift+Tab navigation so neither key ever lands on an
// empty section.  Episodes zone is series-only.
func detailFocusOrder(ds *screens.DetailState) []screens.DetailFocus {
	order := []screens.DetailFocus{screens.FocusDetailInfo}
	if len(ds.Meta.Credits.Crew) > 0 {
		order = append(order, screens.FocusDetailCrew)
	}
	if len(ds.Entry.Cast) > 0 {
		order = append(order, screens.FocusDetailCast)
	}
	if ds.Entry.Tab == "series" || ds.Entry.Tab == "Series" {
		order = append(order, screens.FocusDetailEpisodes)
	}
	if len(ds.Entry.Providers) > 0 {
		order = append(order, screens.FocusDetailProvider)
	}
	if len(ds.Meta.Related.Items) > 0 {
		order = append(order, screens.FocusDetailRelated)
	}
	return order
}

// stepFocus walks the visible focus order by `delta` (+1 forward, -1
// back), wrapping around at either end.  Returns the current zone
// unchanged if the order is empty (defensive — order always contains at
// least FocusDetailInfo so this branch never fires in practice).
func stepFocus(order []screens.DetailFocus, current screens.DetailFocus, delta int) screens.DetailFocus {
	if len(order) == 0 {
		return current
	}
	idx := -1
	for i, f := range order {
		if f == current {
			idx = i
			break
		}
	}
	if idx == -1 {
		// Current zone is no longer visible (e.g. focus was on Crew,
		// then credits empty-cleared) — snap to the start of the order.
		return order[0]
	}
	next := (idx + delta + len(order)) % len(order)
	return order[next]
}

func (m *Model) dispatchPersonSearch(name string) tea.Cmd {
	if m.client == nil {
		// No runtime — search local grid
		tab := m.state.ActiveTab.MediaTabID()
		entries := m.grids[tab]
		q := strings.ToLower(name)
		return func() tea.Msg {
			var matches []ipc.CatalogEntry
			for _, e := range entries {
				if strings.Contains(strings.ToLower(e.Title), q) {
					matches = append(matches, e)
				}
			}
			// Return results via SearchResultMsg; handled by the person-mode branch in Update.
			items := make([]ipc.MediaEntry, 0, len(matches))
			for _, e := range matches {
				items = append(items, ipc.MediaEntry{
					ID: e.ID, Title: e.Title,
					Year: e.Year, Genre: e.Genre, Rating: e.Rating,
					Provider: e.Provider,
				})
			}
			total := len(items)
			return ipc.SearchResultMsg{Result: ipc.SearchResult{Items: items, Total: total}}
		}
	}
	// TODO(Task 7.0): migrate to streaming ScopeResults for the person-mode overlay.
	_ = name
	return nil
}
