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
	Tab         string       `json:"tab"`
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

// DetailReadyMsg carries fetched/assembled detail data into the UI.
type DetailReadyMsg struct {
	Entry DetailEntry
	Err   error
}

// SimilarReadyMsg carries similar title results for the bottom row.
type SimilarReadyMsg struct {
	ForID   string
	Entries []CatalogEntry
	Err     error
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
	// "empty" | "enrich" | "credits" | "artwork" | "related"
	Type string `json:"type"`

	// Enrich fields
	Studio      *string           `json:"studio,omitempty"`
	Networks    []string          `json:"networks,omitempty"`
	ExternalIDs map[string]string `json:"external_ids,omitempty"`

	// Credits fields
	Cast []CastWire `json:"cast,omitempty"`
	Crew []CrewWire `json:"crew,omitempty"`

	// Artwork fields
	Backdrops []ArtworkVariantWire `json:"backdrops,omitempty"`
	Posters   []ArtworkVariantWire `json:"posters,omitempty"`

	// Related fields
	Items []RelatedItemWire `json:"items,omitempty"`
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
