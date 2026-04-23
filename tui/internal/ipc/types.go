package ipc

import "encoding/json"

// IPCVersion is the TUI's protocol version number. Must be bumped together
// with CURRENT_VERSION in runtime/src/ipc/mod.rs when introducing breaking
// changes to the wire protocol.
const IPCVersion = 1

type MediaTab string

const (
	TabMovies  MediaTab = "movies"
	TabSeries  MediaTab = "series"
	TabMusic   MediaTab = "music"
	TabLibrary MediaTab = "library"
)

// SearchScope is the set of entity kinds a search request may target.
type SearchScope string

const (
	ScopeArtist  SearchScope = "artist"
	ScopeAlbum   SearchScope = "album"
	ScopeTrack   SearchScope = "track"
	ScopeMovie   SearchScope = "movie"
	ScopeSeries  SearchScope = "series"
	ScopeEpisode SearchScope = "episode"
)

// EntryKind labels what kind of media a MediaEntry represents.
type EntryKind string

const (
	KindArtist  EntryKind = "artist"
	KindAlbum   EntryKind = "album"
	KindTrack   EntryKind = "track"
	KindMovie   EntryKind = "movie"
	KindSeries  EntryKind = "series"
	KindEpisode EntryKind = "episode"
)

// MpdScope is the subset of SearchScope values valid for MPD searches.
type MpdScope string

const (
	MpdScopeArtist MpdScope = "artist"
	MpdScopeAlbum  MpdScope = "album"
	MpdScopeTrack  MpdScope = "track"
)

type requestEnvelope struct {
	Type string         `json:"type"`
	Data map[string]any `json:"-"`
}

type RawResponse struct {
	Type string          `json:"type"`
	Raw  json.RawMessage `json:"-"`
	Err  error           `json:"-"`
}

func (r RawResponse) IsError() bool {
	return r.Type == "error" || r.Err != nil
}

func (r RawResponse) decodeData(v any) error {
	if r.Err != nil {
		return r.Err
	}
	return json.Unmarshal(r.Raw, v)
}

type SearchResult struct {
	ID     string       `json:"id"`
	Items  []MediaEntry `json:"items"`
	Total  int          `json:"total"`
	Offset int          `json:"offset"`
}


type MediaEntry struct {
	ID          string   `json:"id"`
	Title       string   `json:"title"`
	Year        *string  `json:"year"`
	Genre       *string  `json:"genre"`
	Rating      *string  `json:"rating"`
	Description *string  `json:"description"`
	PosterURL   *string  `json:"poster_url"`
	Provider    string   `json:"provider"`
	Tab         MediaTab `json:"tab"`
	// Fields added for scoped search (Chunk 4).
	Kind        EntryKind `json:"kind,omitempty"`
	Source      string    `json:"source,omitempty"`
	ArtistName  string    `json:"artist_name,omitempty"`
	AlbumName   string    `json:"album_name,omitempty"`
	TrackNumber uint32    `json:"track_number,omitempty"`
	Season      uint32    `json:"season,omitempty"`
	Episode     uint32    `json:"episode,omitempty"`
}

type PluginInfo struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Version     string   `json:"version"`
	PluginType  string   `json:"plugin_type"`
	Status      string   `json:"status"`
	Enabled     bool     `json:"enabled"`
	Tags        []string `json:"tags"`
	Description string   `json:"description"`
	Author      string   `json:"author"`
}

type PluginListResult struct {
	Plugins []PluginInfo `json:"plugins"`
}

type ErrorPayload struct {
	ID      *string `json:"id"`
	Code    string  `json:"code"`
	Message string  `json:"message"`
}
