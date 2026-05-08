package screens

// detail_state.go — state for the detail overlay.
//
// Focus zones:
//   FocusDetailInfo     — top section (poster + metadata), scroll only
//   FocusDetailCrew     — crew list (directors / writers / composers etc.)
//   FocusDetailCast     — cast list, vertical cursor
//   FocusDetailProvider — STREAM VIA badges, horizontal cursor
//   FocusDetailRelated  — related titles row, horizontal cursor

import (
	"strings"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/watchhistory"
)

// DetailFocus is which zone of the detail overlay has keyboard focus.
type DetailFocus int

const (
	FocusDetailInfo           DetailFocus = iota // poster + meta + description
	FocusDetailCrew                              // crew (director/writer/etc.)
	FocusDetailCast                              // cast
	FocusDetailEpisodes                          // episode list inside the Episodes tab (series)
	FocusDetailProvider                          // STREAM VIA provider badges (Streams tab, movies)
	FocusDetailRelated                           // related titles row
	FocusDetailSeasons                           // season picker inside the Episodes tab
	FocusDetailEpisodeStreams                    // per-episode streams column inside the Episodes tab (series)
)

// DetailTab enumerates the tabs shown beneath the detail header.
//
// The set is conditional on the entry's tab/media type — Series shows
// Description + Episodes, Movies shows Description + Streams. Tab body
// rendering routes through `renderDescriptionTab` / `renderEpisodesTab`
// / `renderStreamsTab` in detail.go.
type DetailTab int

const (
	DetailTabDescription DetailTab = iota // CREW · CAST · RELATED
	DetailTabEpisodes                     // season picker + episode list (Series only)
	DetailTabStreams                      // provider badges + play (Movies only)
)

// EpisodeStreamsKey is the cache key for per-episode stream lists.
// Numeric (season, episode) — every entry in `Episodes[seasonN]` has
// a canonical episode number set by the plugin.
type EpisodeStreamsKey struct {
	Season  int
	Episode int
}

// String renders the tab label as displayed in the tab bar.
func (t DetailTab) String() string {
	switch t {
	case DetailTabDescription:
		return "Description"
	case DetailTabEpisodes:
		return "Episodes"
	case DetailTabStreams:
		return "Streams"
	}
	return ""
}

// FetchStatus tracks the lifecycle of one metadata verb's partial.
// The zero value is FetchPending so DetailState's embedded DetailMetadata
// starts in the correct state without explicit initialisation.
type FetchStatus int

const (
	FetchPending FetchStatus = iota // request dispatched or not yet dispatched
	FetchLoaded                     // partial arrived with non-empty payload
	FetchEmpty                      // partial arrived with "empty" / zero-valued payload
)

// DetailMetadata holds the four per-verb payloads streamed back by the
// runtime after a GetDetailMetadata request, plus per-zone cursor state.
// Each verb has its own FetchStatus so the renderer can distinguish
// "still loading" from "we tried, nothing available".
type DetailMetadata struct {
	EnrichStatus            FetchStatus
	CreditsStatus           FetchStatus
	ArtworkStatus           FetchStatus
	RelatedStatus           FetchStatus
	RatingsAggregatorStatus FetchStatus

	Credits           ipc.MetadataPayload
	Artwork           ipc.MetadataPayload
	Related           ipc.MetadataPayload
	RatingsAggregator ipc.MetadataPayload

	ArtworkCursor int
	CrewCursor    int
	RelatedCursor int
}

// BreadcrumbEntry is a single step in the navigation history.
type BreadcrumbEntry struct {
	Label string
	Entry ipc.DetailEntry
}

// DetailState is all mutable state for the detail overlay.
type DetailState struct {
	Entry   ipc.DetailEntry
	Loading bool
	Focus   DetailFocus

	// Active tab beneath the header. Default DetailTabDescription
	// (zero value). Tab navigation is keyboard-driven via tab/shift+tab
	// or 1/2/3 number keys; mouse on the tab bar also switches.
	ActiveTab DetailTab

	// Cast
	CastCursor int
	InfoScroll int

	// Provider selection (STREAM VIA)
	ProviderCursor int // index into Entry.Providers

	// Episodes tab state — populated lazily on tab open via the
	// runtime's `LoadEpisodes` IPC. SeasonsLoaded[i] = true means
	// Episodes[i] is the cached episode list for season N=i+1.
	SeasonCursor   int
	EpisodeCursor  int
	Episodes       map[int][]ipc.EpisodeEntry // keyed by season number
	EpisodesLoaded map[int]bool
	// Per-season error message. Was a single global string until a
	// timeout on one season started shadowing already-loaded data
	// for the season the user navigated back to. Same fix shape as
	// EpisodeStreamsError.
	EpisodesError map[int]string
	// True while a LoadEpisodes IPC for that season is in flight.
	// Prevents the user from piling up duplicate requests by
	// scrolling between seasons faster than the runtime/TMDB can
	// respond — each pending call held a supervisor slot and the
	// queue behind the live request would blow past the TUI's 60 s
	// IPC timeout, surfacing as "Failed to load episodes: timed
	// out" even though the upstream eventually succeeded.
	EpisodesInFlight map[int]bool

	// Per-episode streams column (3rd column in the Episodes tab).
	// Cursor for the focused stream row when the user navigates into
	// the streams column. The streams list itself is keyed by
	// `(seasonNumber, episodeNumber)` so multiple in-flight requests
	// (user scrubbing through episodes faster than the runtime can
	// reply) don't trample each other.
	EpisodeStreamCursor  int
	EpisodeStreams       map[EpisodeStreamsKey][]ipc.StreamInfo
	EpisodeStreamsLoaded map[EpisodeStreamsKey]bool
	// True while a find_streams IPC for this (season, ep) is in flight
	// — set on Enter dispatch, cleared on response (success or error).
	// Distinguishes "never searched" (renderer shows the press-Enter
	// hint) from "search currently running" (renderer shows the
	// spinner).
	EpisodeStreamsInFlight map[EpisodeStreamsKey]bool
	// Per-episode error string. Keyed (rather than a single global
	// field) so a timeout on episode N doesn't keep showing as the
	// "current" error while the user navigates to episode M and a
	// fresh request is in flight.
	EpisodeStreamsError map[EpisodeStreamsKey]string

	// Playback — non-empty while mpv is running for this entry
	NowPlaying *components.NowPlayingState

	// Metadata — populated by streamed DetailMetadataPartial events.
	Meta DetailMetadata

	// Breadcrumb / person mode
	Breadcrumbs   []BreadcrumbEntry
	PersonMode    bool
	PersonName    string
	PersonResults []ipc.CatalogEntry
	PersonLoading bool
	PersonCursor  GridCursor

	// Collection picker — shown when 'c' is pressed
	CollectionPickerOpen   bool
	CollectionPickerCursor int
	CollectionPickerNames  []string // populated by Model when picker is opened

	// Watch history — non-nil if this entry has been (partially) watched before
	WatchHistory *watchhistory.Entry
}

// HasEpisodesTab reports whether the entry is series-shaped and should
// expose the Episodes tab (alongside Description). Movies show Streams.
func (d *DetailState) HasEpisodesTab() bool {
	t := d.Entry.Tab
	return t == "series" || t == "Series"
}

// SeasonSlotCount returns the total number of rows the season list
// should render. Regular seasons 1..SeasonCount + a trailing
// "Specials" slot when the provider exposes one.
func (d *DetailState) SeasonSlotCount() int {
	count := int(d.Entry.SeasonCount)
	if count <= 0 {
		count = 1
	}
	if d.Entry.HasSpecials {
		count++
	}
	return count
}

// SeasonNumberForCursor maps SeasonCursor → the season number to send
// over IPC (LoadEpisodes, FindStreams). Cursors over the Specials slot
// resolve to season 0; regular seasons are cursor+1.
func (d *DetailState) SeasonNumberForCursor() int {
	regular := int(d.Entry.SeasonCount)
	if regular <= 0 {
		regular = 1
	}
	if d.Entry.HasSpecials && d.SeasonCursor == regular {
		return 0
	}
	return d.SeasonCursor + 1
}

// AvailableTabs returns the ordered tab list for this entry — used by
// both the tab-bar renderer and the tab-cycle key handler so they
// agree on the layout.
func (d *DetailState) AvailableTabs() []DetailTab {
	if d.HasEpisodesTab() {
		return []DetailTab{DetailTabDescription, DetailTabEpisodes}
	}
	return []DetailTab{DetailTabDescription, DetailTabStreams}
}

// CycleTab moves the active tab `delta` slots (1 = forward, -1 = back).
// Wraps. No-op on entries with no tabs (shouldn't happen — every detail
// page has at least Description).
func (d *DetailState) CycleTab(delta int) {
	tabs := d.AvailableTabs()
	if len(tabs) == 0 {
		return
	}
	idx := 0
	for i, t := range tabs {
		if t == d.ActiveTab {
			idx = i
			break
		}
	}
	idx = (idx + delta + len(tabs)) % len(tabs)
	d.ActiveTab = tabs[idx]
}

func NewDetailState(entry ipc.DetailEntry) DetailState {
	return DetailState{
		Entry:   entry,
		Loading: true,
		Focus:   FocusDetailInfo,
		// Meta's FetchStatus fields default to FetchPending (zero).
	}
}

// ApplyMetadataPartial merges one runtime-streamed DetailMetadataPartial
// into the state.  Partials for a stale entry (the user navigated away)
// are silently ignored.
//
// Verbs are processed independently — an "enrich" payload arriving after
// "credits" won't clobber the already-loaded credits.  Empty payloads
// flip the corresponding FetchStatus to FetchEmpty so renderers can
// distinguish "loading" from "none available".
func (d *DetailState) ApplyMetadataPartial(p ipc.DetailMetadataPartial) {
	// The TUI strips provider prefixes (e.g. "anilist-") off the entry id
	// before sending GetDetailMetadata, so partials carry the stripped
	// native id ("11061") while d.Entry.ID still has the prefixed form
	// ("anilist-11061"). Accept both: exact match for TMDB-style ids, or
	// suffix match (after a "-") for prefixed providers.
	if p.EntryID != d.Entry.ID && !strings.HasSuffix(d.Entry.ID, "-"+p.EntryID) {
		return
	}
	status := FetchLoaded
	if p.Payload.Type == "empty" || isPayloadEmpty(p.Payload) {
		status = FetchEmpty
	}
	switch p.Verb {
	case "enrich":
		if p.Payload.Studio != nil {
			d.Entry.Studio = *p.Payload.Studio
		}
		if len(p.Payload.Networks) > 0 {
			d.Entry.Networks = p.Payload.Networks
		}
		if len(p.Payload.ExternalIDs) > 0 {
			d.Entry.ExternalIDs = p.Payload.ExternalIDs
			// Mirror the canonical IMDB / TMDB ids onto the
			// convenience fields. The catalog's `search` step
			// can't afford a per-result `external_ids` lookup
			// (TMDB quota), so non-anime entries arrive here
			// with empty `Entry.ImdbID`. The enrich verb is the
			// first time the IMDB id is known — propagate it so
			// downstream consumers (the find_streams dispatch
			// in particular) can pass it to torrentio etc.
			if id := p.Payload.ExternalIDs["imdb"]; id != "" && d.Entry.ImdbID == "" {
				d.Entry.ImdbID = id
			}
			if id := p.Payload.ExternalIDs["tmdb"]; id != "" && d.Entry.TmdbID == "" {
				d.Entry.TmdbID = id
			}
		}
		if p.Payload.SeasonCount != nil && *p.Payload.SeasonCount > 0 {
			d.Entry.SeasonCount = *p.Payload.SeasonCount
		}
		if len(p.Payload.SeasonIDs) > 0 {
			d.Entry.SeasonIDs = append([]string(nil), p.Payload.SeasonIDs...)
		}
		// Latch true once any provider reports specials — different
		// providers might disagree, but if any of them have a Specials
		// track we want to expose it.
		if p.Payload.HasSpecials {
			d.Entry.HasSpecials = true
		}
		d.Meta.EnrichStatus = status
	case "credits":
		d.Meta.Credits = p.Payload
		d.Meta.CreditsStatus = status
		// Mirror Cast into Entry.Cast so detail.go's CAST renderer (which
		// pre-dates Meta.Credits and still reads Entry.Cast) populates.
		// CastWire.Character is the user-facing role; fall back to Role
		// (e.g. "actor") when a character isn't reported.
		if len(p.Payload.Cast) > 0 {
			cast := make([]ipc.CastMember, 0, len(p.Payload.Cast))
			for _, c := range p.Payload.Cast {
				role := c.Role
				if c.Character != nil && *c.Character != "" {
					role = *c.Character
				}
				cast = append(cast, ipc.CastMember{
					Name:     c.Name,
					Role:     role,
					RoleType: "cast",
				})
			}
			d.Entry.Cast = cast
		}
	case "artwork":
		d.Meta.Artwork = p.Payload
		d.Meta.ArtworkStatus = status
	case "related":
		d.Meta.Related = p.Payload
		d.Meta.RelatedStatus = status
	case "ratings_aggregator":
		d.Meta.RatingsAggregator = p.Payload
		d.Meta.RatingsAggregatorStatus = status
	}
}

// isPayloadEmpty is true when every variant-specific field of p is at its
// zero value.  Used to downgrade a FetchLoaded status to FetchEmpty when
// the runtime streams a struct-shaped payload that contains no data.
func isPayloadEmpty(p ipc.MetadataPayload) bool {
	return len(p.Cast) == 0 && len(p.Crew) == 0 &&
		len(p.Backdrops) == 0 && len(p.Posters) == 0 &&
		len(p.Items) == 0 && p.Studio == nil &&
		len(p.Networks) == 0 && len(p.ExternalIDs) == 0 &&
		p.SeasonCount == nil && len(p.SeasonIDs) == 0 &&
		p.Description == ""
}

func (d *DetailState) SelectedCastMember() *ipc.CastMember {
	if d.Focus != FocusDetailCast {
		return nil
	}
	if d.CastCursor < 0 || d.CastCursor >= len(d.Entry.Cast) {
		return nil
	}
	c := d.Entry.Cast[d.CastCursor]
	return &c
}

// SelectedProvider returns the provider name under the cursor, or "".
// CurrentStreamsKey returns the EpisodeStreams map key for the streams
// column the user is currently focused on. Movies (Streams tab) all
// share the sentinel `{0, 0}` since they're a single addressable item;
// series episodes are keyed by `(season, episode)`. The streams cache,
// in-flight set, and error map all key by this value so the same
// streaming pipeline serves both tabs without per-tab fan-outs.
func (d *DetailState) CurrentStreamsKey() EpisodeStreamsKey {
	if d.ActiveTab == DetailTabStreams {
		return EpisodeStreamsKey{Season: 0, Episode: 0}
	}
	seasonNum := d.SeasonNumberForCursor()
	eps := d.Episodes[seasonNum]
	if d.EpisodeCursor < 0 || d.EpisodeCursor >= len(eps) {
		return EpisodeStreamsKey{}
	}
	return EpisodeStreamsKey{Season: seasonNum, Episode: int(eps[d.EpisodeCursor].Episode)}
}

func (d *DetailState) SelectedProvider() string {
	if len(d.Entry.Providers) == 0 {
		return ""
	}
	idx := d.ProviderCursor
	if idx < 0 {
		idx = 0
	}
	if idx >= len(d.Entry.Providers) {
		idx = len(d.Entry.Providers) - 1
	}
	return d.Entry.Providers[idx]
}

func (d *DetailState) PushBreadcrumb(label string) {
	d.Breadcrumbs = append(d.Breadcrumbs, BreadcrumbEntry{
		Label: label,
		Entry: d.Entry,
	})
}

func (d *DetailState) PopBreadcrumb() bool {
	n := len(d.Breadcrumbs)
	if n == 0 {
		return false
	}
	prev := d.Breadcrumbs[n-1]
	d.Breadcrumbs = d.Breadcrumbs[:n-1]
	d.Entry = prev.Entry
	d.Loading = false
	d.PersonMode = false
	d.PersonName = ""
	d.PersonResults = nil
	d.CastCursor = 0
	d.Focus = FocusDetailCast
	return true
}

func (d *DetailState) BreadcrumbTrail(tabName string) string {
	parts := []string{tabName}
	for _, b := range d.Breadcrumbs {
		parts = append(parts, b.Label)
	}
	if d.PersonMode && d.PersonName != "" {
		parts = append(parts, d.PersonName)
	} else if !d.PersonMode {
		parts = append(parts, d.Entry.Title)
	}
	out := ""
	for i, p := range parts {
		if i > 0 {
			out += "  ›  "
		}
		out += p
	}
	return out
}
