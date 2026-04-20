package ipc

import (
	"sync"

	"github.com/stui/stui/pkg/log"
)

// scopeSub tracks a single streaming search subscription.
// It is keyed by query_id in Client.scopeSubs.
type scopeSub struct {
	mu            sync.Mutex
	ch            chan ScopeResultsMsg
	expectedScope map[SearchScope]struct{}
	remaining     int
}

// NextQueryID returns a monotonically increasing query id for use in search
// requests. Safe for concurrent use.
func (c *Client) NextQueryID() uint64 {
	return c.nextQueryID.Add(1)
}

// SubscribeScopeResults registers a subscriber for the given query id and
// expected scope set. The returned channel receives ScopeResultsMsg values
// and is closed once every expected scope has emitted a final (partial=false)
// message.
//
// Buffer size 8 accommodates a partial+final pair per scope with slack for
// multiple scopes. If the buffer fills, additional messages are dropped
// silently rather than blocking the read loop.
func (c *Client) SubscribeScopeResults(queryID uint64, scopes []SearchScope) <-chan ScopeResultsMsg {
	expected := make(map[SearchScope]struct{}, len(scopes))
	for _, s := range scopes {
		expected[s] = struct{}{}
	}
	sub := &scopeSub{
		ch:            make(chan ScopeResultsMsg, 8),
		expectedScope: expected,
		remaining:     len(scopes),
	}
	c.scopeSubs.Store(queryID, sub)
	return sub.ch
}

// dispatchScopeResults delivers an incoming ScopeResultsMsg to its subscriber
// (if one is registered) and GCs the entry when every expected scope has
// emitted a final message. Messages for unknown query_ids (stale or already
// finalized) are silently dropped.
func (c *Client) dispatchScopeResults(msg ScopeResultsMsg) {
	v, ok := c.scopeSubs.Load(msg.QueryID)
	if !ok {
		// No subscriber — stale or already GC'd query id. Drop with a debug log
		// so that unexpected drops are visible at verbose log levels.
		log.Debug("ipc: dropping scope_results for unknown query id", "qid", msg.QueryID)
		return
	}
	sub := v.(*scopeSub)

	select {
	case sub.ch <- msg:
	default:
		// Channel full — drop silently. If the TUI is this far behind, more
		// messages won't help and blocking the read loop would be worse.
	}

	if !msg.Partial {
		sub.mu.Lock()
		delete(sub.expectedScope, msg.Scope)
		sub.remaining--
		lastOne := sub.remaining <= 0
		sub.mu.Unlock()

		if lastOne {
			// Delete before close: under the current single-writer invariant
			// this is functionally equivalent to close-then-delete, but the
			// Delete-first order is safer against a future multi-dispatcher
			// refactor where a second goroutine could observe the closed
			// channel via a concurrent Load before the Delete completes.
			c.scopeSubs.Delete(msg.QueryID)
			close(sub.ch)
		}
	}
}
