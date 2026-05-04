//! Embedded torrent engine.
//!
//! Wraps `librqbit::Session` and its built-in HTTP API so the runtime can
//! stream a torrent to mpv without running an external daemon. Replaces the
//! prior aria2 bridge.
//!
//! Public surface:
//! - [`TorrentEngine::new`] — boot session + local HTTP server.
//! - [`TorrentEngine::start_stream`] — add torrent, return HTTP URL mpv plays.
//! - [`TorrentEngine::start_download`] — add torrent without streaming bias.
//!
//! Streaming uses librqbit's sequential download mode; downloads use its
//! default rarest-first.

#![allow(dead_code)] // populated incrementally across the migration

pub mod http_server;
pub mod session;

pub struct TorrentEngine;

impl TorrentEngine {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(TorrentEngine)
    }
}
