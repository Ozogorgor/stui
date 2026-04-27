package components

// chafa_cache.go — persistent disk cache for chafa-rendered ANSI text.
//
// Chafa is slow (~50-200ms per shell-out) and runs once per
// (poster, width, height). The grid uses 5 cols × ~5 rows = ~25 cards
// per page, so first paint of a fresh tab can stall for seconds.
//
// This cache stores the rendered ANSI text on disk keyed by a hash of
// (poster_path, mtime, width, height). On a warm cache, hits are a
// single ReadFile — no chafa invocation at all. On miss, the caller
// pays the chafa cost once and writes the result here for next time.
//
// Layout: <xdg-cache-home>/stui/chafa/<sha256-hex>.ansi
// Falls back to ~/.stui/cache/chafa/ when XDG_CACHE_HOME is unset.
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

// chafaCacheDirCached is computed once on first call so we don't
// hammer the env / homedir lookup per render.
var chafaCacheDirCached atomic.Pointer[string]

// chafaCacheDir resolves the cache directory, mirroring the poster
// package pattern. The directory is NOT created here — writers
// (chafaCachePut) create it lazily on first write.
func chafaCacheDir() string {
	if p := chafaCacheDirCached.Load(); p != nil {
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
	chafaCacheDirCached.Store(&dir)
	return dir
}

// chafaCacheKey returns a stable filename for a (poster_path, w, h)
// tuple. Includes the poster file's mtime so the cache invalidates if
// the source image is replaced (e.g., user re-downloads a poster
// behind the same URL).
func chafaCacheKey(posterPath string, w, h int) string {
	mtime := int64(0)
	if info, err := os.Stat(posterPath); err == nil {
		mtime = info.ModTime().UnixNano()
	}
	raw := fmt.Sprintf("%s|%d|%d|%d", posterPath, mtime, w, h)
	sum := sha256.Sum256([]byte(raw))
	return hex.EncodeToString(sum[:]) + ".ansi"
}

// chafaCacheGet returns the cached ANSI text for the given key, or
// (nil, false) if not present / unreadable. Returned bytes are the
// raw chafa output (newline-terminated lines).
func chafaCacheGet(posterPath string, w, h int) ([]byte, bool) {
	if posterPath == "" || w <= 0 || h <= 0 {
		return nil, false
	}
	path := filepath.Join(chafaCacheDir(), chafaCacheKey(posterPath, w, h))
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, false
	}
	if len(data) == 0 {
		return nil, false
	}
	return data, true
}

// chafaCachePut writes the rendered ANSI text to disk for next time.
// Failures are silent — the in-memory cache still serves the live
// session, only the next launch loses the speedup. Callers must NOT
// pass empty data (we use empty as the cache-miss signal).
func chafaCachePut(posterPath string, w, h int, data []byte) {
	if posterPath == "" || w <= 0 || h <= 0 || len(data) == 0 {
		return
	}
	dir := chafaCacheDir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return
	}
	path := filepath.Join(dir, chafaCacheKey(posterPath, w, h))
	// Write to a temp file first so partial writes don't poison the
	// cache; rename is atomic on the same filesystem.
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return
	}
	_ = os.Rename(tmp, path)
}
