// Package watchhistory tracks per-item playback positions so stui can offer
// "resume from where you left off" on movies and series.
package watchhistory

import (
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"time"
)

// completedThreshold is the fraction of a video that must be played for it to
// be considered "completed" and removed from the Continue Watching list.
const completedThreshold = 0.90

// Entry records the last-known playback position for one media item.
type Entry struct {
	ID          string  `json:"id"`
	Title       string  `json:"title"`
	Year        string  `json:"year,omitempty"`
	Tab         string  `json:"tab"` // "movies" | "series"
	Provider    string  `json:"provider"`
	ImdbID      string  `json:"imdb_id,omitempty"`
	Position    float64 `json:"position"`          // seconds
	Duration    float64 `json:"duration"`          // total seconds; 0 if unknown
	Completed   bool    `json:"completed"`         // true once >90% watched
	LastWatched int64   `json:"last_watched"`      // unix timestamp
	Season      int     `json:"season,omitempty"`  // 0 = unknown
	Episode     int     `json:"episode,omitempty"` // 0 = unknown
}

// Progress returns Position/Duration as a 0–1 fraction.
// Returns 0 if Duration is unknown.
func (e Entry) Progress() float64 {
	if e.Duration <= 0 {
		return 0
	}
	f := e.Position / e.Duration
	if f > 1 {
		f = 1
	}
	return f
}

// StoreInterface defines the interface for watch history stores.
type StoreInterface interface {
	Save() error
	Upsert(e Entry)
	Get(id string) *Entry
	Remove(id string) bool
	MarkCompleted(id string)
	InProgress() []Entry
	UpdatePosition(id string, position, duration float64) bool
}

// Store holds all watch history entries and the path to its backing file.
type Store struct {
	Entries []Entry `json:"entries"`
	path    string  `json:"-"`
}

// DefaultPath returns the platform-appropriate path to history.json.
func DefaultPath() string {
	dir, _ := os.UserConfigDir()
	return filepath.Join(dir, "stui", "history.json")
}

// Load reads the history file.
// Returns an empty store if the file is absent or invalid.
func Load(path string) *Store {
	s := &Store{path: path}
	data, err := os.ReadFile(path)
	if err != nil {
		return s
	}
	_ = json.Unmarshal(data, s)
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

// Upsert inserts or updates an entry by ID.
// LastWatched is always set to the current time.
func (s *Store) Upsert(e Entry) {
	e.LastWatched = time.Now().Unix()
	for i := range s.Entries {
		if s.Entries[i].ID == e.ID {
			s.Entries[i] = e
			return
		}
	}
	s.Entries = append(s.Entries, e)
}

// Get returns a pointer to the entry for the given ID, or nil if not found.
func (s *Store) Get(id string) *Entry {
	for i := range s.Entries {
		if s.Entries[i].ID == id {
			return &s.Entries[i]
		}
	}
	return nil
}

// Remove deletes the entry with the given ID.
// Returns false if not found.
func (s *Store) Remove(id string) bool {
	for i, e := range s.Entries {
		if e.ID == id {
			s.Entries = append(s.Entries[:i], s.Entries[i+1:]...)
			return true
		}
	}
	return false
}

// MarkCompleted marks the entry as completed (watched to the end).
func (s *Store) MarkCompleted(id string) {
	for i := range s.Entries {
		if s.Entries[i].ID == id {
			s.Entries[i].Completed = true
			s.Entries[i].LastWatched = time.Now().Unix()
			return
		}
	}
}

// InProgress returns all entries that have been started but not yet completed,
// sorted by LastWatched descending (most recently watched first).
func (s *Store) InProgress() []Entry {
	var out []Entry
	for _, e := range s.Entries {
		if e.Position > 0 && !e.Completed {
			out = append(out, e)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		return out[i].LastWatched > out[j].LastWatched
	})
	return out
}

// episodeRe matches patterns like S03E05, s2e1, S1E10.
var episodeRe = regexp.MustCompile(`(?i)s(\d+)e(\d+)`)

// ParseEpisodeInfo extracts season and episode numbers from a title string.
// Returns (0, 0) if no SnnEnn pattern is found.
func ParseEpisodeInfo(title string) (season, episode int) {
	m := episodeRe.FindStringSubmatch(title)
	if m == nil {
		return 0, 0
	}
	season, _ = strconv.Atoi(m[1])
	episode, _ = strconv.Atoi(m[2])
	return season, episode
}

// UpdatePosition updates position + duration for an existing entry.
// If the entry doesn't exist yet, it is a no-op (call Upsert first).
// Returns true if the entry was found and a save is recommended.
func (s *Store) UpdatePosition(id string, position, duration float64) bool {
	for i := range s.Entries {
		if s.Entries[i].ID != id {
			continue
		}
		s.Entries[i].Position = position
		if duration > 0 {
			s.Entries[i].Duration = duration
		}
		// Auto-complete if past threshold
		if duration > 0 && position/duration >= completedThreshold {
			s.Entries[i].Completed = true
		}
		s.Entries[i].LastWatched = time.Now().Unix()
		return true
	}
	return false
}
