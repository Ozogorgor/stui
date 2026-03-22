//! `ProviderCapabilities` — declares what a provider can do.
//!
//! Every provider declares its capabilities at startup.  The engine uses
//! this to skip providers that can't help for a given request — no wasted
//! round trips, no empty result sets.
//!
//! # Routing matrix
//!
//! | Request type          | Required capability     |
//! |-----------------------|-------------------------|
//! | Catalog / trending    | `catalog`               |
//! | Full-text search      | `search`                |
//! | Stream resolution     | `streams`               |
//! | Subtitle fetch        | `subtitles`             |
//! | Metadata enrichment   | `metadata`              |
//!
//! # Media type routing
//!
//! `supported_media` declares which `MediaSource` types the provider handles.
//! A `None` value means "all types" (opt-in to everything).

#![allow(dead_code)]

use crate::media::MediaSource;
use std::fmt;

/// The full capability profile of a provider.
///
/// Cheap to clone — all fields are value types or small `Vec`s.
#[derive(Debug, Clone)]
pub struct ProviderCapabilities {
    /// Provider can return catalog / trending pages.
    pub catalog: bool,

    /// Provider supports free-text search queries.
    pub search: bool,

    /// Provider can resolve media IDs to playable stream URLs.
    pub streams: bool,

    /// Provider can return subtitle tracks.
    pub subtitles: bool,

    /// Provider can enrich `MediaItem` with full metadata (ratings, cast, …).
    pub metadata: bool,

    /// Which `MediaSource` types this provider serves.
    /// `None` = all types supported.
    pub supported_media: Option<Vec<MediaSource>>,
}

impl ProviderCapabilities {
    /// A provider that supplies everything (metadata + streams + subtitles).
    pub fn full() -> Self {
        ProviderCapabilities {
            catalog: true,
            search: true,
            streams: true,
            subtitles: true,
            metadata: true,
            supported_media: None,
        }
    }

    /// A metadata-only provider (no streams, no subtitles).
    pub fn metadata_only() -> Self {
        ProviderCapabilities {
            catalog: true,
            search: true,
            streams: false,
            subtitles: false,
            metadata: true,
            supported_media: None,
        }
    }

    /// A stream-only provider (e.g. Torrentio — no metadata).
    pub fn streams_only() -> Self {
        ProviderCapabilities {
            catalog: false,
            search: false,
            streams: true,
            subtitles: false,
            metadata: false,
            supported_media: None,
        }
    }

    /// A subtitle-only provider (e.g. OpenSubtitles).
    pub fn subtitles_only() -> Self {
        ProviderCapabilities {
            catalog: false,
            search: false,
            streams: false,
            subtitles: true,
            metadata: false,
            supported_media: None,
        }
    }

    /// Builder: restrict to specific media source types.
    pub fn for_media(mut self, types: Vec<MediaSource>) -> Self {
        self.supported_media = Some(types);
        self
    }

    // ── Routing helpers ───────────────────────────────────────────────────

    /// Returns `true` if this provider should be queried for catalog/search
    /// of the given `MediaSource`.
    pub fn handles_catalog(&self, source: &MediaSource) -> bool {
        if !self.catalog && !self.search {
            return false;
        }
        self.handles_source(source)
    }

    /// Returns `true` if this provider should be queried for streams
    /// of the given `MediaSource`.
    pub fn handles_streams(&self, source: &MediaSource) -> bool {
        if !self.streams {
            return false;
        }
        self.handles_source(source)
    }

    /// Returns `true` if this provider should be queried for subtitles.
    pub fn handles_subtitles(&self) -> bool {
        self.subtitles
    }

    /// Returns `true` if this provider handles the given `MediaSource`.
    pub fn handles_source(&self, source: &MediaSource) -> bool {
        match &self.supported_media {
            None => true,
            Some(types) => types.contains(source),
        }
    }
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self::full()
    }
}

impl fmt::Display for ProviderCapabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut caps = vec![];
        if self.catalog {
            caps.push("catalog");
        }
        if self.search {
            caps.push("search");
        }
        if self.streams {
            caps.push("streams");
        }
        if self.subtitles {
            caps.push("subtitles");
        }
        if self.metadata {
            caps.push("metadata");
        }
        write!(f, "[{}]", caps.join(", "))
    }
}
