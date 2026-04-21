package poster

import (
	"context"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const (
	poolWorkers     = 4
	refreshDebounce = 150 * time.Millisecond
	httpTimeout     = 15 * time.Second
)

// Pool downloads poster URLs concurrently into the on-disk cache and
// emits a debounced refresh signal when any download completes.
type Pool struct {
	ctx      context.Context
	cancel   context.CancelFunc // aborts all in-flight HTTP requests; used by resetPoolForTest
	queue    chan string
	refresh  chan struct{}
	inFlight sync.Map   // url -> struct{}
	client   *http.Client
	wg       sync.WaitGroup // tracks active downloads; used by resetPoolForTest
}

var (
	globalOnce sync.Once
	global     *Pool
)

// Global returns the process-wide Pool, lazily spawning 4 workers on first
// call. Safe for concurrent use.
func Global() *Pool {
	globalOnce.Do(func() {
		global = newPool()
		global.start()
	})
	return global
}

// resetPoolForTest cancels all in-flight downloads on the current global pool,
// waits for workers to exit, then replaces the singleton so each test starts
// fresh. Cancelling is faster than draining and ensures no goroutine is still
// writing to a TempDir that Go's test framework is about to clean up.
// Only callable from tests in this package.
func resetPoolForTest() {
	if global != nil {
		global.cancel()  // abort in-flight HTTP requests immediately
		global.wg.Wait() // wait for all workers to acknowledge cancellation
	}
	globalOnce = sync.Once{}
	global = nil
}

func newPool() *Pool {
	ctx, cancel := context.WithCancel(context.Background())
	return &Pool{
		ctx:     ctx,
		cancel:  cancel,
		queue:   make(chan string, 256),
		refresh: make(chan struct{}, 1), // buffered(1): non-blocking send coalesces bursts
		client:  &http.Client{Timeout: httpTimeout},
	}
}

func (p *Pool) start() {
	// Spawn a debouncer: every refreshDebounce window, if any completion
	// landed, emit one struct{} to the observable refresh channel.
	raw := make(chan struct{}, 256)
	go func() {
		var pending bool
		tick := time.NewTicker(refreshDebounce)
		defer tick.Stop()
		for {
			select {
			case <-raw:
				pending = true
			case <-tick.C:
				if pending {
					pending = false
					select {
					case p.refresh <- struct{}{}:
					default: // already pending; coalesce
					}
				}
			}
		}
	}()

	for i := 0; i < poolWorkers; i++ {
		go p.worker(raw)
	}
}

func (p *Pool) worker(doneSignal chan<- struct{}) {
	for url := range p.queue {
		// Check cache again under worker — another worker might have raced.
		if _, hit := CachedPath(url); hit {
			p.inFlight.Delete(url)
			p.wg.Done()
			continue
		}
		if err := p.download(url); err == nil {
			select {
			case doneSignal <- struct{}{}:
			default:
			}
		}
		p.inFlight.Delete(url)
		p.wg.Done()
	}
}

// Enqueue posts a URL for background download. Idempotent — duplicate
// URLs currently queued or in-flight are dropped. Already-cached URLs are
// dropped too (avoids pointless queue pressure).
func (p *Pool) Enqueue(url string) {
	if url == "" {
		return
	}
	if _, hit := CachedPath(url); hit {
		return
	}
	if _, loaded := p.inFlight.LoadOrStore(url, struct{}{}); loaded {
		return
	}
	p.wg.Add(1)
	// Non-blocking send; dropping rather than stalling the UI thread is
	// acceptable for a best-effort poster cache. The in-flight map is
	// cleaned up so a later enqueue can retry.
	select {
	case p.queue <- url:
	default:
		p.inFlight.Delete(url)
		p.wg.Done()
	}
}

// RefreshChan returns a buffered(1) channel that fires at least once per
// debounced window when any download completes. See spec §4.2: the channel
// is buffered(1) with non-blocking send on the pool side so a slow
// consumer cannot stall the workers.
func (p *Pool) RefreshChan() <-chan struct{} {
	return p.refresh
}

// download fetches URL and writes the body to the cache path via an
// atomic tmp-file + rename so partial reads never happen.
func (p *Pool) download(url string) error {
	path, hit := CachedPath(url)
	if hit {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(p.ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	resp, err := p.client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return &httpErr{status: resp.StatusCode}
	}
	// Write to a tmp file in the system temp directory so the destination
	// directory never contains a partial file. os.Rename is atomic when src
	// and dst are on the same filesystem; cross-device falls back to copy.
	f, err := os.CreateTemp("", "stui-poster-*.tmp")
	if err != nil {
		return err
	}
	tmp := f.Name()
	if _, err := io.Copy(f, resp.Body); err != nil {
		_ = f.Close()
		_ = os.Remove(tmp)
		return err
	}
	if err := f.Close(); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		// Cross-device rename: fall back to copy.
		if err2 := copyFile(tmp, path); err2 != nil {
			_ = os.Remove(tmp)
			return err2
		}
		_ = os.Remove(tmp)
	}
	return nil
}

// copyFile copies src to dst. Used as a cross-device fallback for Rename.
func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		_ = out.Close()
		_ = os.Remove(dst)
		return err
	}
	return out.Close()
}

type httpErr struct{ status int }

func (e *httpErr) Error() string { return http.StatusText(e.status) }

// PollRefresh returns a tea.Cmd that blocks on the next refresh signal and
// emits a PostersUpdatedMsg. The caller's Update handler is responsible
// for re-arming the Cmd after each receive to keep the subscription alive.
//
// The `any` return type avoids a cross-package dependency from the poster
// package on Bubbletea — callers adapt via `tea.Cmd(poster.PollRefresh())`.
func PollRefresh() func() any {
	return func() any {
		<-Global().RefreshChan()
		return PostersUpdatedMsg{}
	}
}
