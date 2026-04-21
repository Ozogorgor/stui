package poster

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestCacheKey_Stable(t *testing.T) {
	a := CacheKey("https://image.tmdb.org/t/p/w342/foo.jpg")
	b := CacheKey("https://image.tmdb.org/t/p/w342/foo.jpg")
	if a != b {
		t.Fatalf("CacheKey should be stable, got %q vs %q", a, b)
	}
}

func TestCacheKey_DifferentURLsDiffer(t *testing.T) {
	a := CacheKey("https://a/x.jpg")
	b := CacheKey("https://a/y.jpg")
	if a == b {
		t.Fatalf("different URLs should hash differently: %q == %q", a, b)
	}
}

func TestCacheKey_PreservesWhitelistedExtensions(t *testing.T) {
	tests := []struct {
		url, wantExt string
	}{
		{"https://a/poster.jpg", ".jpg"},
		{"https://a/poster.jpeg", ".jpeg"},
		{"https://a/poster.png", ".png"},
		{"https://a/poster.webp", ".webp"},
		{"https://a/poster.gif", ".gif"},
		{"https://a/poster.BMP", ".jpg"}, // non-whitelisted → fallback
		{"https://a/poster", ".jpg"},      // no extension → fallback
	}
	for _, tc := range tests {
		got := filepath.Ext(CacheKey(tc.url))
		if !strings.EqualFold(got, tc.wantExt) {
			t.Errorf("CacheKey(%q) ext = %q, want %q", tc.url, got, tc.wantExt)
		}
	}
}

func TestCacheKey_StripsQueryAndFragment(t *testing.T) {
	// `?v=…` cache-busters must not fool the extension whitelist.
	got := filepath.Ext(CacheKey("https://a/poster.jpg?v=123&x=y#section"))
	if got != ".jpg" {
		t.Fatalf("query/fragment should be stripped before ext detection; got %q", got)
	}
}

func TestCachedPath_ReportsHitMiss(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	url := "https://a/test.jpg"
	path, hit := CachedPath(url)
	if hit {
		t.Fatalf("should be miss on empty cache dir, got hit=%v path=%q", hit, path)
	}
	// Populate the cached file and re-check.
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("fake"), 0o644); err != nil {
		t.Fatal(err)
	}
	path2, hit2 := CachedPath(url)
	if !hit2 || path2 != path {
		t.Fatalf("should be hit now; got hit=%v path=%q", hit2, path2)
	}
}
