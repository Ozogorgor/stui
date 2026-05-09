//! Localhost HTTP server that exposes librqbit's streaming API to mpv.
//!
//! Why we don't use librqbit's built-in `HttpApi` directly: as of librqbit
//! 8.1.1, `http_api/handlers/streaming.rs` only parses open-ended ranges
//! (`bytes=N-`). Any windowed `bytes=N-M` request silently falls through to
//! a `200 OK` with the full file body from byte 0. mpv's MKV demuxer issues
//! exactly those windowed seeks (e.g. to read SeekHead/Cues at the end of
//! the file), sees `accept-ranges: bytes` advertised but every windowed
//! range answered with the whole file from offset zero, and bails with
//! `end-file=error` ~3 ms after `start-file`. This server replaces the
//! upstream handler with a minimal axum router that:
//!
//! 1. Routes `GET /torrents/<id>/stream/<file_idx>` exactly as librqbit's
//!    upstream URL contract specifies, so URL builders elsewhere stay stable.
//! 2. Honours all three RFC 7233 `Range` forms — `bytes=N-`, `bytes=N-M`,
//!    `bytes=-N` — and emits `206 Partial Content` with `Content-Range` +
//!    truncated body for windowed requests.
//! 3. Re-uses librqbit's `Api::api_stream` (`FileStream: AsyncRead +
//!    AsyncSeek`) underneath, so we keep the stream-aware piece scheduler
//!    that prioritises bytes near the playhead — only the HTTP framing
//!    changes.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http::{header, HeaderMap, HeaderValue, StatusCode};
use librqbit::api::{Api, TorrentIdOrHash};
use librqbit::Session;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Clone)]
struct AppState {
    api: Arc<Api>,
}

pub struct StreamingServer {
    pub addr: SocketAddr,
}

impl StreamingServer {
    /// Bind our streaming router to `127.0.0.1:0` and spawn it in the
    /// background. Returns once the listener has a port.
    pub async fn spawn(session: Arc<Session>) -> Result<Self> {
        // `tracing-subscriber-utils` feature is enabled in our deps, so
        // `Api::new` takes a third `Option<LineBroadcast>` arg. We don't use
        // log streaming, so pass `None, None`.
        let api = Arc::new(Api::new(session, None, None));
        let state = AppState { api };

        let app = Router::new()
            .route(
                "/torrents/{id}/stream/{file_idx}",
                get(stream_handler),
            )
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding torrent_engine http server to 127.0.0.1:0")?;
        let addr = listener.local_addr().context("reading listener addr")?;

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("torrent_engine http server died: {e:#}");
            }
        });

        Ok(Self { addr })
    }
}

async fn stream_handler(
    State(state): State<AppState>,
    Path((id, file_idx)): Path<(usize, usize)>,
    headers: HeaderMap,
) -> Response {
    let idx = TorrentIdOrHash::from(id);

    let mut stream = match state.api.api_stream(idx, file_idx) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "torrent_engine",
                torrent_id = id,
                file_idx,
                "stream open failed: {e:#}"
            );
            return (StatusCode::NOT_FOUND, format!("stream not found: {e}"))
                .into_response();
        }
    };

    let total_len = stream.len();
    let mime = state
        .api
        .torrent_file_mime_type(idx, file_idx)
        .unwrap_or("application/octet-stream");

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, total_len));

    let mut out = HeaderMap::new();
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(mt) = HeaderValue::from_str(mime) {
        out.insert(header::CONTENT_TYPE, mt);
    }

    let (status, start, len_to_send) = match range {
        Some((start, end)) => {
            let len = end - start + 1;
            out.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
            out.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{total_len}"))
                    .unwrap(),
            );
            (StatusCode::PARTIAL_CONTENT, start, Some(len))
        }
        None => {
            out.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&total_len.to_string()).unwrap(),
            );
            (StatusCode::OK, 0, None)
        }
    };

    if start > 0 {
        if let Err(e) = stream.seek(std::io::SeekFrom::Start(start)).await {
            tracing::warn!(
                target: "torrent_engine",
                torrent_id = id,
                file_idx,
                start,
                "stream seek failed: {e:#}"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("seek failed: {e}"),
            )
                .into_response();
        }
    }

    let body: Body = match len_to_send {
        Some(len) => {
            // `AsyncReadExt::take` enforces the upper bound so we don't
            // overrun a windowed range request.
            let limited = stream.take(len);
            let s = tokio_util::io::ReaderStream::with_capacity(limited, 65536);
            Body::from_stream(s)
        }
        None => {
            let s = tokio_util::io::ReaderStream::with_capacity(stream, 65536);
            Body::from_stream(s)
        }
    };

    (status, out, body).into_response()
}

/// Parse an HTTP `Range` header value against a known resource length.
///
/// Returns `Some((start, end_inclusive))` clamped to `[0, total_len)`. Returns
/// `None` for unparseable, unsatisfiable, or non-`bytes` ranges (caller falls
/// back to a `200 OK` full-body response, per RFC 7233 §4.4 leniency).
fn parse_range(s: &str, total_len: u64) -> Option<(u64, u64)> {
    if total_len == 0 {
        return None;
    }
    let s = s.trim().strip_prefix("bytes=")?;
    // Multi-range (comma-separated) is rare in practice and non-trivial to
    // serialise as multipart; mpv never sends it. Reject and let caller 200.
    if s.contains(',') {
        return None;
    }
    let (a, b) = s.split_once('-')?;
    let a = a.trim();
    let b = b.trim();
    let last = total_len - 1;

    if a.is_empty() {
        // Suffix form: "bytes=-N" → last N bytes.
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        let n = n.min(total_len);
        Some((total_len - n, last))
    } else {
        let start: u64 = a.parse().ok()?;
        if start > last {
            return None;
        }
        if b.is_empty() {
            Some((start, last))
        } else {
            let end: u64 = b.parse().ok()?;
            let end = end.min(last);
            if end < start {
                return None;
            }
            Some((start, end))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_ended_range_from_zero() {
        assert_eq!(parse_range("bytes=0-", 1000), Some((0, 999)));
    }

    #[test]
    fn open_ended_range_offset() {
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn windowed_range_in_bounds() {
        // The exact form mpv's MKV demuxer issues; this is what librqbit 8.1.1
        // gets wrong and what motivates this whole module's existence.
        assert_eq!(parse_range("bytes=100-499", 1000), Some((100, 499)));
    }

    #[test]
    fn windowed_range_clamps_end() {
        // RFC 7233 §2.1: end above resource length → clamp to last byte.
        assert_eq!(parse_range("bytes=900-9999", 1000), Some((900, 999)));
    }

    #[test]
    fn suffix_range() {
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
    }

    #[test]
    fn suffix_range_larger_than_total() {
        assert_eq!(parse_range("bytes=-9999", 1000), Some((0, 999)));
    }

    #[test]
    fn rejects_unsatisfiable_start() {
        assert_eq!(parse_range("bytes=2000-", 1000), None);
    }

    #[test]
    fn rejects_inverted_window() {
        assert_eq!(parse_range("bytes=500-100", 1000), None);
    }

    #[test]
    fn rejects_multi_range() {
        assert_eq!(parse_range("bytes=0-99,200-299", 1000), None);
    }

    #[test]
    fn rejects_non_bytes_unit() {
        assert_eq!(parse_range("items=0-9", 1000), None);
    }

    #[tokio::test]
    async fn server_binds_to_localhost() {
        let tmp = tempfile::tempdir().unwrap();
        let session = Session::new(tmp.path().to_path_buf()).await.unwrap();
        let server = StreamingServer::spawn(session).await.unwrap();
        assert!(server.addr.ip().is_loopback());
        assert!(server.addr.port() > 0);
    }
}
