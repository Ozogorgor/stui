package ipc

import (
	"fmt"
	"strings"
)

// BubbleTea messages dispatched by the IPC client

// RuntimeReadyMsg is sent once the runtime process has started and
// responded to the initial ping.
type RuntimeReadyMsg struct {
	RuntimeVersion string
	IPCVersion     uint32
}

// IPCVersionMismatchMsg is dispatched when the runtime's ipc_version differs
// from IPCVersion. The TUI continues to run but should display a warning.
type IPCVersionMismatchMsg struct {
	TUIVersion     uint32
	RuntimeVersion uint32
	RuntimeSemver  string
}

// RuntimeErrorMsg wraps a fatal IPC or runtime error.
type RuntimeErrorMsg struct{ Err error }

// SearchResultMsg carries the result of a search request.
// Retained for the person-mode search path in ui.go (dispatchPersonSearch).
// The Rust runtime's Response::SearchResult retirement is deferred to Task 7.0.
type SearchResultMsg struct {
	ReqID  string
	Result SearchResult
	Err    error
}

// PluginListMsg carries the current plugin list.
type PluginListMsg struct {
	Plugins []PluginInfo
	Err     error
}

// PluginLoadedMsg signals a plugin was loaded.
type PluginLoadedMsg struct {
	PluginID string
	Name     string
	Err      error
}

// StatusMsg carries a generic status string for display in the status bar.
type StatusMsg struct{ Text string }

// GridUpdateMsg is pushed by the runtime whenever catalog data changes.
type GridUpdateMsg struct {
	Tab     string         `json:"tab"`
	Entries []CatalogEntry `json:"entries"`
	Source  string         `json:"source"`
}

// CatalogStaleMsg is pushed when the runtime attempted a catalog refresh
// for a tab and got zero entries back (every provider errored, network is
// offline, etc.). The TUI surfaces this in the status bar as
// "⚠ Offline — showing cached <tab>". Entries already displayed remain
// on screen because the runtime won't overwrite them with an empty set.
type CatalogStaleMsg struct {
	Tab    string `json:"tab"`
	Reason string `json:"reason"`
}

// PluginToastMsg is pushed by the runtime when a plugin is hot-loaded or
// fails to load.
type PluginToastMsg struct {
	PluginName string `json:"plugin_name"`
	Version    string `json:"version"`
	PluginType string `json:"plugin_type"`
	Message    string `json:"message"`
	IsError    bool   `json:"is_error"`
}

// SubtitleFetchedMsg is pushed when auto-download succeeds for a played stream.
type SubtitleFetchedMsg struct {
	Language string `json:"language"`
	Provider string `json:"provider"`
	FileName string `json:"file_name"`
}

// SubtitleSearchFailedMsg is pushed when subtitle search/download fails.
type SubtitleSearchFailedMsg struct {
	Reason string `json:"reason"`
}

// ThemeUpdateMsg is pushed by the Rust runtime whenever matugen rewrites
// its colors.json.
type ThemeUpdateMsg struct {
	Colors map[string]string `json:"colors"`
	Mode   string            `json:"mode"`
}

// CatalogEntry is a richer media item with poster data.
type CatalogEntry struct {
	ID          string  `json:"id"`
	Title       string  `json:"title"`
	Year        *string `json:"year"`
	Genre       *string `json:"genre"`
	Rating      *string `json:"rating"`
	Description *string `json:"description"`
	PosterURL   *string `json:"poster_url"`
	PosterArt   *string `json:"poster_art"`
	Provider    string  `json:"provider"`
	Tab         string  `json:"tab"`
	ImdbID      *string `json:"imdb_id"`
	// TMDB id mirrored across the wire. Populated by the runtime
	// either from the plugin's own external_ids or from the
	// anime-bridge enrichment (Fribb maps anilist/kitsu cours to the
	// parent series' TMDB id, which the spine merge keys on). Used
	// by the detail screen to route episode lookups through TMDB
	// even when the catalog entry is anilist-spined — without this,
	// collapsed anime series produce empty episode lists because the
	// anilist plugin doesn't implement the episodes verb.
	TmdbID *string `json:"tmdb_id,omitempty"`
	// MAL id, mirrored same way. Optional; carries through for
	// anime-aware detail flows (e.g. mapping into kitsu/myanimelist
	// links). Nil for non-anime catalog entries.
	MalID *string `json:"mal_id,omitempty"`
	// Artist / creator name. Populated for music tab entries from
	// `PluginEntry.artist_name` on the runtime side. The IPC layer
	// converts runtime CatalogEntry → MediaEntry before sending
	// (see `catalog_entry_to_media_entry` in runtime/src/main.rs),
	// so the JSON key on the wire is `artist_name` (MediaEntry's
	// field). Nil for non-music entries.
	Artist *string `json:"artist_name,omitempty"`
	// ISO 639-1 language code (e.g. "ja", "en"). Populated by plugins
	// that expose it (tmdb, kitsu, anilist). Surfaced in the detail
	// screen as a human-readable language name.
	OriginalLanguage *string `json:"original_language,omitempty"`
	// Fields added for scoped search (Chunk 4).
	Kind   EntryKind `json:"kind,omitempty"`
	Source string    `json:"source,omitempty"`
}

// DetailEntry is the rich metadata for a single title.
type DetailEntry struct {
	ID          string       `json:"id"`
	Title       string       `json:"title"`
	Year        string       `json:"year"`
	Genre       string       `json:"genre"`
	Rating      string       `json:"rating"`
	Runtime     string       `json:"runtime"`
	Description string       `json:"description"`
	PosterURL   string       `json:"poster_url"`
	PosterArt   string       `json:"poster_art"`
	Cast        []CastMember `json:"cast"`
	Provider    string       `json:"provider"`
	Providers   []string     `json:"providers"`
	ImdbID      string       `json:"imdb_id"`
	// Western-anchor TMDB id, populated from CatalogEntry.TmdbID at
	// detail-open time. Used by the EpisodeScreen open path to route
	// season/episode lookups through TMDB even when the catalog
	// entry is anilist/kitsu-spined.
	TmdbID      string       `json:"tmdb_id,omitempty"`
	// ISO 639-1 language code (e.g. "ja", "en"). Mirrored from
	// CatalogEntry at detail-open time. Rendered as a human-readable
	// language name under the synopsis.
	OriginalLanguage string `json:"original_language,omitempty"`
	Tab              string `json:"tab"`

	// Metadata-enrichment fields populated by DetailMetadataPartial "enrich"
	// verbs after the detail panel opens. They default to zero values when
	// the runtime hasn't (yet) delivered the enrichment.
	IDSource    string            `json:"id_source,omitempty"`
	Kind        string            `json:"kind,omitempty"`
	Studio      string            `json:"studio,omitempty"`
	Networks    []string          `json:"networks,omitempty"`
	ExternalIDs map[string]string `json:"external_ids,omitempty"`
	// Series only: total seasons reported by the provider's lookup verb.
	// Zero means "unknown" — EpisodeScreen falls back to a single-season
	// list rather than guessing.
	SeasonCount uint32 `json:"season_count,omitempty"`
	// Per-season provider-native ids (AniList-style: each entry has its
	// own id). Empty for TMDB-style providers where one id covers every
	// season. EpisodeScreen uses this to pick the right id when fetching
	// episodes for season N.
	SeasonIDs []string `json:"season_ids,omitempty"`
}

// CastMember is a single person in the cast/crew list.
type CastMember struct {
	Name     string `json:"name"`
	Role     string `json:"role"`
	RoleType string `json:"role_type"`
}

// PersonSearchMsg is dispatched when the user activates a cast member link.
type PersonSearchMsg struct {
	PersonName string
	FromID     string
}

// StreamInfo describes a single resolved stream candidate.
type StreamInfo struct {
	URL       string  `json:"url"`
	Label     string  `json:"name"`
	Quality   string  `json:"quality"`
	Protocol  string  `json:"protocol"`
	Seeders   int     `json:"seeders"`
	Score     int     `json:"score"`
	Provider  string  `json:"provider"`
	SizeBytes int64   `json:"size_bytes,omitempty"`
	Codec     string  `json:"codec,omitempty"`
	Source    string  `json:"source,omitempty"`
	HDR       bool    `json:"hdr,omitempty"`
	SpeedMbps float64 `json:"speed_mbps,omitempty"`
	LatencyMs int     `json:"latency_ms,omitempty"`
}

// StreamsResolvedMsg is delivered when the runtime has resolved stream candidates.
type StreamsResolvedMsg struct {
	EntryID string
	Streams []StreamInfo
}

// EpisodeStreamsLoadedMsg carries find_streams results for the
// per-episode streams column on the detail card. Indexed by
// (season, episode) so the screen can route each response to the
// right cache slot — multiple in-flight requests overlap when the
// user scrubs through episodes faster than the runtime can reply.
type EpisodeStreamsLoadedMsg struct {
	Season  int
	Episode int
	Streams []StreamInfo
	Err     error
}

// EpisodeStreamsPartialMsg is one provider's contribution to an
// in-flight find_streams. The runtime now streams these as each
// plugin returns rather than waiting for the full fan-out — the TUI
// appends to its per-(season, episode) cache so the user sees fast
// providers' results immediately while slow ones (Jackett's 25 s
// Torznab fan-out) keep arriving.
type EpisodeStreamsPartialMsg struct {
	EntryID  string
	Season   int
	Episode  int
	Provider string
	Streams  []StreamInfo
}

// EpisodeStreamsCompleteMsg signals that the runtime has finished
// fanning out across every provider for this (season, episode). The
// TUI clears its in-flight spinner on receipt. `Err` is set only
// when zero providers returned anything.
type EpisodeStreamsCompleteMsg struct {
	EntryID string
	Season  int
	Episode int
	Err     string
}

// StreamBenchmarkResultMsg is dispatched when a single stream probe finishes.
type StreamBenchmarkResultMsg struct {
	EntryID   string
	URL       string
	SpeedMbps float64
	LatencyMs int
	Err       error
}

// StreamBenchmarkDoneMsg is dispatched after all probes for an entry complete.
type StreamBenchmarkDoneMsg struct {
	EntryID string
}

// StreamPreferences represents user preferences for stream ranking.
type StreamPreferences struct {
	PreferProtocol string   `json:"prefer_protocol,omitempty"`
	MaxResolution  string   `json:"max_resolution,omitempty"`
	MaxSizeMB      int64    `json:"max_size_mb"`
	MinSeeders     int      `json:"min_seeders"`
	AvoidLabels    []string `json:"avoid_labels"`
	PreferHDR      bool     `json:"prefer_hdr"`
	PreferCodecs   []string `json:"prefer_codecs"`
}

// RankedStream represents a stream with its policy-based score and explanations.
type RankedStream struct {
	Stream  StreamInfo `json:"stream"`
	Score   int64      `json:"score"`
	Reasons []string   `json:"reasons"`
}

// StreamsRankedMsg is delivered when runtime has ranked streams with policy scoring.
type StreamsRankedMsg struct {
	Ranked []RankedStream
	Err    error
}

// StreamPolicyLoadedMsg is dispatched when the runtime returns the persisted stream policy.
type StreamPolicyLoadedMsg struct {
	Policy StreamPreferences
	Err    error
}

// EpisodesLoadedMsg carries episode data for a season.
type EpisodesLoadedMsg struct {
	SeriesID string
	Season   int
	Episodes []EpisodeEntry
}

// EpisodesLoadFailedMsg is sent when the runtime returns an error or
// the response can't be decoded. Lets EpisodeScreen exit its loading
// state and surface the failure in-screen rather than silently sitting
// on a spinner forever.
type EpisodesLoadFailedMsg struct {
	SeriesID string
	Season   int
	Reason   string
}

// LastFMAlbumTracksMsg carries track data for an album from LastFM.
type LastFMAlbumTracksMsg struct {
	Album   string
	Artist  string
	Tracks  []AlbumTrack
}

// AlbumTrack represents a single track from an album.
type AlbumTrack struct {
	Number   int    `json:"number"`
	Title    string `json:"title"`
	Artist   string `json:"artist"`
	Duration string `json:"duration"`
}

// MetadataPluginsForKindMsg carries the runtime's snapshot of the
// metadata-source plugins that contribute to a kind's detail-card
// fan-out. Populated by Client.MetadataPluginsForKind in response to a
// per-kind query from the Settings → Metadata Sources screen.
//
// Lists are mutually disjoint after the runtime's dedupe step:
//   - Priority — user-configured, in fan-out order
//   - Discovered — auto-included via plugin manifest tags
//   - Disabled — user opted out, excluded from the fan-out entirely
type MetadataPluginsForKindMsg struct {
	Kind       string
	Priority   []string
	Discovered []string
	Disabled   []string
	Err        error
}

// BingeContextMsg is fired by EpisodeScreen when the user plays an episode with
// binge mode enabled.
type BingeContextMsg struct {
	SeriesTitle  string
	SeriesID     string
	Tab          MediaTab
	Episodes     []EpisodeEntry
	CurrentIdx   int
	BingeEnabled bool
}

// EpisodeEntry is one episode in a series season.
type EpisodeEntry struct {
	Season   int    `json:"season"`
	Episode  int    `json:"episode"`
	Title    string `json:"title"`
	AirDate  string `json:"air_date,omitempty"`
	Runtime  int    `json:"runtime_mins,omitempty"`
	Provider string `json:"provider"`
	EntryID  string `json:"entry_id"`
}

// PlayerStartedMsg is pushed when mpv has launched and is playing.
type PlayerStartedMsg struct {
	Title    string  `json:"title"`
	Path     string  `json:"path"`
	Duration float64 `json:"duration"`
}

// TrackInfo describes a single audio, subtitle, or video track.
type TrackInfo struct {
	ID        int64  `json:"id"`
	TrackType string `json:"track_type"`
	Lang      string `json:"lang"`
	Title     string `json:"title"`
	Selected  bool   `json:"selected"`
	External  bool   `json:"external"`
}

func (t TrackInfo) Label() string {
	if t.Title != "" {
		return t.Title
	}
	if t.Lang != "" {
		return fmt.Sprintf("%s", strings.ToUpper(t.Lang))
	}
	return fmt.Sprintf("Track %d", t.ID)
}

// PlayerTracksUpdatedMsg is pushed once per file load when mpv reports its track list.
type PlayerTracksUpdatedMsg struct {
	Tracks []TrackInfo `json:"tracks"`
}

// PlayerProgressMsg is pushed ~1/s while mpv is playing.
type PlayerProgressMsg struct {
	Position     float64 `json:"position"`
	Duration     float64 `json:"duration"`
	Paused       bool    `json:"paused"`
	CachePercent float64 `json:"cache_percent"`

	Volume          float64 `json:"volume,omitempty"`
	Muted           bool    `json:"muted,omitempty"`
	SubtitleDelay   float64 `json:"subtitle_delay,omitempty"`
	AudioDelay      float64 `json:"audio_delay,omitempty"`
	AudioLabel      string  `json:"audio_label,omitempty"`
	SubLabel        string  `json:"sub_label,omitempty"`
	Quality         string  `json:"quality,omitempty"`
	Protocol        string  `json:"protocol,omitempty"`
	ActiveCandidate int     `json:"active_candidate,omitempty"`
	CandidateCount  int     `json:"candidate_count,omitempty"`
}

// PlayerEndedMsg is pushed when playback finishes or mpv exits.
type PlayerEndedMsg struct {
	Reason string `json:"reason"`
	Error  string `json:"error,omitempty"`
}

// PlayerTerminalTakeoverMsg is pushed before mpv is launched in terminal VO mode.
type PlayerTerminalTakeoverMsg struct {
	VO string `json:"vo"`
}

// PlayerBufferingMsg is pushed while waiting for pre-roll or during a stall-guard pause.
type PlayerBufferingMsg struct {
	Reason      string  `json:"reason"`
	FillPercent float64 `json:"fill_percent"`
	SpeedMbps   float64 `json:"speed_mbps"`
	PreRollSecs float64 `json:"pre_roll_secs"`
	EtaSecs     float64 `json:"eta_secs"`
}

// PlayerBufferReadyMsg is pushed when the pre-roll or stall-guard recovery finishes.
type PlayerBufferReadyMsg struct {
	PreRollSecs float64 `json:"pre_roll_secs"`
	SpeedMbps   float64 `json:"speed_mbps"`
	Slack       float64 `json:"slack"`
}

// CatalogLoadedMsg signals the initial catalog population is complete for a tab.
type CatalogLoadedMsg struct {
	Tab string `json:"tab"`
}

// DownloadEntry tracks the live state of a single aria2 managed download.
type DownloadEntry struct {
	GID      string
	Title    string
	Progress float64
	Speed    string
	ETA      string
	Seeders  uint64
	Status   string
	Files    []string
	Error    string
}

// DownloadStartedMsg is pushed by the runtime when aria2 begins a new download.
type DownloadStartedMsg struct {
	GID   string `json:"gid"`
	Title string `json:"title"`
	URI   string `json:"uri"`
	Dir   string `json:"dir"`
}

// DownloadProgressMsg is pushed ~2/s while a download is in progress.
type DownloadProgressMsg struct {
	GID      string  `json:"gid"`
	Progress float64 `json:"progress"`
	Speed    string  `json:"speed"`
	ETA      string  `json:"eta"`
	Seeders  uint64  `json:"seeders"`
}

// DownloadCompleteMsg is pushed when a download finishes successfully.
type DownloadCompleteMsg struct {
	GID   string   `json:"gid"`
	Files []string `json:"files"`
}

// DownloadErrorMsg is pushed when an aria2 download fails.
type DownloadErrorMsg struct {
	GID     string `json:"gid"`
	Message string `json:"message"`
}

// QueueUpdateMsg is pushed whenever the playback queue length changes.
type QueueUpdateMsg struct {
	QueueLen int `json:"queue_len"`
}

// MpdOutput describes one MPD audio output device.
type MpdOutput struct {
	ID      uint32 `json:"id"`
	Name    string `json:"name"`
	Plugin  string `json:"plugin"`
	Enabled bool   `json:"enabled"`
}

// MpdStatusMsg is pushed by the runtime's MPD idle loop.
type MpdStatusMsg struct {
	State       string  `json:"state"`
	SongTitle   string  `json:"song_title"`
	SongArtist  string  `json:"song_artist"`
	SongAlbum   string  `json:"song_album"`
	Elapsed     float64 `json:"elapsed"`
	Duration    float64 `json:"duration"`
	Volume      uint32  `json:"volume"`
	Bitrate     uint32  `json:"bitrate"`
	AudioFormat string  `json:"audio_format"`
	ReplayGain  string  `json:"replay_gain"`
	Crossfade   uint32  `json:"crossfade"`
	Consume     bool    `json:"consume"`
	Random      bool    `json:"random"`
	Repeat      bool    `json:"repeat"`
	Single      bool    `json:"single"`
	QueueLength uint32  `json:"queue_length"`
	SongPos     int32   `json:"song_pos"`
	SongID      int32   `json:"song_id"`
}

// MpdOutputsResultMsg is dispatched when GetMpdOutputs completes.
type MpdOutputsResultMsg struct {
	Outputs []MpdOutput
	Err     error
}

// SkipSegmentMsg is pushed when the runtime detects an intro or credits segment.
type SkipSegmentMsg struct {
	SegmentType string  `json:"segment_type"`
	Start       float64 `json:"start"`
	End         float64 `json:"end"`
	FromEnd     bool    `json:"from_end"`
}

// ─────────────────────────────────────────────────────────────────────────────
// Detail-metadata enrichment (mirrors runtime/src/ipc/v1/metadata.rs).
//
// Flow:
//   1. TUI sends GetDetailMetadataRequest on detail-panel open.
//   2. Runtime fans out the four verbs (enrich, credits, artwork, related)
//      and streams back one DetailMetadataPartial per verb as its merge
//      finishes. Verbs arrive out-of-order.

// GetDetailMetadataRequest triggers the four-verb fan-out for one entry.
type GetDetailMetadataRequest struct {
	EntryID  string `json:"entry_id"`
	IDSource string `json:"id_source"`
	Kind     string `json:"kind"`
}

// DetailMetadataPartial is one per-verb chunk of merged metadata for a
// pending GetDetailMetadataRequest. Multiple partials arrive per request.
type DetailMetadataPartial struct {
	EntryID string          `json:"entry_id"`
	Verb    string          `json:"verb"`
	Payload MetadataPayload `json:"payload"`
}

// MetadataPayload is a tagged union discriminated by the "type" field.
// Only the fields matching Type are populated; the rest stay at their
// zero values. Keeping this as a flat struct (instead of one struct per
// variant) makes the Bubbletea side's switch-on-verb straightforward.
type MetadataPayload struct {
	// "empty" | "enrich" | "credits" | "artwork" | "related" | "ratings_aggregator"
	Type string `json:"type"`

	// Enrich fields
	Studio      *string           `json:"studio,omitempty"`
	Networks    []string          `json:"networks,omitempty"`
	ExternalIDs map[string]string `json:"external_ids,omitempty"`
	SeasonCount *uint32           `json:"season_count,omitempty"`
	SeasonIDs   []string          `json:"season_ids,omitempty"`

	// Credits fields
	Cast []CastWire `json:"cast,omitempty"`
	Crew []CrewWire `json:"crew,omitempty"`

	// Artwork fields
	Backdrops []ArtworkVariantWire `json:"backdrops,omitempty"`
	Posters   []ArtworkVariantWire `json:"posters,omitempty"`

	// Related fields
	Items []RelatedItemWire `json:"items,omitempty"`

	// RatingsAggregator fields — pre-formatted multi-line description
	// block from the elfhosted rating-aggregator addon. Renders verbatim
	// in the detail screen.
	Description string `json:"description,omitempty"`
	ExternalURL string `json:"external_url,omitempty"`
}

// CastWire mirrors runtime ipc::v1::metadata::CastWire.
type CastWire struct {
	Name         string  `json:"name"`
	Role         string  `json:"role"`
	Character    *string `json:"character,omitempty"`
	BillingOrder *uint32 `json:"billing_order,omitempty"`
}

// CrewWire mirrors runtime ipc::v1::metadata::CrewWire.
type CrewWire struct {
	Name       string  `json:"name"`
	Role       string  `json:"role"`
	Department *string `json:"department,omitempty"`
}

// ArtworkVariantWire mirrors runtime ipc::v1::metadata::ArtworkVariantWire.
type ArtworkVariantWire struct {
	URL       string  `json:"url"`
	Width     *uint32 `json:"width,omitempty"`
	Height    *uint32 `json:"height,omitempty"`
	SizeLabel string  `json:"size_label"`
}

// RelatedItemWire mirrors runtime ipc::v1::metadata::RelatedItemWire.
type RelatedItemWire struct {
	ID        string  `json:"id"`
	IDSource  string  `json:"id_source"`
	Title     string  `json:"title"`
	Year      *uint16 `json:"year,omitempty"`
	PosterURL *string `json:"poster_url,omitempty"`
	Kind      string  `json:"kind"`
}
