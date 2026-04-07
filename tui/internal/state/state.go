package state

// Tab represents a top-level navigation tab
type Tab int

const (
	TabMovies Tab = iota
	TabSeries
	TabMusic
	TabLibrary
	TabCollections
)

func (t Tab) String() string {
	switch t {
	case TabMovies:
		return "Movies"
	case TabSeries:
		return "Series"
	case TabMusic:
		return "Music"
	case TabLibrary:
		return "Library"
	case TabCollections:
		return "Collections"
	default:
		return "Unknown"
	}
}

// TabFromString maps a String() value back to its Tab constant.
// Returns TabMovies for unrecognised values.
func TabFromString(s string) Tab {
	switch s {
	case "Movies":
		return TabMovies
	case "Series":
		return TabSeries
	case "Music":
		return TabMusic
	case "Library":
		return TabLibrary
	case "Collections":
		return TabCollections
	default:
		return TabMovies
	}
}

// MediaTabID maps a Tab to the wire string expected by the runtime.
// Collections is a local-only tab and has no runtime equivalent.
func (t Tab) MediaTabID() string {
	switch t {
	case TabMovies:
		return "movies"
	case TabSeries:
		return "series"
	case TabMusic:
		return "music"
	case TabLibrary:
		return "library"
	case TabCollections:
		return "collections"
	default:
		return "movies"
	}
}

// FocusArea tracks which UI zone has keyboard focus
type FocusArea int

const (
	FocusTabs FocusArea = iota
	FocusSearch
	FocusResults
	FocusSettings
)

// RuntimeStatus describes the connection state to stui-runtime
type RuntimeStatus int

const (
	RuntimeDisconnected RuntimeStatus = iota
	RuntimeConnecting
	RuntimeReady
	RuntimeError
)

func (r RuntimeStatus) String() string {
	switch r {
	case RuntimeDisconnected:
		return "disconnected"
	case RuntimeConnecting:
		return "connecting…"
	case RuntimeReady:
		return "ready"
	case RuntimeError:
		return "error"
	default:
		return "unknown"
	}
}

// ResultItem is a single row in the results list
type ResultItem struct {
	ID       string
	Title    string
	Year     string
	Genre    string
	Rating   string
	Provider string
}

// AppState holds all mutable UI state
type AppState struct {
	ActiveTab    Tab
	Focus        FocusArea
	SearchQuery  string
	SearchActive bool

	// Results
	Results      []ResultItem
	Cursor       int
	IsLoading    bool
	LoadingStart int64 // timestamp when loading started (for timeout)

	// Runtime connection
	RuntimeStatus  RuntimeStatus
	RuntimeError   string
	RuntimeVersion string   // semver string from the runtime binary, e.g. "0.8.1"
	Plugins        []string // loaded plugin names

	// Layout
	Width  int
	Height int

	// Status bar
	StatusMsg string

	// Structured sub-state — see app_state.go
	CurrentMedia  CurrentMedia
	CurrentStream CurrentStream
	Settings      Settings
}

// Tabs returns all tabs in order.
func Tabs() []Tab {
	return []Tab{TabMovies, TabSeries, TabMusic, TabLibrary, TabCollections}
}

// NewAppState returns a fresh default state
func NewAppState() AppState {
	return AppState{
		ActiveTab:     TabMovies,
		Focus:         FocusTabs,
		RuntimeStatus: RuntimeDisconnected,
		StatusMsg:     "Starting runtime…",
		Settings:      DefaultSettings(),
	}
}

// ViewMode toggles between grid (poster) and list view
type ViewMode int

const (
	ViewGrid ViewMode = iota
	ViewList
)
