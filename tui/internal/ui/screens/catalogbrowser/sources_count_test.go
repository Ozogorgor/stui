package catalogbrowser

import (
	"context"
	"errors"
	"testing"
	"time"
)

type mockResolver struct {
	result int
	err    error
	calls  int
}

func (m *mockResolver) ResolveSourcesCount(_ context.Context, _ string) (int, error) {
	m.calls++
	return m.result, m.err
}

func TestSourcesCountResolver_DoesNotFireBeforeThreshold(t *testing.T) {
	r := newSourcesCountResolver(&mockResolver{result: 42}, 300*time.Millisecond)
	now := time.Now()
	if cmd := r.OnCursor("X", now); cmd != nil {
		t.Fatal("OnCursor should not return a cmd")
	}
	if cmd := r.OnTick(now.Add(200 * time.Millisecond)); cmd != nil {
		t.Fatal("OnTick before threshold should be nil")
	}
}

func TestSourcesCountResolver_FiresAfterThreshold(t *testing.T) {
	res := &mockResolver{result: 42}
	r := newSourcesCountResolver(res, 300*time.Millisecond)
	now := time.Now()
	r.OnCursor("X", now)

	cmd := r.OnTick(now.Add(400 * time.Millisecond))
	if cmd == nil {
		t.Fatal("expected cmd to fire")
	}

	msg := cmd().(SourcesCountUpdatedMsg)
	if msg.EntryID != "X" || msg.Count != 42 {
		t.Fatalf("unexpected: %+v", msg)
	}

	if n, ok := r.Cached("X"); !ok || n != 42 {
		t.Fatalf("not cached: n=%d ok=%v", n, ok)
	}
}

func TestSourcesCountResolver_CursorMoveResetsTimer(t *testing.T) {
	res := &mockResolver{result: 7}
	r := newSourcesCountResolver(res, 300*time.Millisecond)
	now := time.Now()
	r.OnCursor("X", now)
	r.OnCursor("Y", now.Add(100*time.Millisecond))

	cmd := r.OnTick(now.Add(400 * time.Millisecond))
	if cmd == nil {
		t.Fatal("expected fire on Y")
	}
	msg := cmd().(SourcesCountUpdatedMsg)
	if msg.EntryID != "Y" {
		t.Fatalf("expected Y, got %s", msg.EntryID)
	}
	if res.calls != 1 {
		t.Fatalf("expected 1 resolver call, got %d", res.calls)
	}
}

func TestSourcesCountResolver_CachedDoesNotRefire(t *testing.T) {
	res := &mockResolver{result: 5}
	r := newSourcesCountResolver(res, 300*time.Millisecond)
	now := time.Now()
	r.OnCursor("X", now)
	cmd := r.OnTick(now.Add(400 * time.Millisecond))
	cmd()

	// Move away and back
	r.OnCursor("Y", now.Add(500*time.Millisecond))
	r.OnCursor("X", now.Add(600*time.Millisecond))
	if cmd2 := r.OnTick(now.Add(1000 * time.Millisecond)); cmd2 != nil {
		t.Fatal("re-firing on cached entry")
	}
	if res.calls != 1 {
		t.Fatalf("resolver called %d times, expected 1", res.calls)
	}
}

func TestSourcesCountResolver_FiresOnceWhileInflight(t *testing.T) {
	// OnTick after threshold sets inflight; subsequent OnTicks while inflight
	// must not refire.
	res := &mockResolver{result: 5}
	r := newSourcesCountResolver(res, 300*time.Millisecond)
	now := time.Now()
	r.OnCursor("X", now)
	cmd1 := r.OnTick(now.Add(400 * time.Millisecond))
	if cmd1 == nil {
		t.Fatal("first tick should fire")
	}
	cmd2 := r.OnTick(now.Add(500 * time.Millisecond))
	if cmd2 != nil {
		t.Fatal("second tick before completion should be nil")
	}
	cmd1()
	// After completion, cache prevents refire too
	cmd3 := r.OnTick(now.Add(600 * time.Millisecond))
	if cmd3 != nil {
		t.Fatal("third tick after completion should be nil (cached)")
	}
}

func TestSourcesCountResolver_ErrorPath(t *testing.T) {
	res := &mockResolver{err: errors.New("boom")}
	r := newSourcesCountResolver(res, 300*time.Millisecond)
	now := time.Now()
	r.OnCursor("X", now)
	cmd := r.OnTick(now.Add(400 * time.Millisecond))
	msg := cmd().(SourcesCountUpdatedMsg)
	if msg.Count != -1 || msg.Err == nil {
		t.Fatalf("expected error msg, got %+v", msg)
	}
	// Not cached on error
	if _, ok := r.Cached("X"); ok {
		t.Fatal("error result should not be cached")
	}
}
