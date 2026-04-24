package screens

// detail_state.go — state for the detail overlay.
//
// Focus zones:
//   FocusDetailInfo     — top section (poster + metadata), scroll only
//   FocusDetailCrew     — crew list (directors / writers / composers etc.)
//   FocusDetailCast     — cast list, vertical cursor
//   FocusDetailProvider — STREAM VIA badges, horizontal cursor
//   FocusDetailRelated  — related titles row, horizontal cursor
//
// FocusDetailSimilar is a deprecated alias for FocusDetailRelated retained
// through Chunk 6's transitional commits; Task 6.3 removes it and its
// associated `Similar*` fields once ui.go has been rewired.

import (
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/watchhistory"
)

// DetailFocus is which zone of the detail overlay has keyboard focus.
type DetailFocus int

const (
	FocusDetailInfo     DetailFocus = iota // poster + meta + description
	FocusDetailCrew                        // crew (director/writer/etc.)
	FocusDetailCast                        // cast
	FocusDetailProvider                    // STREAM VIA provider badges
	FocusDetailRelated                     // related titles row
)

// FocusDetailSimilar is a compatibility alias for FocusDetailRelated kept
// during the chunk-6 transition so ui.go continues to compile while the
// follow-up task swaps call sites over to FocusDetailRelated. Removed in
// Task 6.3.
const FocusDetailSimilar = FocusDetailRelated

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
	EnrichStatus  FetchStatus
	CreditsStatus FetchStatus
	ArtworkStatus FetchStatus
	RelatedStatus FetchStatus

	Credits ipc.MetadataPayload
	Artwork ipc.MetadataPayload
	Related ipc.MetadataPayload

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

	// Cast
	CastCursor int
	InfoScroll int

	// Provider selection (STREAM VIA)
	ProviderCursor int // index into Entry.Providers

	// Playback — non-empty while mpv is running for this entry
	NowPlaying *components.NowPlayingState

	// Metadata — populated by streamed DetailMetadataPartial events.
	Meta DetailMetadata

	// Deprecated legacy fields retained during the chunk-6 transition so
	// ui.go continues to compile; Task 6.3 removes them in favour of
	// ds.Meta.Related.Items + ds.Meta.RelatedCursor + ds.Meta.RelatedStatus.
	Similar        []ipc.CatalogEntry
	SimilarCursor  int
	SimilarLoading bool

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
	if p.EntryID != d.Entry.ID {
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
		}
		d.Meta.EnrichStatus = status
	case "credits":
		d.Meta.Credits = p.Payload
		d.Meta.CreditsStatus = status
	case "artwork":
		d.Meta.Artwork = p.Payload
		d.Meta.ArtworkStatus = status
	case "related":
		d.Meta.Related = p.Payload
		d.Meta.RelatedStatus = status
	}
}

// isPayloadEmpty is true when every variant-specific field of p is at its
// zero value.  Used to downgrade a FetchLoaded status to FetchEmpty when
// the runtime streams a struct-shaped payload that contains no data.
func isPayloadEmpty(p ipc.MetadataPayload) bool {
	return len(p.Cast) == 0 && len(p.Crew) == 0 &&
		len(p.Backdrops) == 0 && len(p.Posters) == 0 &&
		len(p.Items) == 0 && p.Studio == nil &&
		len(p.Networks) == 0 && len(p.ExternalIDs) == 0
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
