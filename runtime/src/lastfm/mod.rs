//! lastfm — direct fetchers for last.fm endpoints the runtime needs
//! outside the WASM plugin sandbox.
//!
//! Today this is just `album_tracks`, used by the AlbumDetail screen.
//! The lastfm WASM plugin already calls `album.getInfo` for enrich
//! (genre / wiki / synthetic rating), but PluginEntry has no
//! tracks-on-album field so the plugin can't surface the tracks
//! list. Rather than carve out a new SDK shape just for this one
//! flow, the runtime hits the same endpoint directly with reqwest
//! and forwards results over IPC. If we later add a typed
//! tracks-on-album verb to the plugin SDK, this module can shrink
//! to a thin caller of `engine.supervisor_<verb>`.

pub mod album_tracks;
