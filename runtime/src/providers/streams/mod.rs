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
//! | `torrent`  | Generic torrent/magnet bridge (plugs into the librqbit engine) |
//! | `direct`   | Direct HTTP video URLs (CDN-hosted or yt-dlp-resolved) |
//! | `vod`      | VOD/streaming service bridges (future: Jellyfin, Plex) |
//!
//! Community stream providers live in the WASM plugin system:
//! `~/.stui/plugins/prowlarr-provider.wasm`, etc.

pub mod direct;
pub mod vod;

#[allow(unused_imports)]
pub use direct::DirectProvider;
#[allow(unused_imports)]
pub use vod::VodProvider;
