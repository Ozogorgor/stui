//! Stremio addon protocol bridge.
//!
//! Stremio addons expose a simple HTTP JSON API that returns catalog and stream
//! data.  By implementing this bridge, stui gains instant access to the entire
//! Stremio addon ecosystem:
//!
//! - **Torrentio** — the most popular torrent aggregator
//! - **OpenSubtitles** (official addon)
//! - **Anime Kitsu** — anime catalog + streams
//! - **RPDB** — ratings and posters
//! - Hundreds of community addons
//!
//! # Addon URL format
//!
//! Stremio addons are identified by a manifest URL:
//!   `https://torrentio.strem.fun/manifest.json`
//!   `https://v3-cinemeta.strem.io/manifest.json`
//!
//! The bridge fetches the manifest, then calls resource endpoints:
//!   `{base_url}/catalog/{type}/{id}.json`
//!   `{base_url}/stream/{type}/{id}.json`
//!   `{base_url}/subtitles/{type}/{id}.json`
//!
//! # Integration
//!
//! Stremio addons are registered alongside WASM plugins.  The engine calls
//! `StremioAddon::as_provider()` to get a `Box<dyn Provider>`.

pub mod manifest;
pub mod client;
pub mod adapter;

pub use manifest::StremioManifest;
pub use client::StremioClient;
pub use adapter::StremioAddon;
