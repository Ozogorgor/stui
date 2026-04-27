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

	// Cycle focus zones in the order they're rendered: Info → Crew →
	// Cast → Episodes (series only) → Provider → Related → Info. Empty
	// zones are skipped via detailFocusOrder so the user never lands on
	// an unrenderable section.
	case "tab":
		if ds.PersonMode {
			return m, nil
		}
		order := detailFocusOrder(ds)
		ds.Focus = stepFocus(order, ds.Focus, +1)
		return m, nil

	case "shift+tab":
		if ds.PersonMode {
			return m, nil
		}
		order := detailFocusOrder(ds)
		ds.Focus = stepFocus(order, ds.Focus, -1)
		return m, nil

	case "j", "down":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorDown(ds.PersonCursor, len(ds.PersonResults))
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
			} else if len(ds.Entry.Providers) > 0 {
				ds.Focus = screens.FocusDetailProvider
			}
		case ds.Focus == screens.FocusDetailProvider:
			if len(ds.Meta.Related.Items) > 0 {
				ds.Focus = screens.FocusDetailRelated
			}
		case ds.Focus == screens.FocusDetailRelated:
			// already at bottom
		}
		return m, nil

	case "k", "up":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorUp(ds.PersonCursor)
		case ds.Focus == screens.FocusDetailInfo:
			if ds.InfoScroll > 0 {
				ds.InfoScroll--
			}
		case ds.Focus == screens.FocusDetailCrew:
			if ds.Meta.CrewCursor > 0 {
				ds.Meta.CrewCursor--
			} else {
				ds.Focus = screens.FocusDetailInfo
			}
		case ds.Focus == screens.FocusDetailCast:
			if ds.CastCursor > 0 {
				ds.CastCursor--
			} else if len(ds.Meta.Credits.Crew) > 0 {
				ds.Focus = screens.FocusDetailCrew
			} else {
				ds.Focus = screens.FocusDetailInfo
			}
		case ds.Focus == screens.FocusDetailProvider:
			switch {
			case len(ds.Entry.Cast) > 0:
				ds.Focus = screens.FocusDetailCast
			case len(ds.Meta.Credits.Crew) > 0:
				ds.Focus = screens.FocusDetailCrew
			default:
				ds.Focus = screens.FocusDetailInfo
			}
		case ds.Focus == screens.FocusDetailRelated:
			switch {
			case len(ds.Entry.Providers) > 0:
				ds.Focus = screens.FocusDetailProvider
			case len(ds.Entry.Cast) > 0:
				ds.Focus = screens.FocusDetailCast
			case len(ds.Meta.Credits.Crew) > 0:
				ds.Focus = screens.FocusDetailCrew
			}
		}
		return m, nil

	case "h", "left":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorLeft(ds.PersonCursor)
		case ds.Focus == screens.FocusDetailInfo && len(ds.Meta.Artwork.Backdrops) > 0:
			// Cycle backdrop carousel while the info zone has focus.
			n := len(ds.Meta.Artwork.Backdrops)
			ds.Meta.ArtworkCursor = (ds.Meta.ArtworkCursor - 1 + n) % n
		case ds.Focus == screens.FocusDetailProvider:
			if ds.ProviderCursor > 0 {
				ds.ProviderCursor--
			}
		case ds.Focus == screens.FocusDetailRelated:
			if ds.Meta.RelatedCursor > 0 {
				ds.Meta.RelatedCursor--
			}
		}
		return m, nil

	case "l", "right":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorRight(ds.PersonCursor, len(ds.PersonResults))
		case ds.Focus == screens.FocusDetailInfo && len(ds.Meta.Artwork.Backdrops) > 0:
			// Cycle backdrop carousel while the info zone has focus.
			n := len(ds.Meta.Artwork.Backdrops)
			ds.Meta.ArtworkCursor = (ds.Meta.ArtworkCursor + 1) % n
		case ds.Focus == screens.FocusDetailProvider:
			if ds.ProviderCursor < len(ds.Entry.Providers)-1 {
				ds.ProviderCursor++
			}
		case ds.Focus == screens.FocusDetailRelated:
			if ds.Meta.RelatedCursor < len(ds.Meta.Related.Items)-1 {
				ds.Meta.RelatedCursor++
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

		case ds.Focus == screens.FocusDetailEpisodes:
			// Same path as the `e` keybind below — opens the season /
			// episode browser.  Repeated here so users who Tab into the
			// EPISODES badge can launch it without learning a separate key.
			idSource := ds.Entry.IDSource
			if idSource == "" {
				if prefix, _, ok := splitProviderPrefix(ds.Entry.ID); ok {
					idSource = prefix
				} else {
					idSource = firstProvider(ds.Entry.Provider)
				}
			}
			backdropURL := ""
			if len(ds.Meta.Artwork.Backdrops) > 0 {
				backdropURL = ds.Meta.Artwork.Backdrops[0].URL
			}
			s := screens.NewEpisodeScreen(m.client, ds.Entry.Title, ds.Entry.ID, idSource, m.state.Settings.AutoplayNext, screens.EpisodeScreenOpts{
				Year:        ds.Entry.Year,
				Genre:       ds.Entry.Genre,
				Rating:      ds.Entry.Rating,
				PosterURL:   ds.Entry.PosterURL,
				PosterArt:   ds.Entry.PosterArt,
				BackdropURL: backdropURL,
				Seasons:     seasonsList(ds.Entry.SeasonCount),
				SeasonIDs:   ds.Entry.SeasonIDs,
			})
			return m, screen.TransitionCmd(s, true)

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
		// Open episode browser for series items. Resolve id_source so the
		// runtime knows which plugin owns this entry — prefer the entry's
		// own field, then peel a "<provider>-<id>" prefix off the id, then
		// fall back to the first comma-separated provider.
		if ds.Entry.Tab == "series" || ds.Entry.Tab == "Series" {
			idSource := ds.Entry.IDSource
			if idSource == "" {
				if prefix, _, ok := splitProviderPrefix(ds.Entry.ID); ok {
					idSource = prefix
				} else {
					idSource = firstProvider(ds.Entry.Provider)
				}
			}
			backdropURL := ""
			if len(ds.Meta.Artwork.Backdrops) > 0 {
				backdropURL = ds.Meta.Artwork.Backdrops[0].URL
			}
			s := screens.NewEpisodeScreen(m.client, ds.Entry.Title, ds.Entry.ID, idSource, m.state.Settings.AutoplayNext, screens.EpisodeScreenOpts{
				Year:        ds.Entry.Year,
				Genre:       ds.Entry.Genre,
				Rating:      ds.Entry.Rating,
				PosterURL:   ds.Entry.PosterURL,
				PosterArt:   ds.Entry.PosterArt,
				BackdropURL: backdropURL,
				Seasons:     seasonsList(ds.Entry.SeasonCount),
				SeasonIDs:   ds.Entry.SeasonIDs,
			})
			return m, screen.TransitionCmd(s, true)
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
