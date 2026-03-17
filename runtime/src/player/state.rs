//! `PlaybackState` — the single authoritative model for what mpv is doing.
//!
//! The `PlayerManager` owns one `PlaybackState` and updates it as mpv emits
//! property-change events.  Any part of the runtime that needs to know about
//! playback reads from this state rather than querying mpv directly.
//!
//! The TUI receives a serialised snapshot whenever the state changes via the
//! `player_progress` IPC event.

use serde::{Deserialize, Serialize};

/// A single audio or subtitle track as reported by mpv's `track-list` property.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackInfo {
    /// mpv's internal track ID (used for `sid` / `aid` commands).
    pub id: i64,
    /// `"audio"` | `"sub"` | `"video"`
    pub track_type: String,
    /// BCP-47 language tag, e.g. `"en"`, `"ja"` — may be empty.
    pub lang: String,
    /// Human-readable title from the container, or empty.
    pub title: String,
    /// Whether this is the currently active track.
    pub selected: bool,
    /// Whether this is an external track (loaded via `--sub-file`).
    pub external: bool,
}

impl TrackInfo {
    /// Display label for the TUI: prefers title, falls back to lang, then "Track N".
    pub fn label(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        if !self.lang.is_empty() {
            return self.lang.to_uppercase();
        }
        format!("Track {}", self.id)
    }
}

/// Authoritative playback state — updated from mpv property-change events.
///
/// All fields have sensible defaults so the struct is valid before mpv connects.
/// The TUI renders directly from this; no additional mpv queries needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackState {
    // ── Transport ──────────────────────────────────────────────────────────

    /// True while mpv has a file loaded and is playing (or paused).
    pub active: bool,

    /// True while playback is paused.
    pub paused: bool,

    /// Current playback position in seconds.
    pub position: f64,

    /// Total duration in seconds (0 if not yet known).
    pub duration: f64,

    // ── Audio / subtitle tracks ────────────────────────────────────────────

    /// Current audio track ID (`aid`). `None` = no audio / not yet known.
    pub audio_track: Option<i64>,

    /// Current subtitle track ID (`sid`). `None` = subtitles off.
    pub subtitle_track: Option<i64>,

    /// All tracks reported by mpv's `track-list` property.
    #[serde(default)]
    pub tracks: Vec<TrackInfo>,

    // ── Sync adjustments ──────────────────────────────────────────────────

    /// Current `sub-delay` in seconds (positive = subtitles appear later).
    pub subtitle_delay: f64,

    /// Current `audio-delay` in seconds (positive = audio plays later).
    pub audio_delay: f64,

    // ── Volume ────────────────────────────────────────────────────────────

    /// Current volume (0–130, mpv's scale; 100 = 100%).
    pub volume: f64,

    /// True if audio is muted.
    pub muted: bool,

    // ── Buffering / cache ─────────────────────────────────────────────────

    /// Network cache fill percentage (0–100). 100 for local files.
    pub cache_percent: f64,

    /// True during the initial buffer fill (shows a buffering indicator).
    pub buffering: bool,

    // ── Media identity ────────────────────────────────────────────────────

    /// Display title from mpv's `media-title` property.
    pub title: String,

    /// The URL or path currently loaded in mpv.
    pub url: String,

    /// Quality label of the active stream candidate, e.g. `"1080p"`.
    pub quality: Option<String>,

    /// Stream protocol of the active candidate: `"HTTP"`, `"Torrent"`, etc.
    pub protocol: Option<String>,

    // ── Stream candidates ─────────────────────────────────────────────────

    /// Index of the active candidate in `candidates`.
    pub active_candidate: usize,

    /// Total number of available stream candidates (for TUI "stream N/M" label).
    pub candidate_count: usize,
}

impl Default for PlaybackState {
    fn default() -> Self {
        PlaybackState {
            active:           false,
            paused:           false,
            position:         0.0,
            duration:         0.0,
            audio_track:      None,
            subtitle_track:   None,
            tracks:           vec![],
            subtitle_delay:   0.0,
            audio_delay:      0.0,
            volume:           100.0,
            muted:            false,
            cache_percent:    100.0,
            buffering:        false,
            title:            String::new(),
            url:              String::new(),
            quality:          None,
            protocol:         None,
            active_candidate: 0,
            candidate_count:  0,
        }
    }
}

impl PlaybackState {
    /// Format `position` as `HH:MM:SS` or `MM:SS`.
    pub fn position_str(&self) -> String {
        format_duration(self.position)
    }

    /// Format `duration` as `HH:MM:SS` or `MM:SS`.
    pub fn duration_str(&self) -> String {
        format_duration(self.duration)
    }

    /// Compact progress string suitable for the status bar: `"01:22:43 / 02:10:05"`.
    pub fn progress_str(&self) -> String {
        if self.duration > 0.0 {
            format!("{} / {}", self.position_str(), self.duration_str())
        } else {
            self.position_str()
        }
    }

    /// Progress as a fraction 0.0–1.0 (for drawing a progress bar).
    pub fn progress_fraction(&self) -> f64 {
        if self.duration > 0.0 {
            (self.position / self.duration).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Label for the active audio track, e.g. `"English"` or `"Track 2"`.
    pub fn audio_label(&self) -> String {
        if let Some(aid) = self.audio_track {
            if let Some(t) = self.tracks.iter().find(|t| t.track_type == "audio" && t.id == aid) {
                return t.label();
            }
        }
        "—".to_string()
    }

    /// Label for the active subtitle track, e.g. `"English"` or `"Off"`.
    pub fn sub_label(&self) -> String {
        match self.subtitle_track {
            None => "Off".to_string(),
            Some(sid) => {
                if let Some(t) = self.tracks.iter().find(|t| t.track_type == "sub" && t.id == sid) {
                    return t.label();
                }
                format!("Track {sid}")
            }
        }
    }

    /// All audio tracks.
    pub fn audio_tracks(&self) -> Vec<&TrackInfo> {
        self.tracks.iter().filter(|t| t.track_type == "audio").collect()
    }

    /// All subtitle tracks (including external ones).
    pub fn subtitle_tracks(&self) -> Vec<&TrackInfo> {
        self.tracks.iter().filter(|t| t.track_type == "sub").collect()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn format_duration(secs: f64) -> String {
    if secs < 0.0 {
        return "0:00".to_string();
    }
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_works() {
        assert_eq!(format_duration(0.0),    "0:00");
        assert_eq!(format_duration(90.0),   "1:30");
        assert_eq!(format_duration(3661.0), "1:01:01");
    }

    #[test]
    fn progress_fraction_clamps() {
        let mut s = PlaybackState::default();
        s.duration = 100.0;
        s.position = 50.0;
        assert!((s.progress_fraction() - 0.5).abs() < 1e-6);
        s.position = 150.0;
        assert_eq!(s.progress_fraction(), 1.0);
    }

    #[test]
    fn sub_label_off_when_none() {
        let s = PlaybackState::default();
        assert_eq!(s.sub_label(), "Off");
    }
}
