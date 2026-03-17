package screens

// detail_state.go — state for the detail overlay.
//
// Focus zones:
//   FocusDetailInfo     — top section (poster + metadata), scroll only
//   FocusDetailCast     — cast list, vertical cursor
//   FocusDetailProvider — STREAM VIA badges, horizontal cursor  ← NEW
//   FocusDetailSimilar  — similar titles row, horizontal cursor

import (
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/watchhistory"
)

// DetailFocus is which zone of the detail overlay has keyboard focus.
type DetailFocus int

const (
	FocusDetailInfo     DetailFocus = iota // poster + meta + description
	FocusDetailCast                        // cast & crew list
	FocusDetailProvider                    // STREAM VIA provider badges
	FocusDetailSimilar                     // similar titles row
)

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

	// Similar
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
	}
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
