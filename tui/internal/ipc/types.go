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

// SearchOptions holds optional sort and filter parameters for a search request.
// All fields are zero-value safe: empty string / zero / nil means "no preference".
type SearchOptions struct {
	// Sort order: "rating" | "newest" | "oldest" | "alphabetical" | "relevance".
	// Empty string defaults to "rating".
	Sort string `json:"sort,omitempty"`
	// Genre substring filter (case-insensitive). Empty = no filter.
	Genre string `json:"genre,omitempty"`
	// Minimum composite rating 0.0–10.0. Zero = no minimum.
	MinRating float64 `json:"min_rating,omitempty"`
	// Year range. Both must be non-zero for the filter to apply.
	YearFrom int `json:"year_from,omitempty"`
	YearTo   int `json:"year_to,omitempty"`
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
}

type PluginInfo struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Version     string   `json:"version"`
	PluginType  string   `json:"plugin_type"`
	Status      string   `json:"status"`
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
