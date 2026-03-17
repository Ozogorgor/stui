//! Stream providers — resolve media IDs into playable stream URLs.
//!
//! Stream providers implement the `Provider::streams()` method and return
//! `Vec<Stream>` for a given entry ID.  The streams are then ranked by the
//! `quality` module and presented to the user.
//!
//! # Built-in stream providers
//!
//! | Module     | Description |
//! |------------|-------------|
//! | `torrent`  | Generic torrent/magnet bridge (plugs into aria2) |
//! | `direct`   | Direct HTTP video URLs (CDN-hosted or yt-dlp-resolved) |
//! | `vod`      | VOD/streaming service bridges (future: Jellyfin, Plex) |
//!
//! Community stream providers live in the WASM plugin system:
//! `~/.stui/plugins/prowlarr-provider.wasm`, etc.

#[cfg(feature = "torrent")]
pub mod torrent;
pub mod direct;
pub mod vod;

#[cfg(feature = "torrent")]
pub use torrent::TorrentProvider;
pub use direct::DirectProvider;
pub use vod::VodProvider;
