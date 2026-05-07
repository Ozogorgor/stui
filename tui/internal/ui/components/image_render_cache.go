package components

// image_render_cache.go — persistent disk cache for rendered ANSI text.
//
// (Previously chafa_cache.go. Renamed in the chafa→mosaic cleanup;
// the cache is renderer-agnostic — it just stores rendered ANSI
// bytes keyed by source path/mtime/cell-size, plus a salt that's
// bumped whenever the renderer's output bytes change.)
//
// This cache stores the rendered ANSI text on disk keyed by a hash of
// (renderer_salt, poster_path, mtime, width, height). On a warm cache,
// hits are a single ReadFile — no decode + render at all. On miss, the
// caller pays the render cost once and writes the result here for next
// time.
//
// Layout: <xdg-cache-home>/stui/chafa/<sha256-hex>.ansi
// Falls back to ~/.stui/cache/chafa/ when XDG_CACHE_HOME is unset.
// (Directory name retained for backward compatibility with existing
// users; renaming would orphan their warm caches twice.)
//
// No size cap or LRU. Each cached entry is ~5-50KB; even at 1000
// posters that's well under 50MB. If this ever becomes a problem, a
// mtime-based prune pass on startup is the cheapest fix.

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"sync/atomic"
)

// cacheKeySalt is bumped when the renderer changes (or when a sizing
// fix is shipped) so old cache entries don't get served back. Old
// .ansi files stay on disk but new lookups produce new hashes and
// never match them.
//
// History:
//   mosaic-v1 → first chafa→mosaic swap; cells were under-sized
//   mosaic-v2 → fixed pixel↔cell math (Width/Height take pixels = cells*2)
//   mosaic-v3 → aspect-preserving fit (was stretching to fill, distorting posters)
//   mosaic-v4 → center horizontally + vertically inside the cell box
//   mosaic-v5 → reverted padding; layout owners (card.go) center via lipgloss
const cacheKeySalt = "mosaic-v5"

// imageRenderCacheDirCached is computed once on first call so we don't
// hammer the env / homedir lookup per render.
var imageRenderCacheDirCached atomic.Pointer[string]

// imageRenderCacheDir resolves the cache directory, mirroring the poster
// package pattern. The directory is NOT created here — writers
// (imageRenderCachePut) create it lazily on first write.
func imageRenderCacheDir() string {
	if p := imageRenderCacheDirCached.Load(); p != nil {
		return *p
	}
	var dir string
	if x := os.Getenv("XDG_CACHE_HOME"); x != "" {
		dir = filepath.Join(x, "stui", "chafa")
	} else if home, err := os.UserHomeDir(); err == nil && home != "" {
		dir = filepath.Join(home, ".cache", "stui", "chafa")
	} else {
		dir = filepath.Join(os.TempDir(), "stui-chafa")
	}
	imageRenderCacheDirCached.Store(&dir)
	return dir
}

// imageRenderCacheKey returns a stable filename for a (poster_path, w, h)
// tuple. Includes the poster file's mtime so the cache invalidates if
// the source image is replaced (e.g., user re-downloads a poster
// behind the same URL).
func imageRenderCacheKey(posterPath string, w, h int) string {
	mtime := int64(0)
	if info, err := os.Stat(posterPath); err == nil {
		mtime = info.ModTime().UnixNano()
	}
	raw := fmt.Sprintf("%s|%s|%d|%d|%d", cacheKeySalt, posterPath, mtime, w, h)
	sum := sha256.Sum256([]byte(raw))
	return hex.EncodeToString(sum[:]) + ".ansi"
}

// imageRenderCacheGet returns the cached ANSI text for the given key, or
// (nil, false) if not present / unreadable. Returned bytes are the
// raw renderer output (newline-terminated lines).
func imageRenderCacheGet(posterPath string, w, h int) ([]byte, bool) {
	if posterPath == "" || w <= 0 || h <= 0 {
		return nil, false
	}
	path := filepath.Join(imageRenderCacheDir(), imageRenderCacheKey(posterPath, w, h))
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, false
	}
	if len(data) == 0 {
		return nil, false
	}
	return data, true
}

// imageRenderCachePut writes the rendered ANSI text to disk for next
// time. Failures are silent — the in-memory cache still serves the
// live session, only the next launch loses the speedup. Callers must
// NOT pass empty data (we use empty as the cache-miss signal).
func imageRenderCachePut(posterPath string, w, h int, data []byte) {
	if posterPath == "" || w <= 0 || h <= 0 || len(data) == 0 {
		return
	}
	dir := imageRenderCacheDir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return
	}
	path := filepath.Join(dir, imageRenderCacheKey(posterPath, w, h))
	// Write to a temp file first so partial writes don't poison the
	// cache; rename is atomic on the same filesystem.
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return
	}
	_ = os.Rename(tmp, path)
}
