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
	Backend     string `toml:"backend"`      // "off" | "cava" | "chroma"
	Bars        int    `toml:"bars"`         // number of frequency bars
	Height      int    `toml:"height"`       // rows in terminal
	Framerate   int    `toml:"framerate"`    // fps
	Mode        string `toml:"mode"`         // cliamp: "wave"|"scope"|"retro"|"matrix"|"flame"|"pulse"|"binary"|"butterfly"|"terrain"|"sakura"|"firework"|"glitch"|"lightning"|"rain"|"scatter"|"columns"|"bricks" — classic: "bars"|"mirror"|"filled"|"led"
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
	PreferHTTP      bool `toml:"prefer_http"`
	AutoFallback    bool `toml:"auto_fallback"`
	MaxCandidates   int  `toml:"max_candidates"`
	BenchmarkStreams bool `toml:"benchmark_streams"`
	AutoDeleteVideo bool `toml:"auto_delete_video"`
	AutoDeleteAudio bool `toml:"auto_delete_audio"`
}

type DownloadsConfig struct {
	VideoDir string `toml:"video_dir"`
	MusicDir string `toml:"music_dir"`
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
}

type NotificationsConfig struct {
	Enabled    bool   `toml:"enabled"`
	Backend    string `toml:"backend"`
	OnPlayback bool   `toml:"on_playback"`
	OnDownload bool   `toml:"on_download"`
	OnStreams   bool   `toml:"on_streams"`
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
type Config struct {
	Interface     InterfaceConfig     `toml:"interface"`
	Playback      PlaybackConfig      `toml:"playback"`
	Streaming     StreamingConfig     `toml:"streaming"`
	Downloads     DownloadsConfig     `toml:"downloads"`
	Subtitles     SubtitlesConfig     `toml:"subtitles"`
	Providers     ProvidersConfig     `toml:"providers"`
	Notifications NotificationsConfig `toml:"notifications"`
	Skipper       SkipperConfig       `toml:"skipper"`
	Visualizer    VisualizerSettings  `toml:"visualizer"`
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
		},
		Downloads: DownloadsConfig{
			VideoDir: filepath.Join(home, "Videos"),
			MusicDir: filepath.Join(home, "Music"),
		},
		Subtitles: SubtitlesConfig{
			PreferredLanguage: "eng",
		},
		Providers: ProvidersConfig{
			EnableTMDB:      true,
			EnableTorrentio: true,
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
