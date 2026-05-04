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

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

mod http_server;
mod session;
mod url;

pub use url::stream_url_for;

pub struct TorrentEngine {
    pub(crate) session: Arc<librqbit::Session>,
    pub(crate) base_url: String,
}

impl TorrentEngine {
    pub async fn new(staging_dir: PathBuf) -> Result<Self> {
        let s = session::TorrentSession::new(staging_dir).await?;
        let server = http_server::StreamingServer::spawn(s.inner.clone()).await?;
        Ok(Self {
            session: s.inner,
            base_url: format!("http://{}", server.addr),
        })
    }
}
