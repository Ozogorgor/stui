package poster

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// fixtureServer spawns an httptest server that tracks peak concurrency +
// total requests so we can assert both caps.
func fixtureServer(t *testing.T) (*httptest.Server, *int32, *int32) {
	t.Helper()
	var inFlight, peak, total int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		cur := atomic.AddInt32(&inFlight, 1)
		for {
			p := atomic.LoadInt32(&peak)
			if cur <= p || atomic.CompareAndSwapInt32(&peak, p, cur) {
				break
			}
		}
		atomic.AddInt32(&total, 1)
		// Hold briefly so concurrency is observable.
		time.Sleep(50 * time.Millisecond)
		w.Header().Set("Content-Type", "image/png")
		_, _ = w.Write([]byte(fmt.Sprintf("fake-png-%s", r.URL.Path)))
		atomic.AddInt32(&inFlight, -1)
	}))
	// Drain + cancel the pool BEFORE srv.Close so no worker goroutine is
	// still writing files when Go's TempDir cleanup runs. t.Cleanup is LIFO,
	// so we register srv.Close first and the cancel/wait second — the cancel
	// cleanup runs first, then srv.Close.
	t.Cleanup(srv.Close)
	t.Cleanup(func() {
		if global != nil {
			global.cancel()
			global.gwg.Wait() // wait for worker + debouncer goroutines to exit
			global.wg.Wait()  // wait for any downloads that beat the ctx check
		}
	})
	return srv, &peak, &total
}

func TestPool_FetchesAndCaches(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, _, _ := fixtureServer(t)

	url := srv.URL + "/inception.png"
	Global().Enqueue(url)

	// Wait up to 2s for the file to appear.
	path := filepath.Join(os.Getenv("XDG_CACHE_HOME"), "stui", "posters", CacheKey(url))
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("expected cached file at %s, got %v", path, err)
	}
}

func TestPool_InFlightCapIsFour(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, peak, total := fixtureServer(t)

	// 10 distinct URLs enqueued concurrently.
	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			Global().Enqueue(fmt.Sprintf("%s/poster-%d.png", srv.URL, n))
		}(i)
	}
	wg.Wait()

	// Wait for drain.
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && atomic.LoadInt32(total) < 10 {
		time.Sleep(20 * time.Millisecond)
	}
	if got := atomic.LoadInt32(total); got != 10 {
		t.Fatalf("expected 10 total requests, got %d", got)
	}
	if got := atomic.LoadInt32(peak); got > 4 {
		t.Fatalf("concurrency cap breached: peak %d > 4", got)
	}
}

func TestPool_DedupesDuplicateEnqueues(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, _, total := fixtureServer(t)

	url := srv.URL + "/dupe.png"
	for i := 0; i < 5; i++ {
		Global().Enqueue(url)
	}

	// Wait for drain (single request expected).
	time.Sleep(300 * time.Millisecond)
	if got := atomic.LoadInt32(total); got != 1 {
		t.Fatalf("duplicate enqueues should dedup; got %d requests", got)
	}
}

func TestPool_RefreshFiresAtLeastOnce(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	resetPoolForTest()
	srv, _, _ := fixtureServer(t)

	ch := Global().RefreshChan()
	Global().Enqueue(srv.URL + "/notify.png")

	select {
	case <-ch:
		// pass
	case <-time.After(2 * time.Second):
		t.Fatal("RefreshChan never fired after a successful download")
	}
}
