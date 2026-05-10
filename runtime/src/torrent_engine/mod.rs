//! Embedded torrent engine.
//!
//! Wraps `librqbit::Session` and its built-in HTTP API so the runtime can
//! stream a torrent to mpv without running an external daemon. Replaces the
//! prior aria2 bridge.
//!
//! Public surface:
//! - [`TorrentEngine::new`] — boot session + local HTTP server.
//! - [`TorrentEngine::start_stream`] — add torrent, return HTTP URL mpv plays.
//! - [`TorrentEngine::start_download`] — add torrent + return completion handle.
//!
//! Streaming uses librqbit's built-in stream-aware piece scheduler: when a
//! client reads the per-file HTTP endpoint, librqbit's `TorrentStreams`
//! prioritises the pieces ahead of the read cursor. There is no separate
//! `sequential_download` flag in `AddTorrentOptions` — the prioritisation
//! kicks in automatically once mpv opens the stream URL. Bulk downloads
//! (Task 6) skip the stream URL and use the default rarest-first scheduler.

#![allow(dead_code)] // populated incrementally across the migration

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

/// How long to wait for librqbit to fetch metadata from peers before giving
/// up. Dead magnets would hang forever otherwise; 60 s is generous enough
/// for healthy torrents on a slow link.
const METADATA_TIMEOUT: Duration = Duration::from_secs(60);

mod http_server;
mod session;
mod url;

pub use url::stream_url_for;

/// File extensions we treat as the streaming target. Lowercase, no dot.
const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "webm", "avi", "mov", "ts", "m4v"];

/// Audio extensions for per-track album streaming. Lowercase, no dot. Order is
/// not significant — preference (e.g. lossless first) is applied separately
/// in [`pick_album_tracks`].
const AUDIO_EXTS: &[&str] = &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav"];

/// One playable track in a music album torrent. The runtime hands these to
/// `MpdBridge::queue_and_play_many` so mpd opens the librqbit-served HTTP
/// URLs in order.
#[derive(Debug, Clone)]
pub struct TrackStream {
    pub url: String,
    pub filename: String,
    pub size_bytes: u64,
}

/// Result of [`TorrentEngine::start_album_stream`]. Holds the torrent id so
/// later phases (prefetch warmer, cancellation) can reference it.
#[derive(Debug, Clone)]
pub struct AlbumStream {
    pub torrent_id: usize,
    pub tracks: Vec<TrackStream>,
}

pub struct TorrentEngine {
    pub(crate) session: Arc<librqbit::Session>,
    pub(crate) base_url: String,
    pub(crate) staging_dir: PathBuf,
}

/// Handle returned by [`TorrentEngine::start_download`]. The consumer (the
/// download translator that runs after `d` keybind) reads `final_path`
/// relative to the engine's `staging_dir` and awaits `completion` to learn
/// when every piece has landed on disk.
pub struct DownloadHandle {
    pub torrent_id: usize,
    /// Largest file in the torrent, **relative** to staging_dir. The
    /// consumer joins this onto staging_dir to get the absolute path.
    pub final_path: PathBuf,
    pub completion: tokio::sync::oneshot::Receiver<Result<()>>,
}

impl TorrentEngine {
    pub async fn new(staging_dir: PathBuf) -> Result<Self> {
        let s = session::TorrentSession::new(staging_dir.clone()).await?;
        let server = http_server::StreamingServer::spawn(s.inner.clone()).await?;
        Ok(Self {
            session: s.inner,
            base_url: format!("http://{}", server.addr),
            staging_dir,
        })
    }

    /// Ensure a torrent is in the Live state, unpausing and waiting if needed.
    /// Returns Err if the torrent does not transition to Live within 10 seconds.
    async fn ensure_torrent_live(&self, handle: &Arc<librqbit::ManagedTorrent>) -> Result<()> {
        if handle.live().is_none() {
            let _ = self.session.unpause(handle).await;
            let live_deadline = std::time::Instant::now() + Duration::from_secs(10);
            while handle.live().is_none() {
                if std::time::Instant::now() >= live_deadline {
                    return Err(anyhow!(
                        "torrent did not transition to Live state within 10s"
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok(())
    }

    /// Directory librqbit writes torrent payloads into. Callers join
    /// [`DownloadHandle::final_path`] onto this to get an absolute path
    /// suitable for `MpdBridge::queue_and_play` or the download translator.
    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    /// Add a torrent to the session, or return the existing handle if a
    /// torrent with the same info_hash is already managed.
    ///
    /// Re-adding a magnet that's already in the session causes
    /// `librqbit::Session::add_torrent` to re-run the metadata handshake
    /// against peers, which hangs when those peers are saturated by the
    /// existing torrent's stream. The fast path here parses the magnet
    /// URI, looks up the info_hash in the session, and short-circuits
    /// before calling `add_torrent`. Non-magnet inputs fall through to
    /// the slow path (we still bound it with `METADATA_TIMEOUT`).
    async fn add_or_reuse_handle(
        &self,
        magnet_or_url: &str,
    ) -> Result<Arc<librqbit::ManagedTorrent>> {
        use librqbit::{AddTorrent, AddTorrentOptions, Magnet};

        if let Ok(magnet) = Magnet::parse(magnet_or_url) {
            if let Some(info_hash) = magnet.as_id20() {
                if let Some(existing) = self.session.with_torrents(|torrents| {
                    for (_, t) in torrents {
                        if t.info_hash() == info_hash {
                            return Some(t.clone());
                        }
                    }
                    None
                }) {
                    return Ok(existing);
                }
            }
        }

        let add_fut = self.session.add_torrent(
            AddTorrent::from_url(magnet_or_url),
            Some(AddTorrentOptions {
                paused: false,
                overwrite: true,
                ..Default::default()
            }),
        );
        let resp = tokio::time::timeout(METADATA_TIMEOUT, add_fut)
            .await
            .map_err(|_| {
                anyhow!(
                    "torrent metadata fetch timed out after {}s — magnet has no reachable peers",
                    METADATA_TIMEOUT.as_secs()
                )
            })?
            .context("adding torrent to librqbit session")?;

        resp.into_handle()
            .ok_or_else(|| anyhow!("librqbit returned a list-only response, not a handle"))
    }

    /// Add a torrent and return an HTTP URL pointing at the largest video
    /// file inside it. mpv plays this URL directly with seek support via
    /// HTTP Range requests; librqbit prioritises pieces ahead of the read
    /// cursor automatically once mpv connects.
    pub async fn start_stream(&self, magnet_or_url: &str) -> Result<String> {
        let handle = self.add_or_reuse_handle(magnet_or_url).await?;
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

        let file_idx =
            pick_video_file(&files).ok_or_else(|| anyhow!("no playable video file in torrent"))?;

        // librqbit's HTTP stream endpoint returns 500 while the torrent is in
        // `Initializing` (e.g. checksum-validating already-downloaded pieces
        // from a previous session). mpv only retries a fetch a few times
        // before giving up — if it hits 500s during this window the player
        // ends up showing a stuck black screen with no data flowing. Wait
        // for the state machine to leave `Initializing` before we hand the
        // URL back to the caller (who will `loadfile_replace` mpv into it).
        tokio::time::timeout(METADATA_TIMEOUT, handle.wait_until_initialized())
            .await
            .map_err(|_| {
                anyhow!(
                    "torrent initialization timed out after {}s",
                    METADATA_TIMEOUT.as_secs()
                )
            })?
            .context("waiting for librqbit torrent to leave Initializing state")?;

        // Persistence-restored torrents come back in their previous
        // `Paused`/`Live` state. `wait_until_initialized` is satisfied by
        // either, but a `Paused` torrent's HTTP stream endpoint accepts
        // connections and then hangs without writing — its `FileStream`
        // poll-read returns `Pending` because the chunk_tracker hasn't
        // been hydrated yet, and nothing wakes the waker without active
        // peer fetching. mpv reads no bytes, gives up, and the user sees
        // a black window with our "Fetching torrent metadata…" OSD frozen.
        // Force-unpause and wait for `Live` before serving.
        self.ensure_torrent_live(&handle).await?;

        let stream_url = stream_url_for(&self.base_url, id, file_idx);

        // Even after Live, the streaming path can take seconds to wire
        // up its chunk_tracker so reads beyond piece 0 don't return
        // Pending forever. We saw clicks 1–2 fail and click 3 work after
        // a 3 min gap, with no `Cannot load libcuda.so.1` (mpv decoder
        // entry) until the click that succeeded. Probing the file
        // *in-process* via `ManagedTorrent::stream` exercises the real
        // piece-fetch path that mpv would use over HTTP, but without
        // competing for HTTP slots or being subject to mpv's stingy
        // retry budget — once 16 MiB flows, mpv reading the same URL
        // also flows.
        probe_via_in_process_stream(&handle, file_idx).await?;

        // The HTTP server (librqbit's `HttpApi`) is a separate path from
        // the in-process FileStream above; mpv only ever talks to it via
        // HTTP. On the very first call after engine boot the HTTP server
        // can hold the first request for several seconds — long enough
        // for mpv to give up and leave the user with a blank window —
        // even when the underlying piece path is hot. Hit the endpoint
        // ourselves so its first-request warmup is on us, not on mpv.
        probe_http_endpoint(&stream_url).await?;

        Ok(stream_url)
    }

    /// Add a torrent for bulk download and return a [`DownloadHandle`]
    /// whose `completion` future resolves when every piece is on disk.
    ///
    /// Unlike [`start_stream`](Self::start_stream), the caller never opens
    /// librqbit's `/stream/` endpoint, so librqbit falls back to its
    /// default rarest-first piece scheduler — appropriate for the `d`
    /// keybind workflow where the user wants the full torrent before
    /// playback/organisation.
    pub async fn start_download(&self, magnet_or_url: &str) -> Result<DownloadHandle> {
        let handle = self.add_or_reuse_handle(magnet_or_url).await?;
        let torrent_id = handle.id();

        // A persistence-restored handle starts paused (Session::Json
        // remembers the paused state across restarts). The download
        // path doesn't probe via `ManagedTorrent::stream` like
        // start_stream does, so without an explicit unpause here the
        // translator would just wait on `wait_until_completed` forever
        // while live() stays None.
        self.ensure_torrent_live(&handle).await?;

        // Largest file → the "main" payload the download_translator will
        // later move into the user's library.
        let final_path: PathBuf = handle
            .with_metadata(|m| {
                m.file_infos
                    .iter()
                    .max_by_key(|fi| fi.len)
                    .map(|fi| fi.relative_filename.clone())
                    .unwrap_or_default()
            })
            .context("reading torrent metadata for final path")?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let h2 = Arc::clone(&handle);
        tokio::spawn(async move {
            let res = h2.wait_until_completed().await;
            // Receiver may be gone (caller dropped the handle); ignore the
            // send error in that case.
            let _ = tx.send(res);
        });

        Ok(DownloadHandle {
            torrent_id,
            final_path,
            completion: rx,
        })
    }

    /// Add a music torrent and return per-track HTTP stream URLs.
    ///
    /// Behaviour mirrors [`start_stream`](Self::start_stream) — adds the
    /// torrent, waits for metadata, force-unpauses + waits for `Live` so
    /// the streaming HTTP path can serve reads — but instead of picking the
    /// single largest video file it returns every audio file in the torrent
    /// as an [`AlbumStream`].
    ///
    /// Audio file selection (see [`pick_album_tracks`]):
    ///   1. natural-sort by full relative path so multi-disc albums (`CD1/`,
    ///      `CD2/`) interleave correctly,
    ///   2. prefer FLAC: if any `.flac` files are present, return only those;
    ///      otherwise return all audio files. mpd handles format diversity
    ///      natively, so the fallback path "just works" on lossy-only
    ///      torrents — this is purely about avoiding showing both the
    ///      lossless and lossy copies of the same album as a 24-track queue.
    pub async fn start_album_stream(&self, magnet_or_url: &str) -> Result<AlbumStream> {
        let handle = self.add_or_reuse_handle(magnet_or_url).await?;
        let torrent_id = handle.id();

        // Same persistence-paused unpause + wait-for-Live dance as
        // start_stream / start_download — without it, opening a track URL
        // would hang on a paused torrent.
        self.ensure_torrent_live(&handle).await?;

        let files: Vec<(usize, PathBuf, u64)> = handle
            .with_metadata(|m| {
                m.file_infos
                    .iter()
                    .enumerate()
                    .map(|(i, fi)| (i, fi.relative_filename.clone(), fi.len))
                    .collect()
            })
            .context("reading torrent metadata for album track list")?;

        let picked = pick_album_tracks(&files);
        if picked.is_empty() {
            return Err(anyhow!(
                "no audio files (flac/mp3/m4a/aac/ogg/opus/wav) in torrent"
            ));
        }

        let tracks = picked
            .into_iter()
            .map(|(file_idx, path, size)| TrackStream {
                url: stream_url_for(&self.base_url, torrent_id, file_idx),
                filename: path.display().to_string(),
                size_bytes: size,
            })
            .collect();

        Ok(AlbumStream { torrent_id, tracks })
    }
}

/// Pull bytes from the file *in process* via librqbit's `FileStream`,
/// bypassing the HTTP layer entirely. This forces the real piece-fetch
/// path that mpv will use to wake up and confirms reads flow past the
/// first piece — a pure-HTTP probe that only reads piece 0 has been
/// observed to succeed while mpv (which reads further) still hangs,
/// because chunk_tracker hadn't yet caught up on subsequent pieces.
///
/// Returns Ok once `MIN_BYTES_TO_FLOW` have been read, or the stream
/// reaches EOF (whichever first). Returns Err on read error/timeout.
async fn probe_via_in_process_stream(
    handle: &Arc<librqbit::ManagedTorrent>,
    file_id: usize,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    // 16 MiB picks up several pieces on typical torrents (256 KiB–4 MiB
    // pieces) so we exercise more than just piece 0.
    const MIN_BYTES_TO_FLOW: usize = 16 * 1024 * 1024;
    const READ_DEADLINE: Duration = Duration::from_secs(15);

    let mut fs = handle
        .clone()
        .stream(file_id)
        .context("opening librqbit FileStream for warmup probe")?;

    let mut buf = vec![0u8; 256 * 1024];
    let mut total_read: usize = 0;

    let result = tokio::time::timeout(READ_DEADLINE, async {
        while total_read < MIN_BYTES_TO_FLOW {
            let n = fs
                .read(&mut buf)
                .await
                .context("FileStream read errored during warmup")?;
            if n == 0 {
                // EOF before reaching MIN_BYTES_TO_FLOW means the file is
                // smaller than 16 MiB. That's fine — we've drained the
                // whole file successfully.
                break;
            }
            total_read = total_read.saturating_add(n);
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            tracing::info!(
                target: "torrent_engine",
                bytes_read = total_read,
                "warmup probe drained librqbit FileStream"
            );
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!(
            "warmup probe timed out after {}s with only {total_read} bytes read",
            READ_DEADLINE.as_secs()
        )),
    }
}

/// Hit our HTTP stream endpoint with a small windowed `Range` GET and
/// confirm both that the first chunk flows AND the server returns
/// `206 Partial Content` (i.e. actually honours the Range header).
///
/// The 206 check is load-bearing: librqbit 8.1.1 upstream answered
/// windowed ranges (`bytes=N-M`) with `200 OK` and the full file body
/// from offset zero. mpv's MKV demuxer issues exactly that shape of
/// request to read the SeekHead at the end of the file, saw a server
/// claiming `accept-ranges: bytes` but ignoring the actual Range, and
/// emitted `end-file=error` ~3 ms after `start-file`. Our replacement
/// server in `http_server.rs` parses windowed ranges correctly; this
/// probe defends that contract so a future regression — ours or
/// upstream's — fails loudly instead of silently breaking playback.
///
/// We deliberately do NOT call `resp.bytes()` — reading just the first
/// chunk and dropping the response is enough to confirm the path is hot
/// without monopolising the connection, freeing librqbit to serve mpv's
/// own fetch a moment later.
async fn probe_http_endpoint(url: &str) -> Result<()> {
    const ATTEMPTS: u32 = 5;
    const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

    let client = reqwest::Client::builder()
        .build()
        .context("building reqwest client for HTTP warmup probe")?;

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=ATTEMPTS {
        let send_fut = client
            .get(url)
            .header("Range", "bytes=0-65535")
            .timeout(PER_ATTEMPT_TIMEOUT)
            .send();

        match tokio::time::timeout(PER_ATTEMPT_TIMEOUT, send_fut).await {
            Ok(Ok(mut resp)) if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
                let chunk_fut = resp.chunk();
                match tokio::time::timeout(PER_ATTEMPT_TIMEOUT, chunk_fut).await {
                    Ok(Ok(Some(b))) if !b.is_empty() => {
                        tracing::info!(
                            target: "torrent_engine",
                            attempt,
                            chunk_bytes = b.len(),
                            "HTTP warmup probe succeeded (206 Partial Content)"
                        );
                        // Drop `resp` so the connection closes and
                        // librqbit stops serving body bytes we don't
                        // need. mpv will open its own connection next.
                        drop(resp);
                        return Ok(());
                    }
                    Ok(Ok(_)) => last_err = Some(anyhow!("HTTP probe got empty body")),
                    Ok(Err(e)) => last_err = Some(anyhow!("HTTP probe chunk error: {e}")),
                    Err(_) => last_err = Some(anyhow!("HTTP probe chunk read timed out")),
                }
            }
            Ok(Ok(resp)) => {
                last_err = Some(anyhow!(
                    "HTTP probe got status {} (expected 206 Partial Content; \
                     server may be ignoring the Range header)",
                    resp.status()
                ))
            }
            Ok(Err(e)) => last_err = Some(anyhow!("HTTP probe send error: {e}")),
            Err(_) => last_err = Some(anyhow!("HTTP probe request timed out")),
        }

        tracing::debug!(
            target: "torrent_engine",
            "HTTP warmup probe attempt {attempt}/{ATTEMPTS} failed; retrying"
        );
        tokio::time::sleep(Duration::from_millis(
            500_u64.saturating_mul(attempt as u64),
        ))
        .await;
    }

    Err(anyhow!(
        "HTTP stream endpoint never served bytes after {ATTEMPTS} attempts: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
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

/// Pick audio files for album streaming, sorted into playback order.
///
/// Steps:
///   1. filter to known audio extensions ([`AUDIO_EXTS`]),
///   2. if any `.flac` is present, drop everything else (prefer-lossless),
///   3. sort by full relative path with natural ordering so multi-disc albums
///      interleave correctly (e.g., `CD2` before `CD10`).
///
/// Returns `(file_idx, path, size)` triples in queue order.
fn pick_album_tracks(files: &[(usize, PathBuf, u64)]) -> Vec<(usize, PathBuf, u64)> {
    fn ext_of(p: &PathBuf) -> Option<String> {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
    }

    /// Natural-order comparator for paths: splits trailing numeric components
    /// and compares them numerically so "CD2" < "CD10".
    fn natural_cmp(a: &PathBuf, b: &PathBuf) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        fn parse_trailing_number(s: &str) -> (String, Option<u64>) {
            let mut last_digit_end = s.len();
            for (i, c) in s.char_indices().rev() {
                if !c.is_ascii_digit() {
                    last_digit_end = i + c.len_utf8();
                    break;
                }
            }
            if last_digit_end == s.len() && !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                // Entire string is numeric
                return (String::new(), s.parse::<u64>().ok());
            }
            if last_digit_end < s.len() {
                let prefix = &s[..last_digit_end];
                let num_part = &s[last_digit_end..];
                (prefix.to_string(), num_part.parse::<u64>().ok())
            } else {
                (s.to_string(), None)
            }
        }

        // Split both paths into components
        let a_comps: Vec<_> = a.iter().collect();
        let b_comps: Vec<_> = b.iter().collect();

        for (ac, bc) in a_comps.iter().zip(b_comps.iter()) {
            let a_str = ac.to_string_lossy();
            let b_str = bc.to_string_lossy();
            let (a_prefix, a_num) = parse_trailing_number(&a_str);
            let (b_prefix, b_num) = parse_trailing_number(&b_str);

            match a_prefix.cmp(&b_prefix) {
                Ordering::Equal => match (a_num, b_num) {
                    (Some(an), Some(bn)) => match an.cmp(&bn) {
                        Ordering::Equal => continue,
                        other => return other,
                    },
                    (Some(_), None) => return Ordering::Greater,
                    (None, Some(_)) => return Ordering::Less,
                    (None, None) => continue,
                },
                other => return other,
            }
        }

        a_comps.len().cmp(&b_comps.len())
    }

    let audio: Vec<&(usize, PathBuf, u64)> = files
        .iter()
        .filter(|(_, p, _)| {
            ext_of(p)
                .map(|e| AUDIO_EXTS.contains(&e.as_str()))
                .unwrap_or(false)
        })
        .collect();

    let has_flac = audio
        .iter()
        .any(|(_, p, _)| ext_of(p).map(|e| e == "flac").unwrap_or(false));

    let mut picked: Vec<(usize, PathBuf, u64)> = audio
        .into_iter()
        .filter(|(_, p, _)| {
            if has_flac {
                ext_of(p).map(|e| e == "flac").unwrap_or(false)
            } else {
                true
            }
        })
        .cloned()
        .collect();

    picked.sort_by(|a, b| natural_cmp(&a.1, &b.1));
    picked
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

    #[test]
    fn album_picker_prefers_flac_when_mixed() {
        let files = vec![
            (0, "01.mp3".into(), 5_000_000),
            (1, "02.mp3".into(), 5_000_000),
            (2, "01.flac".into(), 30_000_000),
            (3, "02.flac".into(), 30_000_000),
            (4, "cover.jpg".into(), 200_000),
        ];
        let picked = pick_album_tracks(&files);
        let names: Vec<String> = picked
            .iter()
            .map(|(_, p, _)| p.display().to_string())
            .collect();
        assert_eq!(names, vec!["01.flac", "02.flac"]);
    }

    #[test]
    fn album_picker_falls_back_when_no_flac() {
        let files = vec![
            (0, "track2.mp3".into(), 5_000_000),
            (1, "track1.mp3".into(), 5_000_000),
        ];
        let picked = pick_album_tracks(&files);
        let names: Vec<String> = picked
            .iter()
            .map(|(_, p, _)| p.display().to_string())
            .collect();
        // sorted by path
        assert_eq!(names, vec!["track1.mp3", "track2.mp3"]);
    }

    #[test]
    fn album_picker_interleaves_multi_disc() {
        let files = vec![
            (0, "CD2/01.flac".into(), 30_000_000),
            (1, "CD1/02.flac".into(), 30_000_000),
            (2, "CD1/01.flac".into(), 30_000_000),
            (3, "CD2/02.flac".into(), 30_000_000),
        ];
        let picked = pick_album_tracks(&files);
        let names: Vec<String> = picked
            .iter()
            .map(|(_, p, _)| p.display().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["CD1/01.flac", "CD1/02.flac", "CD2/01.flac", "CD2/02.flac"]
        );
    }

    #[test]
    fn album_picker_returns_empty_when_no_audio() {
        let files = vec![
            (0, "movie.mkv".into(), 1_000_000),
            (1, "readme.txt".into(), 100),
        ];
        assert!(pick_album_tracks(&files).is_empty());
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
