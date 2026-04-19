// Package catalogbrowser provides a reusable 3-column catalog browser
// component for STUI. The package defines a DataSource abstraction that
// the Music Library (MPD-backed) and Music Browse (plugin-backed) screens
// share, along with an optional grid-oriented source for Movies/Series/
// Library tabs.
//
// Tasks from the spec: 5.1 defines types; 5.2 extracts the component;
// 5.3/5.4 provide concrete MPD and plugin data sources; 5.5 adds a source
// picker modal; 5.6 adds lazy sources-count resolution for video.
package catalogbrowser

import (
	"context"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
)

// Entry is the renderable form of a catalog item surfaced by a DataSource.
// It's a superset of both MPD-native fields and plugin-sourced fields; the
// renderer picks which subset to show based on the entry's Kind and the
// DataSource's HasMultipleSources() flag.
type Entry struct {
	ID          string
	Kind        ipc.EntryKind
	Title       string
	Source      string // plugin id or "Local" for MPD
	ArtistName  string
	AlbumName   string
	TrackNumber uint32
	Year        uint32
	Duration    uint32 // seconds
}

// Cursor is the browser's position within a snapshot of columns.
type Cursor struct {
	Column int
	Row    int
	Scroll int
}

// DataSourceState captures enough to restore a DataSource's rendered view
// after a search completes or is cancelled.
type DataSourceState struct {
	Items  map[ipc.EntryKind][]Entry
	Cursor Cursor
}

// SearchStatus reflects the current query state surfaced by a DataSource.
// Used by the renderer to show "searching…" / "partial" indicators.
type SearchStatus struct {
	Active  bool   // true while a query is in flight
	Partial bool   // true while some scopes are still pending
	Query   string // current query string
	QueryID uint64 // IPC query id; zero when !Active
}

// DataSource is the common surface for everything CatalogBrowser renders.
// Implementations:
//
//   - MpdDataSource (Task 5.3): MPD bridge backend, synchronous search.
//   - PluginDataSource (Task 5.4): plugin engine backend, streaming search.
//   - GridDataSource (Task 6.4): flat grid for Movies/Series/Library.
//
// Contract:
//
//   - Items(kind) reflects what the renderer should draw right now.
//   - Search returns a tea.Cmd; the command drives the IPC dance and
//     eventually posts ScopeResultsApplied messages into the Bubbletea
//     loop (or a structured error message on failure).
//   - HasMultipleSources() is stable for the lifetime of the DataSource
//     and is used to toggle the Source/Sources column in CatalogBrowser.
//   - Snapshot is cheap (at least O(items)), invoked when a search starts.
//     Restore restores prior view on Esc / cleared query.
//   - Status returns a fresh copy of the current search state.
type DataSource interface {
	Items(kind ipc.EntryKind) []Entry
	Search(ctx context.Context, query string, kinds []ipc.EntryKind) tea.Cmd
	HasMultipleSources() bool
	Snapshot() DataSourceState
	Restore(s DataSourceState)
	Status() SearchStatus
}
