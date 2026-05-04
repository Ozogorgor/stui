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
//! Streaming uses librqbit's built-in stream-aware piece scheduler: when a
//! client reads the per-file HTTP endpoint, librqbit's `TorrentStreams`
//! prioritises the pieces ahead of the read cursor. There is no separate
//! `sequential_download` flag in `AddTorrentOptions` — the prioritisation
//! kicks in automatically once mpv opens the stream URL. Bulk downloads
//! (Task 6) skip the stream URL and use the default rarest-first scheduler.

#![allow(dead_code)] // populated incrementally across the migration

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

mod http_server;
mod session;
mod url;

pub use url::stream_url_for;

/// File extensions we treat as the streaming target. Lowercase, no dot.
const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "webm", "avi", "mov", "ts", "m4v"];

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

    /// Add a torrent and return an HTTP URL pointing at the largest video
    /// file inside it. mpv plays this URL directly with seek support via
    /// HTTP Range requests; librqbit prioritises pieces ahead of the read
    /// cursor automatically once mpv connects.
    pub async fn start_stream(&self, magnet_or_url: &str) -> Result<String> {
        use librqbit::{AddTorrent, AddTorrentOptions};

        // For magnets, `add_torrent` blocks until the metadata handshake
        // completes, so the returned handle always has `metadata` populated.
        let resp = self
            .session
            .add_torrent(
                AddTorrent::from_url(magnet_or_url),
                Some(AddTorrentOptions {
                    paused: false,
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("adding torrent to librqbit session")?;

        let handle = resp
            .into_handle()
            .ok_or_else(|| anyhow!("librqbit returned a list-only response, not a handle"))?;
        let id = handle.id();

        // Snapshot file list out of the metadata lock so we can run the
        // picker on plain values.
        let files: Vec<(usize, PathBuf, u64)> = handle
            .with_metadata(|m| {
                m.file_infos
                    .iter()
                    .enumerate()
                    .map(|(i, fi)| (i, fi.relative_filename.clone(), fi.len))
                    .collect()
            })
            .context("reading torrent metadata for file list")?;

        let file_idx = pick_video_file(&files)
            .ok_or_else(|| anyhow!("no playable video file in torrent"))?;

        Ok(stream_url_for(&self.base_url, id, file_idx))
    }
}

/// Pick the largest file with a known video extension. Returns its index in
/// the torrent's file list, or `None` if no file qualifies (e.g. an
/// audio-only or document-only torrent).
fn pick_video_file(files: &[(usize, PathBuf, u64)]) -> Option<usize> {
    files
        .iter()
        .filter(|(_, p, _)| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| VIDEO_EXTS.contains(&e.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .max_by_key(|(_, _, size)| *size)
        .map(|(idx, _, _)| *idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_largest_video_ignores_samples_and_jpegs() {
        let files = vec![
            (0, "sample.mkv".into(), 50_000_000),
            (1, "movie.mkv".into(), 5_000_000_000),
            (2, "poster.jpg".into(), 100_000),
        ];
        assert_eq!(pick_video_file(&files), Some(1));
    }

    #[test]
    fn no_video_returns_none() {
        let files = vec![(0, "readme.txt".into(), 1024)];
        assert_eq!(pick_video_file(&files), None);
    }

    #[test]
    fn picker_is_case_insensitive_on_extension() {
        let files = vec![(0, "MOVIE.MKV".into(), 1_000)];
        assert_eq!(pick_video_file(&files), Some(0));
    }

    /// Smoke test: hits the public ubuntu .iso magnet, so it touches the
    /// network and DHT. Not run in CI; invoke explicitly with
    /// `cargo test -p stui-runtime -- --ignored live_stream`.
    #[tokio::test]
    #[ignore]
    async fn live_stream_ubuntu_iso_smoke() {
        let magnet = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862";
        let tmp = tempfile::tempdir().unwrap();
        let eng = TorrentEngine::new(tmp.path().to_path_buf()).await.unwrap();
        // .iso isn't in our video-ext list, so we expect this to error with
        // "no playable video file" rather than succeed. The point of the
        // smoke test is that everything up to the picker works against a
        // live magnet — swap the magnet for a video torrent if you want a
        // green path.
        let err = eng.start_stream(magnet).await.unwrap_err().to_string();
        assert!(
            err.contains("no playable video file"),
            "unexpected error: {err}"
        );
    }
}
