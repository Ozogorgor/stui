package screens

import (
	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
)

// Searchable is implemented by screens that participate in focus-scoped
// search. The root model probes the focused screen with a type-assertion
// before activating the top-bar input on `/`. Screens that do not
// implement this interface leave the search bar in its default idle state
// (showing "Search… /") and ignore the keystroke entirely.
//
// Lifecycle (driven by the root model):
//
//  1. User presses `/` → root model calls SearchScopes() to confirm
//     eligibility. If the slice is empty, the bar stays hidden.
//  2. SearchPlaceholder() supplies the input's placeholder text
//     (e.g. "Search library…").
//  3. As the user types, the root model debounces and calls
//     StartSearch(query) — the returned tea.Cmd dispatches the IPC.
//  4. Streaming scope results arrive as ipc.ScopeResultsMsg → routed
//     via MusicScreen.ApplyScopeResults (which asserts to the concrete
//     sub-screen type and calls HandleScopeResults). Synchronous MPD
//     results arrive as ipc.MpdSearchResult → via ApplyMpdSearchResult.
//  5. Esc / cleared input → RestoreView() is called on MusicScreen
//     which delegates to the active sub-screen.
//
// Routing note: because Music sub-screens (MusicLibraryScreen,
// MusicBrowseScreen) do not implement tea.Model — their Update methods
// return concrete types, not (tea.Model, tea.Cmd) — the result-routing
// methods are NOT included in this interface. Instead, MusicScreen exposes
// typed Apply* methods (ApplyScopeResults, ApplyMpdSearchResult,
// ApplyRestoreView) that assert to the concrete sub-screen type
// internally. For grid-tab screens (Task 6.4) that do implement tea.Model,
// the root model will use a similar typed pattern keyed by ActiveTab.
//
// Implementations live alongside the screen they belong to:
//   - Music Library  — Task 6.2  (MusicLibraryScreen)
//   - Music Browse   — Task 6.3  (MusicBrowseScreen)
//   - Movies / Series / Library grids — Task 6.4
type Searchable interface {
	// SearchScopes returns the scopes this screen searches in. An empty
	// slice signals that the screen is not currently searchable (the root
	// model will not activate the bar and the keystroke is ignored).
	SearchScopes() []ipc.SearchScope

	// SearchPlaceholder is shown as placeholder text in the search input
	// while the bar is focused (e.g. "Search library…").
	SearchPlaceholder() string

	// StartSearch dispatches a search for query. The returned tea.Cmd
	// is batched by the root model alongside debounce logic and
	// propagated to the Bubble Tea runtime.
	StartSearch(query string) tea.Cmd
}
