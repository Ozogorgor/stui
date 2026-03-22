package mediacache

import (
	"sync"
	"time"

	"github.com/stui/stui/internal/ipc"
)

type IPCClient interface {
	GetMediaCacheTab(tab string) <-chan ipc.CachedTab
	GetMediaCacheAll() <-chan []ipc.CatalogEntry
	GetMediaCacheStats() <-chan ipc.MediaCacheStats
	ClearMediaCache()
}

type IPCStore struct {
	mu      sync.RWMutex
	tabs    map[string][]ipc.CatalogEntry
	updated map[string]int64
	client  IPCClient
}

func NewIPCStore(client IPCClient) *IPCStore {
	return &IPCStore{
		tabs:    make(map[string][]ipc.CatalogEntry),
		updated: make(map[string]int64),
		client:  client,
	}
}

func (s *IPCStore) LoadTab(tab string) {
	if s.client == nil {
		return
	}
	go func() {
		ch := s.client.GetMediaCacheTab(tab)
		cached := <-ch
		if cached.Tab != "" {
			s.mu.Lock()
			s.tabs[cached.Tab] = cached.Entries
			s.updated[cached.Tab] = cached.UpdatedAt
			s.mu.Unlock()
		}
	}()
}

func (s *IPCStore) LoadAll() {
	if s.client == nil {
		return
	}
	go func() {
		ch := s.client.GetMediaCacheAll()
		entries := <-ch
		s.mu.Lock()
		s.tabs["_all"] = entries
		s.mu.Unlock()
	}()
}

func (s *IPCStore) LoadStats() ipc.MediaCacheStats {
	if s.client == nil {
		return ipc.MediaCacheStats{}
	}
	ch := s.client.GetMediaCacheStats()
	stats := <-ch
	return stats
}

func (s *IPCStore) Clear() error {
	if s.client != nil {
		s.client.ClearMediaCache()
	}
	s.mu.Lock()
	s.tabs = make(map[string][]ipc.CatalogEntry)
	s.updated = make(map[string]int64)
	s.mu.Unlock()
	return nil
}

func (s *IPCStore) SaveTab(tab string, entries []ipc.CatalogEntry) {
	s.mu.Lock()
	s.tabs[tab] = entries
	s.updated[tab] = time.Now().Unix()
	s.mu.Unlock()
}

func (s *IPCStore) EntriesForTab(tab string) []ipc.CatalogEntry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if entries, ok := s.tabs[tab]; ok {
		return entries
	}
	return nil
}

func (s *IPCStore) AllEntries() []ipc.CatalogEntry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if entries, ok := s.tabs["_all"]; ok {
		return entries
	}
	var all []ipc.CatalogEntry
	for tab, entries := range s.tabs {
		if tab != "_all" {
			all = append(all, entries...)
		}
	}
	return all
}

func (s *IPCStore) TotalCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	count := 0
	for tab, entries := range s.tabs {
		if tab != "_all" {
			count += len(entries)
		}
	}
	return count
}

func (s *IPCStore) LastUpdated() int64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var latest int64
	for _, t := range s.updated {
		if t > latest {
			latest = t
		}
	}
	return latest
}

func (s *IPCStore) TabLastUpdated(tab string) int64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.updated[tab]
}
