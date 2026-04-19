package catalogbrowser

import (
	"context"
	"sync"
	"time"

	tea "charm.land/bubbletea/v2"
)

// SourcesResolver resolves a stream-source count for a given entry id.
// Implementations typically wrap an IPC call into the runtime's plugin
// engine (Streams capability).
type SourcesResolver interface {
	ResolveSourcesCount(ctx context.Context, entryID string) (int, error)
}

// SourcesCountResolver coordinates lazy, hover-triggered resolution of
// sources counts. It tracks the currently-focused entry and only fires a
// resolution request after the cursor has dwelt on the same entry for
// `hoverThreshold`. Results are cached per entry id and returned via
// SourcesCountUpdatedMsg into the Bubbletea loop.
//
// Single-instance per grid — caller passes ticks (e.g., wall-clock or a
// dedicated tea.Tick) to drive the dwell logic.
type SourcesCountResolver struct {
	resolver       SourcesResolver
	hoverThreshold time.Duration

	mu          sync.Mutex
	counts      map[string]int       // entry id → resolved count
	inflight    map[string]bool      // entry ids currently being resolved
	currentID   string               // entry id currently under cursor
	currentSince time.Time           // when the cursor entered currentID
	fired       bool                 // whether dwell threshold has fired for currentID
}

// SourcesCountUpdatedMsg is posted when a resolution completes (success or
// error). On success, Count >= 0; on error, Count == -1 and Err is set.
type SourcesCountUpdatedMsg struct {
	EntryID string
	Count   int
	Err     error
}

// newSourcesCountResolver constructs a resolver. hoverThreshold is the
// dwell time before a resolution fires; spec default is 300ms.
func newSourcesCountResolver(resolver SourcesResolver, hoverThreshold time.Duration) *SourcesCountResolver {
	return &SourcesCountResolver{
		resolver:       resolver,
		hoverThreshold: hoverThreshold,
		counts:         map[string]int{},
		inflight:       map[string]bool{},
	}
}

// NewSourcesCountResolver is the public constructor.
func NewSourcesCountResolver(resolver SourcesResolver) *SourcesCountResolver {
	return newSourcesCountResolver(resolver, 300*time.Millisecond)
}

// Cached returns a previously-resolved count for an entry, plus whether
// the entry has been resolved at all. Renderer uses this to decide
// between "▸" and the integer.
func (r *SourcesCountResolver) Cached(entryID string) (int, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	n, ok := r.counts[entryID]
	return n, ok
}

// OnCursor signals that the cursor has moved to entryID at the given time.
// Returns nil if no immediate work is needed; non-nil cmd if the cached
// state should be re-rendered.
func (r *SourcesCountResolver) OnCursor(entryID string, now time.Time) tea.Cmd {
	r.mu.Lock()
	defer r.mu.Unlock()
	if entryID == r.currentID {
		return nil
	}
	r.currentID = entryID
	r.currentSince = now
	r.fired = false
	return nil
}

// OnTick checks whether the dwell threshold has elapsed for the current
// entry; if so and not already cached/inflight, fires a resolution and
// returns a tea.Cmd that produces SourcesCountUpdatedMsg on completion.
//
// The caller is expected to drive ticks via tea.Tick at a cadence finer
// than hoverThreshold (e.g., every 100ms).
func (r *SourcesCountResolver) OnTick(now time.Time) tea.Cmd {
	r.mu.Lock()
	if r.currentID == "" || r.fired {
		r.mu.Unlock()
		return nil
	}
	if now.Sub(r.currentSince) < r.hoverThreshold {
		r.mu.Unlock()
		return nil
	}
	if _, cached := r.counts[r.currentID]; cached {
		r.fired = true
		r.mu.Unlock()
		return nil
	}
	if r.inflight[r.currentID] {
		r.fired = true
		r.mu.Unlock()
		return nil
	}
	r.inflight[r.currentID] = true
	r.fired = true
	entryID := r.currentID
	resolver := r.resolver
	r.mu.Unlock()

	return func() tea.Msg {
		ctx := context.Background()
		n, err := resolver.ResolveSourcesCount(ctx, entryID)

		r.mu.Lock()
		delete(r.inflight, entryID)
		if err == nil {
			r.counts[entryID] = n
		}
		r.mu.Unlock()

		if err != nil {
			return SourcesCountUpdatedMsg{EntryID: entryID, Count: -1, Err: err}
		}
		return SourcesCountUpdatedMsg{EntryID: entryID, Count: n}
	}
}

// Forget clears the cache for an entry id (e.g., when the underlying
// catalog row changes). Optional helper; not required by the basic flow.
func (r *SourcesCountResolver) Forget(entryID string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.counts, entryID)
	delete(r.inflight, entryID)
}
