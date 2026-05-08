package config

import (
	"os"
	"os/exec"
	"path/filepath"

	"github.com/BurntSushi/toml"
)

// ── Sub-structs ───────────────────────────────────────────────────────────────

// VisualizerSettings holds the persisted audio-visualizer preferences.
type VisualizerSettings struct {
	Backend     string `toml:"backend"`   // "off" | "cava" | "chroma"
	Bars        int    `toml:"bars"`      // number of frequency bars
	Height      int    `toml:"height"`    // rows in terminal
	Framerate   int    `toml:"framerate"` // fps
	Mode        string `toml:"mode"`      // cliamp: "wave"|"scope"|"retro"|"matrix"|"flame"|"pulse"|"binary"|"butterfly"|"terrain"|"sakura"|"firework"|"glitch"|"lightning"|"rain"|"scatter"|"columns"|"bricks" — classic: "bars"|"mirror"|"filled"|"led"
	Gradient    bool   `toml:"gradient"`
	PeakHold    bool   `toml:"peak_hold"`
	InputMethod string `toml:"input_method"` // "pulse" | "pipewire" | "alsa"
}

type InterfaceConfig struct {
	Theme        string `toml:"theme"`
	ThemeMode    string `toml:"theme_mode"`
	ShowBorders  bool   `toml:"show_borders"`
	MouseSupport bool   `toml:"mouse_support"`
	BiDiMode     string `toml:"bidi_mode"`
}

type PlaybackConfig struct {
	DefaultVolume     int    `toml:"default_volume"`
	Hwdec             string `toml:"hwdec"`
	CacheSecs         int    `toml:"cache_secs"`
	KeepOpen          bool   `toml:"keep_open"`
	AutoplayNext      bool   `toml:"autoplay_next"`
	AutoplayCountdown int    `toml:"autoplay_countdown"`
	MinPrerollSecs    int    `toml:"min_preroll_secs"`
	DemuxerMaxMB      int    `toml:"demuxer_max_mb"`
	TerminalVO        string `toml:"terminal_vo"`
}

type StreamingConfig struct {
	PreferHTTP       bool `toml:"prefer_http"`
	AutoFallback     bool `toml:"auto_fallback"`
	MaxCandidates    int  `toml:"max_candidates"`
	BenchmarkStreams bool `toml:"benchmark_streams"`
	AutoDeleteVideo  bool `toml:"auto_delete_video"`
	AutoDeleteAudio  bool `toml:"auto_delete_audio"`
	// MinSeeders drops torrent streams whose seeder count is at or
	// below this value before they reach the picker. Default 5 —
	// streams without seeder info (HTTP, debrid) always pass through.
	MinSeeders int `toml:"min_seeders"`
	// RequireSeeders is a debug toggle: when true, streams whose
	// seeder count is unknown (None) are also filtered out. Useful
	// for diagnosing why a plugin's results show no `↑N` indicator.
	RequireSeeders bool `toml:"require_seeders"`
	// RequireResolution drops streams whose resolution couldn't be
	// extracted from the release title (Unknown). Off by default.
	RequireResolution bool `toml:"require_resolution"`
	// Allow* are per-tier resolution toggles (Settings → Streaming).
	// All four default to true; a user with limited bandwidth flips
	// the matching tier off to remove those candidates from the
	// picker before ranking. `StreamQuality::Unknown` is governed by
	// `RequireResolution` instead, so it ignores these.
	Allow4K    bool `toml:"allow_4k"`
	Allow1080p bool `toml:"allow_1080p"`
	Allow720p  bool `toml:"allow_720p"`
	AllowSD    bool `toml:"allow_sd"`
}

type DownloadsConfig struct {
	VideoDir string `toml:"video_dir"`
	MusicDir string `toml:"music_dir"`
}

// StorageConfig is the user's organised library roots — distinct from
// DownloadsConfig (which is where new files land before being moved here).
//
// ExtraMusicDirs is a free-form list of additional music roots that the
// MPD bridge should also scan, beyond the primary Music root. Useful when
// a user keeps music split across multiple drives or NAS shares.
type StorageConfig struct {
	Movies         string   `toml:"movies"`
	Series         string   `toml:"series"`
	Anime          string   `toml:"anime"`
	Music          string   `toml:"music"`
	Podcasts       string   `toml:"podcasts"`
	ExtraMusicDirs []string `toml:"extra_music_dirs"`
}

type SubtitlesConfig struct {
	AutoDownload      bool    `toml:"auto_download"`
	PreferredLanguage string  `toml:"preferred_language"`
	DefaultDelay      float64 `toml:"default_delay"`
}

type ProvidersConfig struct {
	EnableTMDB          bool `toml:"enable_tmdb"`
	EnableOMDB          bool `toml:"enable_omdb"`
	EnableTorrentio     bool `toml:"enable_torrentio"`
	EnableProwlarr      bool `toml:"enable_prowlarr"`
	EnableOpenSubtitles bool `toml:"enable_opensubtitles"`
	// RatingSourceWeights maps plugin/source names to weight multipliers
	// for enriching ratings in the UI. Higher weights rank sources
	// more prominently.
	RatingSourceWeights map[string]float64 `toml:"rating_source_weights"`
}

type NotificationsConfig struct {
	Enabled    bool   `toml:"enabled"`
	Backend    string `toml:"backend"`
	OnPlayback bool   `toml:"on_playback"`
	OnDownload bool   `toml:"on_download"`
	OnStreams  bool   `toml:"on_streams"`
}

type SkipperConfig struct {
	Enabled             bool    `toml:"enabled"`
	AutoSkipIntro       bool    `toml:"auto_skip_intro"`
	AutoSkipCredits     bool    `toml:"auto_skip_credits"`
	IntroScanSecs       int     `toml:"intro_scan_secs"`
	MinIntroSecs        int     `toml:"min_intro_secs"`
	MaxIntroSecs        int     `toml:"max_intro_secs"`
	SimilarityThreshold float64 `toml:"similarity_threshold"`
	MinEpisodes         int     `toml:"min_episodes"`
}

// Config is the full set of user preferences.
// Always construct via Default() — never use a zero-value Config directly,
// detectVisualizerBackend returns "cava" or "chroma" if either is installed,
// otherwise falls back to the built-in "cliamp" backend (no external deps).
// Used only for the first-run default.
func detectVisualizerBackend() string {
	if _, err := exec.LookPath("cava"); err == nil {
		return "cava"
	}
	if _, err := exec.LookPath("chroma"); err == nil {
		return "chroma"
	}
	return "cliamp"
}

// Config is the full set of user preferences.
// Always construct via Default() — never use a zero-value Config directly,
// as many defaults are non-zero (e.g. DefaultVolume=100, PreferHTTP=true).
// MPDConfig holds the MPD connection settings (read from stui.toml [mpd]).
type MPDConfig struct {
	Host string `toml:"host"`
	Port int    `toml:"port"`
}

type Config struct {
	Interface     InterfaceConfig     `toml:"interface"`
	Playback      PlaybackConfig      `toml:"playback"`
	Streaming     StreamingConfig     `toml:"streaming"`
	Downloads     DownloadsConfig     `toml:"downloads"`
	Storage       StorageConfig       `toml:"storage"`
	Subtitles     SubtitlesConfig     `toml:"subtitles"`
	Providers     ProvidersConfig     `toml:"providers"`
	Notifications NotificationsConfig `toml:"notifications"`
	Skipper       SkipperConfig       `toml:"skipper"`
	Visualizer    VisualizerSettings  `toml:"visualizer"`
	MPD           MPDConfig           `toml:"mpd"`
}

// ConfigReloadMsg is sent to the bubbletea program when config.toml or the
// active theme file is changed by an external process.
type ConfigReloadMsg struct {
	Config Config
}

// Default returns a Config with all application-default values.
func Default() Config {
	home, _ := os.UserHomeDir()
	if home == "" {
		home = "."
	}
	return Config{
		Interface: InterfaceConfig{
			Theme:       "default",
			ThemeMode:   "dark",
			ShowBorders: true,
			BiDiMode:    "auto",
		},
		Playback: PlaybackConfig{
			DefaultVolume:     100,
			Hwdec:             "auto",
			CacheSecs:         20,
			AutoplayCountdown: 5,
			MinPrerollSecs:    3,
			DemuxerMaxMB:      200,
		},
		Streaming: StreamingConfig{
			PreferHTTP:      true,
			AutoFallback:    true,
			MaxCandidates:   10,
			AutoDeleteVideo: true,
			Allow4K:         true,
			Allow1080p:      true,
			Allow720p:       true,
			AllowSD:         true,
		},
		Downloads: DownloadsConfig{
			VideoDir: filepath.Join(home, "Videos"),
			MusicDir: filepath.Join(home, "Music"),
		},
		Storage: StorageConfig{
			Movies:   filepath.Join(home, "Videos", "Movies"),
			Series:   filepath.Join(home, "Videos", "Series"),
			Anime:    filepath.Join(home, "Videos", "Anime"),
			Music:    filepath.Join(home, "Music"),
			Podcasts: filepath.Join(home, "Music", "Podcasts"),
		},
		Subtitles: SubtitlesConfig{
			PreferredLanguage: "eng",
		},
		Providers: ProvidersConfig{
			EnableTMDB:          true,
			EnableOMDB:          true,
			EnableTorrentio:     true,
			EnableProwlarr:      true,
			EnableOpenSubtitles: true,
			// Default weights for rating sources — can be overridden in config.toml
			RatingSourceWeights: map[string]float64{
				"omdb":        1.0,
				"tmdb":        1.0,
				"musicbrainz": 1.0,
				"lastfm":      1.0,
			},
		},
		Notifications: NotificationsConfig{
			Enabled:    true,
			Backend:    "auto",
			OnPlayback: true,
			OnDownload: true,
		},
		Skipper: SkipperConfig{
			Enabled:             true,
			IntroScanSecs:       300,
			MinIntroSecs:        20,
			MaxIntroSecs:        120,
			SimilarityThreshold: 0.85,
			MinEpisodes:         2,
		},
		Visualizer: VisualizerSettings{
			Backend:     detectVisualizerBackend(),
			Bars:        20,
			Height:      8,
			Framerate:   20,
			Mode:        "wave",
			Gradient:    true,
			PeakHold:    true,
			InputMethod: "pulse",
		},
		MPD: MPDConfig{
			Host: "127.0.0.1",
			Port: 6600,
		},
	}
}

// DefaultPath returns ~/.config/stui/config.toml.
func DefaultPath() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "config.toml")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "stui", "config.toml")
	}
	return ""
}

// Load reads config.toml at path, merging over Default() so missing keys keep
// their default values. Returns Default() (no error) if the file does not exist.
func Load(path string) (Config, error) {
	cfg := Default()
	data, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return cfg, nil
	}
	if err != nil {
		return cfg, err
	}
	if _, err := toml.Decode(string(data), &cfg); err != nil {
		return cfg, err
	}
	return cfg, nil
}

// EnsureExists writes Default() to path if no file exists there yet.
// Idempotent: an existing file is left untouched, so user edits are
// always preserved across launches. The first launch of stui ends
// with a populated config.toml the user can open, read, and edit —
// rather than the previous behaviour of silently using in-memory
// defaults with nothing on disk to discover.
func EnsureExists(path string) error {
	if path == "" {
		return nil
	}
	if _, err := os.Stat(path); err == nil {
		return nil
	}
	return Save(path, Default())
}

// Save writes cfg to path atomically (temp file + rename).
// Creates parent directories as needed.
func Save(path string, cfg Config) error {
	if path == "" {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if err := toml.NewEncoder(f).Encode(cfg); err != nil {
		f.Close()
		os.Remove(tmp)
		return err
	}
	if err := f.Close(); err != nil {
		os.Remove(tmp)
		return err
	}
	return os.Rename(tmp, path)
}

// ApplyChange applies a single settings-screen change to cfg and returns the
// updated Config. key is the settingItem.key from settings.go. Unknown keys
// are silently ignored (actions, read-only items).
func ApplyChange(cfg Config, key string, value interface{}) Config {
	switch key {
	case "interface.theme":
		if v, ok := value.(string); ok {
			cfg.Interface.Theme = v
		}
	case "app.theme_mode":
		if v, ok := value.(string); ok {
			cfg.Interface.ThemeMode = v
		}
	case "ui.show_borders":
		if v, ok := value.(bool); ok {
			cfg.Interface.ShowBorders = v
		}
	case "ui.mouse_support":
		if v, ok := value.(bool); ok {
			cfg.Interface.MouseSupport = v
		}
	case "ui.bidi_mode":
		if v, ok := value.(string); ok {
			cfg.Interface.BiDiMode = v
		}
	case "player.default_volume":
		if v, ok := value.(int); ok {
			cfg.Playback.DefaultVolume = v
		}
	case "player.hwdec":
		if v, ok := value.(string); ok {
			cfg.Playback.Hwdec = v
		}
	case "player.cache_secs":
		if v, ok := value.(int); ok {
			cfg.Playback.CacheSecs = v
		}
	case "player.keep_open":
		if v, ok := value.(bool); ok {
			cfg.Playback.KeepOpen = v
		}
	case "playback.autoplay_next":
		if v, ok := value.(bool); ok {
			cfg.Playback.AutoplayNext = v
		}
	case "playback.autoplay_countdown":
		if v, ok := value.(int); ok {
			cfg.Playback.AutoplayCountdown = v
		}
	case "player.min_preroll_secs":
		if v, ok := value.(int); ok {
			cfg.Playback.MinPrerollSecs = v
		}
	case "player.demuxer_max_mb":
		if v, ok := value.(int); ok {
			cfg.Playback.DemuxerMaxMB = v
		}
	case "player.terminal_vo":
		if v, ok := value.(string); ok {
			cfg.Playback.TerminalVO = v
		}
	case "streaming.prefer_http":
		if v, ok := value.(bool); ok {
			cfg.Streaming.PreferHTTP = v
		}
	case "streaming.auto_fallback":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AutoFallback = v
		}
	case "streaming.max_candidates":
		if v, ok := value.(int); ok {
			cfg.Streaming.MaxCandidates = v
		}
	case "streaming.min_seeders":
		if v, ok := value.(int); ok {
			cfg.Streaming.MinSeeders = v
		}
	case "streaming.require_seeders":
		if v, ok := value.(bool); ok {
			cfg.Streaming.RequireSeeders = v
		}
	case "streaming.require_resolution":
		if v, ok := value.(bool); ok {
			cfg.Streaming.RequireResolution = v
		}
	case "streaming.allow_4k":
		if v, ok := value.(bool); ok {
			cfg.Streaming.Allow4K = v
		}
	case "streaming.allow_1080p":
		if v, ok := value.(bool); ok {
			cfg.Streaming.Allow1080p = v
		}
	case "streaming.allow_720p":
		if v, ok := value.(bool); ok {
			cfg.Streaming.Allow720p = v
		}
	case "streaming.allow_sd":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AllowSD = v
		}
	case "streaming.benchmark_streams":
		if v, ok := value.(bool); ok {
			cfg.Streaming.BenchmarkStreams = v
		}
	case "streaming.auto_delete_video":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AutoDeleteVideo = v
		}
	case "streaming.auto_delete_audio":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AutoDeleteAudio = v
		}
	case "downloads.video_dir":
		if v, ok := value.(string); ok {
			cfg.Downloads.VideoDir = v
		}
	case "downloads.music_dir":
		if v, ok := value.(string); ok {
			cfg.Downloads.MusicDir = v
		}
	case "storage.movies":
		if v, ok := value.(string); ok {
			cfg.Storage.Movies = v
		}
	case "storage.series":
		if v, ok := value.(string); ok {
			cfg.Storage.Series = v
		}
	case "storage.anime":
		if v, ok := value.(string); ok {
			cfg.Storage.Anime = v
		}
	case "storage.music":
		if v, ok := value.(string); ok {
			cfg.Storage.Music = v
		}
	case "storage.podcasts":
		if v, ok := value.(string); ok {
			cfg.Storage.Podcasts = v
		}
	case "storage.extra_music_dirs":
		if v, ok := value.([]string); ok {
			cfg.Storage.ExtraMusicDirs = v
		}
	case "subtitles.auto_download":
		if v, ok := value.(bool); ok {
			cfg.Subtitles.AutoDownload = v
		}
	case "subtitles.preferred_language":
		if v, ok := value.(string); ok {
			cfg.Subtitles.PreferredLanguage = v
		}
	case "subtitles.default_delay":
		if v, ok := value.(float64); ok {
			cfg.Subtitles.DefaultDelay = v
		}
	case "providers.enable_tmdb":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableTMDB = v
		}
	case "providers.enable_omdb":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableOMDB = v
		}
	case "providers.enable_torrentio":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableTorrentio = v
		}
	case "providers.enable_prowlarr":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableProwlarr = v
		}
	case "providers.enable_opensubtitles":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableOpenSubtitles = v
		}
	case "rating_weights":
		// The rating-weights editor (screens/rating_weights.go) ships
		// the full updated map as one payload so the local config and
		// the runtime overlay stay in sync. Accept either the typed
		// `map[string]float64` (direct path) or the looser
		// `map[string]interface{}` shape that may arrive after a
		// JSON round-trip — both flatten to the same field.
		switch v := value.(type) {
		case map[string]float64:
			cfg.Providers.RatingSourceWeights = v
		case map[string]interface{}:
			out := make(map[string]float64, len(v))
			for k, raw := range v {
				switch n := raw.(type) {
				case float64:
					out[k] = n
				case float32:
					out[k] = float64(n)
				case int:
					out[k] = float64(n)
				case int64:
					out[k] = float64(n)
				}
			}
			cfg.Providers.RatingSourceWeights = out
		}
	case "notifications.enabled":
		if v, ok := value.(bool); ok {
			cfg.Notifications.Enabled = v
		}
	case "notifications.backend":
		if v, ok := value.(string); ok {
			cfg.Notifications.Backend = v
		}
	case "notifications.on_playback":
		if v, ok := value.(bool); ok {
			cfg.Notifications.OnPlayback = v
		}
	case "notifications.on_download":
		if v, ok := value.(bool); ok {
			cfg.Notifications.OnDownload = v
		}
	case "notifications.on_streams":
		if v, ok := value.(bool); ok {
			cfg.Notifications.OnStreams = v
		}
	case "skipper.enabled":
		if v, ok := value.(bool); ok {
			cfg.Skipper.Enabled = v
		}
	case "skipper.auto_skip_intro":
		if v, ok := value.(bool); ok {
			cfg.Skipper.AutoSkipIntro = v
		}
	case "skipper.auto_skip_credits":
		if v, ok := value.(bool); ok {
			cfg.Skipper.AutoSkipCredits = v
		}
	case "skipper.intro_scan_secs":
		if v, ok := value.(int); ok {
			cfg.Skipper.IntroScanSecs = v
		}
	case "skipper.min_intro_secs":
		if v, ok := value.(int); ok {
			cfg.Skipper.MinIntroSecs = v
		}
	case "skipper.max_intro_secs":
		if v, ok := value.(int); ok {
			cfg.Skipper.MaxIntroSecs = v
		}
	case "skipper.similarity_threshold":
		if v, ok := value.(float64); ok {
			cfg.Skipper.SimilarityThreshold = v
		}
	case "skipper.min_episodes":
		if v, ok := value.(int); ok {
			cfg.Skipper.MinEpisodes = v
		}
	case "visualizer.backend":
		if v, ok := value.(string); ok {
			cfg.Visualizer.Backend = v
		}
	case "visualizer.bars":
		if v, ok := value.(int); ok {
			cfg.Visualizer.Bars = v
		}
	case "visualizer.height":
		if v, ok := value.(int); ok {
			cfg.Visualizer.Height = v
		}
	case "visualizer.framerate":
		if v, ok := value.(int); ok {
			cfg.Visualizer.Framerate = v
		}
	case "visualizer.mode":
		if v, ok := value.(string); ok {
			cfg.Visualizer.Mode = v
		}
	case "visualizer.peak_hold":
		if v, ok := value.(bool); ok {
			cfg.Visualizer.PeakHold = v
		}
	case "visualizer.gradient":
		if v, ok := value.(bool); ok {
			cfg.Visualizer.Gradient = v
		}
	case "visualizer.input_method":
		if v, ok := value.(string); ok {
			cfg.Visualizer.InputMethod = v
		}
	}
	return cfg
}
