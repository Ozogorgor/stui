package watchhistory

import (
	"sort"
	"sync"
	"time"

	"github.com/stui/stui/internal/ipc"
)

type IPCStore struct {
	mu      sync.RWMutex
	entries map[string]Entry
	client  *ipc.Client
}

func NewIPCStore(client *ipc.Client) *IPCStore {
	return &IPCStore{
		entries: make(map[string]Entry),
		client:  client,
	}
}

func (s *IPCStore) Load() {
	ch := s.client.GetWatchHistoryInProgress("")
	select {
	case entries, ok := <-ch:
		if !ok {
			return
		}
		s.mu.Lock()
		defer s.mu.Unlock()
		for _, e := range entries {
			year := ""
			if e.Year != nil {
				year = *e.Year
			}
			imdbID := ""
			if e.ImdbID != nil {
				imdbID = *e.ImdbID
			}
			s.entries[e.ID] = Entry{
				ID:          e.ID,
				Title:       e.Title,
				Year:        year,
				Tab:         e.Tab,
				Provider:    e.Provider,
				ImdbID:      imdbID,
				Position:    e.Position,
				Duration:    e.Duration,
				Completed:   e.Completed,
				LastWatched: e.LastWatched,
				Season:      int(e.Season),
				Episode:     int(e.Episode),
			}
		}
	}
}

func (s *IPCStore) Save() error {
	return nil
}

func (s *IPCStore) Upsert(e Entry) {
	e.LastWatched = time.Now().Unix()
	s.mu.Lock()
	s.entries[e.ID] = e
	s.mu.Unlock()

	year := e.Year
	imdbID := e.ImdbID
	s.client.UpsertWatchHistoryEntry(ipc.WatchHistoryEntry{
		ID:          e.ID,
		Title:       e.Title,
		Year:        &year,
		Tab:         e.Tab,
		Provider:    e.Provider,
		ImdbID:      &imdbID,
		Position:    e.Position,
		Duration:    e.Duration,
		Completed:   e.Completed,
		LastWatched: e.LastWatched,
		Season:      uint(e.Season),
		Episode:     uint(e.Episode),
	})
}

func (s *IPCStore) Get(id string) *Entry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if e, ok := s.entries[id]; ok {
		return &e
	}
	return nil
}

func (s *IPCStore) Remove(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.entries[id]; ok {
		delete(s.entries, id)
		s.client.RemoveWatchHistoryEntry(id)
		return true
	}
	return false
}

func (s *IPCStore) MarkCompleted(id string) {
	s.client.MarkWatchHistoryCompleted(id)
	s.mu.Lock()
	defer s.mu.Unlock()
	if e, ok := s.entries[id]; ok {
		e.Completed = true
		e.LastWatched = time.Now().Unix()
		s.entries[id] = e
	}
}

func (s *IPCStore) InProgress() []Entry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []Entry
	for _, e := range s.entries {
		if e.Position > 0 && !e.Completed {
			out = append(out, e)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		return out[i].LastWatched > out[j].LastWatched
	})
	return out
}

func (s *IPCStore) UpdatePosition(id string, position, duration float64) bool {
	s.client.UpdateWatchHistoryPosition(id, position, duration)
	s.mu.Lock()
	defer s.mu.Unlock()
	if e, ok := s.entries[id]; ok {
		e.Position = position
		if duration > 0 {
			e.Duration = duration
		}
		e.LastWatched = time.Now().Unix()
		s.entries[id] = e
		return true
	}
	return false
}
