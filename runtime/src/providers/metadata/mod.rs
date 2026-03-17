//! Metadata providers — fetch catalog content, posters, cast, and ratings.
//!
//! These providers do NOT return playable stream URLs; they return enriched
//! `CatalogEntry` / `MediaItem` data.  Stream resolution is handled by
//! `providers::streams`.

#[cfg(feature = "anime")]
pub mod anilist;
pub mod imdb;
#[cfg(feature = "anime")]
pub mod jikan;
#[cfg(feature = "music")]
pub mod lastfm;
#[cfg(feature = "music")]
pub mod musicbrainz;
pub mod omdb;
pub mod tmdb;

#[cfg(feature = "anime")]
pub use anilist::AniListProvider;
pub use imdb::ImdbProvider;
#[cfg(feature = "anime")]
pub use jikan::JikanProvider;
#[cfg(feature = "music")]
pub use lastfm::LastFmProvider;
#[cfg(feature = "music")]
pub use musicbrainz::MusicBrainzProvider;
pub use omdb::OmdbProvider;
pub use tmdb::TmdbProvider;
