// Package collections provides persistent user-defined media collections
// (e.g. Watchlist, Favorites) backed by a JSON file.
package collections

import (
	"encoding/json"
	"os"
	"path/filepath"
	"time"
)

// Entry is a saved media item inside a collection.
type Entry struct {
	ID       string `json:"id"`
	Title    string `json:"title"`
	Year     string `json:"year,omitempty"`
	Tab      string `json:"tab"`             // "movies", "series", etc.
	Provider string `json:"provider,omitempty"`
	ImdbID   string `json:"imdb_id,omitempty"`
	AddedAt  int64  `json:"added_at"` // unix timestamp
}

// Collection is a named list of entries.
type Collection struct {
	Name    string  `json:"name"`
	Entries []Entry `json:"entries"`
}

// Store holds all user collections and the path to its backing file.
type Store struct {
	Collections []Collection `json:"collections"`
	path        string       `json:"-"`
}

// DefaultPath returns the platform-appropriate path to collections.json.
func DefaultPath() string {
	dir, _ := os.UserConfigDir()
	return filepath.Join(dir, "stui", "collections.json")
}

// Load reads the collections file.
// Returns a new Store with default collections if the file is absent or invalid.
func Load(path string) *Store {
	s := &Store{path: path}
	data, err := os.ReadFile(path)
	if err != nil || json.Unmarshal(data, s) != nil {
		s.Collections = defaultCollections()
		return s
	}
	if len(s.Collections) == 0 {
		s.Collections = defaultCollections()
	}
	return s
}

// Save writes the store to disk atomically via temp-file + rename.
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

// AddTo adds an entry to the named collection.
// Returns false if the collection is not found or the entry is already present.
func (s *Store) AddTo(collName string, e Entry) bool {
	for i := range s.Collections {
		if s.Collections[i].Name != collName {
			continue
		}
		for _, existing := range s.Collections[i].Entries {
			if existing.ID == e.ID {
				return false // duplicate
			}
		}
		e.AddedAt = time.Now().Unix()
		s.Collections[i].Entries = append(s.Collections[i].Entries, e)
		return true
	}
	return false
}

// RemoveFrom removes an entry by ID from the named collection.
// Returns false if not found.
func (s *Store) RemoveFrom(collName, entryID string) bool {
	for i := range s.Collections {
		if s.Collections[i].Name != collName {
			continue
		}
		for j, e := range s.Collections[i].Entries {
			if e.ID == entryID {
				s.Collections[i].Entries = append(
					s.Collections[i].Entries[:j],
					s.Collections[i].Entries[j+1:]...,
				)
				return true
			}
		}
	}
	return false
}

// HasEntry reports whether the named collection contains an entry with the given ID.
func (s *Store) HasEntry(collName, entryID string) bool {
	for _, c := range s.Collections {
		if c.Name != collName {
			continue
		}
		for _, e := range c.Entries {
			if e.ID == entryID {
				return true
			}
		}
	}
	return false
}

// NewCollection creates an empty collection with the given name.
// Returns false if the name already exists.
func (s *Store) NewCollection(name string) bool {
	for _, c := range s.Collections {
		if c.Name == name {
			return false
		}
	}
	s.Collections = append(s.Collections, Collection{Name: name})
	return true
}

// DeleteCollection removes a collection by name.
// Returns false if not found.
func (s *Store) DeleteCollection(name string) bool {
	for i, c := range s.Collections {
		if c.Name == name {
			s.Collections = append(s.Collections[:i], s.Collections[i+1:]...)
			return true
		}
	}
	return false
}

// RenameCollection renames a collection.
// Returns false if the source is not found or the target name already exists.
func (s *Store) RenameCollection(oldName, newName string) bool {
	if oldName == newName {
		return true
	}
	for _, c := range s.Collections {
		if c.Name == newName {
			return false // conflict
		}
	}
	for i := range s.Collections {
		if s.Collections[i].Name == oldName {
			s.Collections[i].Name = newName
			return true
		}
	}
	return false
}

// Names returns the name of every collection in order.
func (s *Store) Names() []string {
	out := make([]string, len(s.Collections))
	for i, c := range s.Collections {
		out[i] = c.Name
	}
	return out
}

func defaultCollections() []Collection {
	return []Collection{
		{Name: "Watchlist"},
		{Name: "Favorites"},
	}
}
