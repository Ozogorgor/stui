package components

// image_render_pool.go — async image-render worker pool.
//
// (Previously chafa_pool.go. Renamed in the chafa→mosaic cleanup;
// the pool is renderer-agnostic — it dispatches whatever renderMosaic
// returns.)
//
// Synchronous renders (~50-200ms with chafa, much less with mosaic
// but still non-trivial decode) blocked the View() path. With ~25
// visible cards, first paint of a fresh tab stalled for seconds. This
// pool runs renders in N background workers, fills the disk + memory
// caches as they complete, and signals the controller via an
// `ImageRenderedMsg` so cards re-render with real images as they
// become ready.
//
// Flow per card render:
//   1. ImageView.Lines() checks the L1 in-memory cache. Hit → return.
//   2. Miss → checks the L2 disk cache. Hit → load + populate L1 → return.
//   3. Miss → enqueue an async job (idempotent per (path, w, h)),
//      return placeholder lines, fire ImageRenderedMsg later.
//
// Single-flight: concurrent Lines() for the same key share one
// pending job. The pool dedups via a `pending` map keyed by the
// disk-cache filename (which already encodes path+w+h+mtime).

import (
	"sync"

	tea "charm.land/bubbletea/v2"
)

// imageRenderJob is one unit of work for the pool.
type imageRenderJob struct {
	path     string
	width    int
	height   int
	cacheKey string // imageRenderCacheKey(path, w, h) — also dedup key
}

// ImageRenderedMsg is dispatched after a background image render
// completes. The controller catches it and triggers a View()
// refresh; the next Lines() call hits the cache and returns the
// real image.
type ImageRenderedMsg struct {
	// Path is the source poster path (the file the renderer rendered).
	// Cards re-rendering on this msg compare it to their own path
	// to decide whether they need to re-paint.
	Path string
}

var (
	imageRenderPoolOnce sync.Once
	imageRenderPool     *imageRenderWorkerPool
)

// imageRenderPoolGlobal returns the lazy-initialized process-wide pool.
func imageRenderPoolGlobal() *imageRenderWorkerPool {
	imageRenderPoolOnce.Do(func() {
		imageRenderPool = newImageRenderWorkerPool(imageRenderPoolWorkers)
	})
	return imageRenderPool
}

// Number of concurrent render workers. Each one runs a JPEG/PNG/WebP
// decode + mosaic render in goroutine. 4 caps the in-flight CPU work
// without starving the rest of the TUI; bumping higher buys little
// extra throughput once decode saturates the cores.
const imageRenderPoolWorkers = 4

type imageRenderWorkerPool struct {
	jobs    chan imageRenderJob
	done    chan ImageRenderedMsg
	mu      sync.Mutex
	pending map[string]bool // cacheKey → in-flight
}

func newImageRenderWorkerPool(n int) *imageRenderWorkerPool {
	p := &imageRenderWorkerPool{
		// Buffered job queue; if it fills the caller falls back to
		// synchronous render (rare — would need 1000+ outstanding
		// posters at once).
		jobs:    make(chan imageRenderJob, 256),
		done:    make(chan ImageRenderedMsg, 256),
		pending: make(map[string]bool),
	}
	for i := 0; i < n; i++ {
		go p.worker()
	}
	return p
}

// Enqueue schedules an image render. Returns true if the job was
// accepted (or was already pending — single-flight). Returns false
// if the queue is full; the caller should fall back to synchronous
// render to avoid losing the request.
func (p *imageRenderWorkerPool) Enqueue(j imageRenderJob) bool {
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
// image render and emits an ImageRenderedMsg. The controller subscribes
// once at startup and re-subscribes after each msg fires (same
// long-lived-listener pattern as listenIPC).
func (p *imageRenderWorkerPool) PollMsg() tea.Cmd {
	return func() tea.Msg {
		return <-p.done
	}
}

func (p *imageRenderWorkerPool) worker() {
	for j := range p.jobs {
		// Render. We re-check the disk cache first in case another
		// worker (or a synchronous render path) populated it after
		// the job was enqueued.
		if _, hit := imageRenderCacheGet(j.path, j.width, j.height); !hit {
			out, err := renderMosaic(j.path, j.width, j.height)
			if err == nil && len(out) > 0 {
				imageRenderCachePut(j.path, j.width, j.height, out)
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
		case p.done <- ImageRenderedMsg{Path: j.path}:
		default:
		}
	}
}

// EnqueueImageRender is the package-level helper card.go calls
// when a poster's image is cached on disk but the renderer hasn't
// processed it yet. Idempotent and non-blocking.
func EnqueueImageRender(path string, w, h int) {
	imageRenderPoolGlobal().Enqueue(imageRenderJob{
		path:     path,
		width:    w,
		height:   h,
		cacheKey: imageRenderCacheKey(path, w, h),
	})
}

// ImageRenderPollCmd returns a Bubbletea Cmd that delivers the next
// ImageRenderedMsg. Wired into Init() the same way listenIPC is.
func ImageRenderPollCmd() tea.Cmd {
	return imageRenderPoolGlobal().PollMsg()
}
