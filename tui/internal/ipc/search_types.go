package ipc

// SearchReq is the payload sent to the runtime for a scoped plugin search.
// It mirrors ipc::v1::SearchRequest on the Rust side.
type SearchReq struct {
	ID      string        `json:"id"`
	Query   string        `json:"query"`
	Scopes  []SearchScope `json:"scopes"`
	Limit   uint32        `json:"limit"`
	Offset  uint32        `json:"offset"`
	QueryID uint64        `json:"query_id"`
}

// ScopeResultsMsg is a server-initiated push carrying search results for one
// scope. It mirrors ipc::v1::ScopeResultsMsg on the Rust side and arrives as
// an Event::ScopeResults frame.
type ScopeResultsMsg struct {
	QueryID uint64       `json:"query_id"`
	Scope   SearchScope  `json:"scope"`
	Entries []MediaEntry `json:"entries"`
	Partial bool         `json:"partial"`
	Error   *ScopeError  `json:"error,omitempty"`
}

// ScopeError is a tagged enum signalling why a scope produced no results.
// Type values: "all_failed" | "no_plugins_configured".
// Additional payload fields may be added in future Rust versions.
type ScopeError struct {
	Type string `json:"type"`
}

// MpdSearchReq is the payload sent to the runtime for an MPD-backed search.
// It mirrors ipc::v1::MpdSearchRequest on the Rust side.
type MpdSearchReq struct {
	ID      string     `json:"id"`
	Query   string     `json:"query"`
	Scopes  []MpdScope `json:"scopes"`
	Limit   uint32     `json:"limit"`
	QueryID uint64     `json:"query_id"`
}

// MpdSearchResult carries results for all requested MPD scopes in a single
// response. It mirrors ipc::v1::MpdSearchResult on the Rust side.
// MpdArtist / MpdAlbum / MpdSong are the same wire shapes used elsewhere in
// the MPD library — no separate *Wire alias is needed.
type MpdSearchResult struct {
	ID      string       `json:"id"`
	QueryID uint64       `json:"query_id"`
	Artists []MpdArtist  `json:"artists"`
	Albums  []MpdAlbum   `json:"albums"`
	Tracks  []MpdSong    `json:"tracks"`
	Error   *MpdSearchErr `json:"error,omitempty"`
}

// MpdSearchErr is a tagged enum signalling why an MPD search failed.
// Type values: "not_connected" | "command_failed".
// Message is populated only for "command_failed".
type MpdSearchErr struct {
	Type    string `json:"type"`
	Message string `json:"message,omitempty"`
}
