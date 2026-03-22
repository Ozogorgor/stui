// Package mediacache persists catalog grid data locally so STUI can show a
// browseable offline library when providers are unreachable or the runtime
// fails to start.
//
// The cache is a single JSON file (~/.config/stui/mediacache.json) containing
// a map of tab IDs to a CachedTab envelope.  It is written atomically whenever
// the runtime pushes a live (non-cache) GridUpdateMsg, so it always reflects
// the most recent successful catalog fetch.
package mediacache

import (
	"encoding/json"
	"os"
	"path/filepath"
	"time"

	"github.com/stui/stui/internal/ipc"
)

// StoreInterface defines the interface for media cache stores.
type StoreInterface interface {
	SaveTab(tab string, entries []ipc.CatalogEntry)
	EntriesForTab(tab string) []ipc.CatalogEntry
	AllEntries() []ipc.CatalogEntry
	TotalCount() int
	Clear() error
	LastUpdated() int64
}

// CachedTab is one tab's worth of saved catalog data.
type CachedTab struct {
	Tab       string             `json:"tab"`
	Entries   []ipc.CatalogEntry `json:"entries"`
	UpdatedAt int64              `json:"updated_at"` // unix timestamp
}

// Store holds the full on-disk media cache.
type Store struct {
	Tabs map[string]CachedTab `json:"tabs"`
	path string               `json:"-"`
}

// DefaultPath returns the platform-appropriate path to mediacache.json.
func DefaultPath() string {
	dir, _ := os.UserConfigDir()
	return filepath.Join(dir, "stui", "mediacache.json")
}

// Load reads the cache file and returns a Store.
// Returns an empty store (not nil) if the file is absent or invalid.
func Load(path string) *Store {
	s := &Store{
		Tabs: make(map[string]CachedTab),
		path: path,
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return s
	}
	_ = json.Unmarshal(data, s)
	if s.Tabs == nil {
		s.Tabs = make(map[string]CachedTab)
	}
	return s
}

// Save writes the store to disk atomically.
func (s *Store) Save() error {
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(s.path), 0o755); err != nil {
		return err
	}
	tmp := s.path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, s.path)
}

// SaveTab updates the in-memory cache for a tab and persists it asynchronously.
// This should be called on every live GridUpdateMsg so the cache stays fresh.
func (s *Store) SaveTab(tab string, entries []ipc.CatalogEntry) {
	s.Tabs[tab] = CachedTab{
		Tab:       tab,
		Entries:   entries,
		UpdatedAt: time.Now().Unix(),
	}
	go func() { _ = s.Save() }()
}

// AllEntries returns all cached entries across all tabs in an unspecified order.
func (s *Store) AllEntries() []ipc.CatalogEntry {
	var out []ipc.CatalogEntry
	for _, ct := range s.Tabs {
		out = append(out, ct.Entries...)
	}
	return out
}

// EntriesForTab returns cached entries for a single tab, or nil if absent.
func (s *Store) EntriesForTab(tab string) []ipc.CatalogEntry {
	if ct, ok := s.Tabs[tab]; ok {
		return ct.Entries
	}
	return nil
}

// TotalCount returns the total number of cached entries across all tabs.
func (s *Store) TotalCount() int {
	n := 0
	for _, ct := range s.Tabs {
		n += len(ct.Entries)
	}
	return n
}

// Clear removes all cached data and deletes the file.
func (s *Store) Clear() error {
	s.Tabs = make(map[string]CachedTab)
	_ = os.Remove(s.path)
	return nil
}

// UpdatedAt returns the most recent update timestamp across all tabs.
// Returns zero if cache is empty.
func (s *Store) LastUpdated() int64 {
	var latest int64
	for _, ct := range s.Tabs {
		if ct.UpdatedAt > latest {
			latest = ct.UpdatedAt
		}
	}
	return latest
}
