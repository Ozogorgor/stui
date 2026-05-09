//! `MediaSource` — coarse content-type classification for the universal pipeline.
//!
//! This enum answers the question "what kind of thing is this?" at the
//! routing level.  It drives which providers are queried, which tabs are
//! shown in the TUI, and which player path is taken.
//!
//! # Design intent
//!
//! By making every piece of content — movies, tracks, radio stations,
//! podcasts, YouTube videos — representable as a `MediaSource`, the pipeline
//! stays completely uniform:
//!
//! ```text
//! MediaItem(source: MediaSource)
//!     ↓
//! providers that support that source
//!     ↓
//! Vec<StreamCandidate>
//!     ↓
//! ranking
//!     ↓
//! mpv (plays anything as a URL)
//! ```
//!
//! No special cases needed per media type.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Coarse content-type classification.
///
/// Used to:
/// - Filter which providers are queried for a given search
/// - Determine the TUI tab a `MediaItem` belongs to
/// - Route to the right player strategy (torrent engine for torrents, mpv for streams)
///
/// New variants can be added here freely; existing providers return
/// `supported_sources()` to declare what they serve.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MediaSource {
    /// A theatrical or home-release film.
    #[default]
    Movie,

    /// A multi-episode TV show or streaming series.
    Series,

    /// A single episode of a series.
    Episode,

    /// A music track, single, or song.
    Track,

    /// A music album or playlist.
    Album,

    /// A live or on-demand internet radio station (Icecast, Shoutcast, etc.).
    Radio,

    /// A podcast episode.
    Podcast,

    /// An online video (YouTube, PeerTube, Odysee, etc.).
    Video,

    /// A locally-indexed media file.
    LocalFile,

    /// Content type is not yet determined or not classifiable.
    Unknown,
}

impl MediaSource {
    /// Short label suitable for display in the TUI tab bar.
    pub fn tab_label(&self) -> &'static str {
        match self {
            MediaSource::Movie => "Movies",
            MediaSource::Series => "Series",
            MediaSource::Episode => "Series", // groups with Series in UI
            MediaSource::Track => "Music",
            MediaSource::Album => "Music",
            MediaSource::Radio => "Radio",
            MediaSource::Podcast => "Podcasts",
            MediaSource::Video => "Videos",
            MediaSource::LocalFile => "Library",
            MediaSource::Unknown => "Other",
        }
    }

    /// Returns true if this source type is audio-only (no video track expected).
    pub fn is_audio_only(&self) -> bool {
        matches!(
            self,
            MediaSource::Track | MediaSource::Album | MediaSource::Radio | MediaSource::Podcast
        )
    }

    /// Returns true if this source typically resolves via torrent or magnet.
    pub fn supports_torrent(&self) -> bool {
        matches!(
            self,
            MediaSource::Movie
                | MediaSource::Series
                | MediaSource::Episode
                | MediaSource::Track
                | MediaSource::Album
        )
    }

    /// Best-guess `MediaSource` from a legacy `MediaTab` string.
    pub fn from_tab_str(tab: &str) -> Self {
        match tab.to_lowercase().as_str() {
            "movies" => MediaSource::Movie,
            "series" => MediaSource::Series,
            "music" => MediaSource::Track,
            "library" => MediaSource::LocalFile,
            "radio" => MediaSource::Radio,
            "podcasts" => MediaSource::Podcast,
            "videos" => MediaSource::Video,
            _ => MediaSource::Unknown,
        }
    }

    /// All sources that map to the "browse" tabs (shown in the main grid).
    pub fn browseable() -> &'static [MediaSource] {
        &[
            MediaSource::Movie,
            MediaSource::Series,
            MediaSource::Track,
            MediaSource::Radio,
            MediaSource::Podcast,
            MediaSource::Video,
        ]
    }
}

impl std::fmt::Display for MediaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.tab_label())
    }
}
