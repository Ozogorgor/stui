package components

// chafa_pool.go — async chafa-render worker pool.
//
// Synchronous chafa shell-outs (~50-200ms each) blocked the View()
// render path; with ~25 visible cards, first paint of a fresh tab
// stalled for seconds. This pool runs chafa in N background workers,
// fills the disk + memory caches as renders complete, and signals
// the controller via a `ChafaRenderedMsg` so cards re-render with
// real images as they become ready.
//
// Flow per card render:
//   1. ImageView.Lines() checks the L1 in-memory cache. Hit → return.
//   2. Miss → checks the L2 disk cache. Hit → load + populate L1 → return.
//   3. Miss → enqueue an async job (idempotent per (path, w, h)),
//      return placeholder lines, fire ChafaRenderedMsg later.
//
// Single-flight: concurrent Lines() for the same key share one
// pending job. The pool dedups via a `pending` map keyed by the
// disk-cache filename (which already encodes path+w+h+mtime).

import (
	"os/exec"
	"strings"
	"sync"

	tea "charm.land/bubbletea/v2"
)

// chafaJob is one unit of work for the pool.
type chafaJob struct {
	path     string
	width    int
	height   int
	format   string // "symbols" | "kitty"
	cacheKey string // chafaCacheKey(path, w, h) — also dedup key
}

// ChafaRenderedMsg is dispatched after a background chafa render
// completes. The controller catches it and triggers a View()
// refresh; the next Lines() call hits the cache and returns the
// real image.
type ChafaRenderedMsg struct {
	// Path is the source poster path (the file chafa rendered).
	// Cards re-rendering on this msg compare it to their own path
	// to decide whether they need to re-paint.
	Path string
}

var (
	chafaPoolOnce sync.Once
	chafaPool     *chafaWorkerPool
)

// chafaPoolGlobal returns the lazy-initialized process-wide pool.
func chafaPoolGlobal() *chafaWorkerPool {
	chafaPoolOnce.Do(func() {
		chafaPool = newChafaWorkerPool(chafaPoolWorkers)
	})
	return chafaPool
}

// Number of concurrent chafa workers. Each spawns its own process,
// so this caps the number of in-flight forks. 4 strikes a balance
// between throughput (more workers = parallel renders) and CPU/RAM
// (each chafa peaks at ~50-100MB during decode).
const chafaPoolWorkers = 4

type chafaWorkerPool struct {
	jobs    chan chafaJob
	done    chan ChafaRenderedMsg
	mu      sync.Mutex
	pending map[string]bool // cacheKey → in-flight
}

func newChafaWorkerPool(n int) *chafaWorkerPool {
	p := &chafaWorkerPool{
		// Buffered job queue; if it fills the caller falls back to
		// synchronous render (rare — would need 1000+ outstanding
		// posters at once).
		jobs:    make(chan chafaJob, 256),
		done:    make(chan ChafaRenderedMsg, 256),
		pending: make(map[string]bool),
	}
	for i := 0; i < n; i++ {
		go p.worker()
	}
	return p
}

// Enqueue schedules a chafa render. Returns true if the job was
// accepted (or was already pending — single-flight). Returns false
// if the queue is full; the caller should fall back to synchronous
// render to avoid losing the request.
func (p *chafaWorkerPool) Enqueue(j chafaJob) bool {
	p.mu.Lock()
	if p.pending[j.cacheKey] {
		p.mu.Unlock()
		return true // already in flight; nothing to do
	}
	p.pending[j.cacheKey] = true
	p.mu.Unlock()

	select {
	case p.jobs <- j:
		return true
	default:
		// Queue is full — drop the pending flag so a future caller
		// can retry, and signal the caller to render synchronously.
		p.mu.Lock()
		delete(p.pending, j.cacheKey)
		p.mu.Unlock()
		return false
	}
}

// PollMsg returns a Bubbletea Cmd that waits for the next completed
// chafa render and emits a ChafaRenderedMsg. The controller subscribes
// once at startup and re-subscribes after each msg fires (same
// long-lived-listener pattern as listenIPC).
func (p *chafaWorkerPool) PollMsg() tea.Cmd {
	return func() tea.Msg {
		return <-p.done
	}
}

func (p *chafaWorkerPool) worker() {
	for j := range p.jobs {
		// Render. We re-check the disk cache first in case another
		// worker (or a synchronous render path) populated it after
		// the job was enqueued.
		if _, hit := chafaCacheGet(j.path, j.width, j.height); !hit {
			cmd := exec.Command("chafa",
				"--format", j.format,
				"--size", formatSize(j.width, j.height),
				"--animate", "off",
				j.path,
			)
			out, err := cmd.Output()
			if err == nil && len(out) > 0 {
				chafaCachePut(j.path, j.width, j.height, out)
			}
			// On error we still drop the pending flag below — the
			// disk-cache miss persists, the card stays on its
			// placeholder, and the next render attempt may succeed
			// (if the file is a transient decode failure).
		}

		p.mu.Lock()
		delete(p.pending, j.cacheKey)
		p.mu.Unlock()

		// Non-blocking send: if the controller hasn't drained the
		// queue, drop the message. The cache write above is the
		// load-bearing part — re-renders trigger naturally on the
		// next View() pass via cache hit.
		select {
		case p.done <- ChafaRenderedMsg{Path: j.path}:
		default:
		}
	}
}

// formatSize avoids pulling in fmt for one Sprintf — keeps this
// hot-path file's import surface minimal.
func formatSize(w, h int) string {
	return itoa(w) + "x" + itoa(h)
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	neg := n < 0
	if neg {
		n = -n
	}
	var b strings.Builder
	for n > 0 {
		b.WriteByte('0' + byte(n%10))
		n /= 10
	}
	out := b.String()
	// reverse
	r := []byte(out)
	for i, j := 0, len(r)-1; i < j; i, j = i+1, j-1 {
		r[i], r[j] = r[j], r[i]
	}
	if neg {
		return "-" + string(r)
	}
	return string(r)
}

// EnqueueChafaRender is the package-level helper card.go calls
// when a poster's image is cached on disk but chafa hasn't rendered
// it yet. Idempotent and non-blocking.
func EnqueueChafaRender(path string, w, h int) {
	chafaPoolGlobal().Enqueue(chafaJob{
		path:     path,
		width:    w,
		height:   h,
		format:   "symbols",
		cacheKey: chafaCacheKey(path, w, h),
	})
}

// ChafaPollCmd returns a Bubbletea Cmd that delivers the next
// ChafaRenderedMsg. Wired into Init() the same way listenIPC is.
func ChafaPollCmd() tea.Cmd {
	return chafaPoolGlobal().PollMsg()
}
