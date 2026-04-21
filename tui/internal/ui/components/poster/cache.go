// Package poster owns the TUI-side poster cache + download pool.
//
// Broader asset caching is out of scope for this package; size-based
// eviction lives in the runtime-side caching pass (see
// docs/superpowers/specs/2026-04-22-browse-posters-scrollbar-design.md §8).
package poster

import (
	"crypto/sha256"
	"encoding/hex"
	"net/url"
	"os"
	"path/filepath"
	"strings"
)

// Whitelisted image extensions. Anything else falls back to ".jpg".
var allowedExts = map[string]bool{
	".jpg":  true,
	".jpeg": true,
	".png":  true,
	".webp": true,
	".gif":  true,
}

// CacheKey returns a stable filename for a poster URL:
// `<sha256-hex>.<extension>` where extension is preserved from the URL's
// path (query + fragment stripped first); unknown extensions fall back to
// `.jpg`.
func CacheKey(u string) string {
	sum := sha256.Sum256([]byte(u))
	hash := hex.EncodeToString(sum[:])

	ext := extFromURL(u)
	if !allowedExts[ext] {
		ext = ".jpg"
	}
	return hash + ext
}

// extFromURL extracts the extension from the URL path (not query or fragment).
// Lowercased; returns "" if the path has no extension.
func extFromURL(raw string) string {
	u, err := url.Parse(raw)
	if err != nil {
		return ""
	}
	return strings.ToLower(filepath.Ext(u.Path))
}

// CachedPath returns the absolute on-disk path for a URL's poster, plus
// whether a file currently exists at that path.
func CachedPath(u string) (string, bool) {
	path := filepath.Join(cacheDir(), CacheKey(u))
	if _, err := os.Stat(path); err == nil {
		return path, true
	}
	return path, false
}

// cacheDir resolves to <xdg-cache-home>/stui/posters/, falling back to
// ~/.stui/cache/posters/. The directory is NOT created here — callers
// (pool.go) create it lazily on first successful download.
func cacheDir() string {
	if x := os.Getenv("XDG_CACHE_HOME"); x != "" {
		return filepath.Join(x, "stui", "posters")
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return filepath.Join(os.TempDir(), "stui-posters")
	}
	return filepath.Join(home, ".stui", "cache", "posters")
}
