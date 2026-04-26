//! Background refresh task for the anime bridge. Fetches the latest
//! Fribb snapshot from GitHub on a 24h cadence; uses ETag for cheap
//! freshness checks. On success, atomically swaps the in-memory
//! `AnimeIndex` via `AnimeBridge::swap_index`.
//!
//! HTTP boundary lives behind a `BridgeHttp` trait so cache-test
//! mocks can count calls and inject canned responses without
//! standing up a real server. Mirrors the TVDB `HttpFetch` pattern
//! at `runtime/src/tvdb/http.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::{AnimeBridge, AnimeIndex};

const FRIBB_URL: &str = "https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json";
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Outcome of a single fetch attempt — either fresh data, no-change,
/// or an error.
#[derive(Debug)]
pub enum FetchOutcome {
    /// 200 OK, body returned along with the new ETag (if any).
    Fresh { body: Vec<u8>, etag: Option<String> },
    /// 304 Not Modified — current cache is still authoritative.
    NotModified,
}

#[async_trait]
pub trait BridgeHttp: Send + Sync + 'static {
    async fn fetch(&self, url: &str, etag: Option<&str>) -> Result<FetchOutcome>;
}

pub struct ReqwestBridgeHttp {
    client: reqwest::Client,
}

impl ReqwestBridgeHttp {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("stui-runtime/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .context("anime_bridge: build reqwest client")?;
        Ok(Self { client })
    }
}

#[async_trait]
impl BridgeHttp for ReqwestBridgeHttp {
    async fn fetch(&self, url: &str, etag: Option<&str>) -> Result<FetchOutcome> {
        let mut req = self.client.get(url);
        if let Some(tag) = etag {
            req = req.header("If-None-Match", tag);
        }
        let resp = req.send().await.context("anime_bridge: HTTP send failed")?;
        let status = resp.status().as_u16();
        match status {
            304 => Ok(FetchOutcome::NotModified),
            200 => {
                let new_etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                let body = resp
                    .bytes()
                    .await
                    .context("anime_bridge: read body failed")?
                    .to_vec();
                Ok(FetchOutcome::Fresh { body, etag: new_etag })
            }
            other => anyhow::bail!("anime_bridge: HTTP {other}"),
        }
    }
}

/// Spawn the 24h refresh loop. Intended to be called from an async
/// context (e.g., the IPC layer's startup, after `Engine::new()`).
/// The task runs forever; the spawned `JoinHandle` is dropped (the
/// task continues in the background).
pub fn spawn_refresh_task(
    bridge: Arc<AnimeBridge>,
    http: Arc<dyn BridgeHttp>,
    cache_dir: PathBuf,
) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_one_refresh(&bridge, &*http, &cache_dir).await {
                warn!(err = %e, "anime_bridge: refresh attempt failed");
            }
            sleep(REFRESH_INTERVAL).await;
        }
    });
}

/// One-shot refresh attempt, exposed for tests. Reads the cached
/// ETag from `{cache_dir}/anime-bridge.etag`; sends `If-None-Match`
/// if present. On 200, writes body + ETag and swaps the index.
pub async fn run_one_refresh(
    bridge: &Arc<AnimeBridge>,
    http: &dyn BridgeHttp,
    cache_dir: &Path,
) -> Result<()> {
    // Best-effort cache dir creation; ignore "already exists" errors.
    if let Err(e) = tokio::fs::create_dir_all(cache_dir).await {
        debug!(err = %e, "anime_bridge: create_dir_all returned (non-fatal if already exists)");
    }
    let body_path = cache_dir.join("anime-bridge.json");
    let etag_path = cache_dir.join("anime-bridge.etag");

    let cached_etag = tokio::fs::read_to_string(&etag_path).await.ok();

    let outcome = http.fetch(FRIBB_URL, cached_etag.as_deref()).await?;
    match outcome {
        FetchOutcome::NotModified => {
            debug!("anime_bridge: 304 Not Modified");
            Ok(())
        }
        FetchOutcome::Fresh { body, etag } => {
            // Try to parse BEFORE writing — a corrupt response
            // shouldn't overwrite our last-good cache.
            let new_index = AnimeIndex::from_json(&body)
                .context("anime_bridge: parse refreshed snapshot")?;

            if let Err(e) = tokio::fs::write(&body_path, &body).await {
                warn!(err = %e, "anime_bridge: cache body write failed (non-fatal)");
            }
            if let Some(tag) = etag {
                if let Err(e) = tokio::fs::write(&etag_path, &tag).await {
                    warn!(err = %e, "anime_bridge: cache etag write failed (non-fatal)");
                }
            }

            info!(entries = new_index.by_mal.len(), "anime_bridge: refreshed and swapping index");
            bridge.swap_index(Arc::new(new_index));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// Counting mock — returns one canned outcome per call. Calls
    /// past `responses.len()` panic the test.
    struct MockHttp {
        responses: Mutex<Vec<Result<FetchOutcome>>>,
        calls: AtomicU32,
        last_etag: Mutex<Option<String>>,
    }
    impl MockHttp {
        fn new(responses: Vec<Result<FetchOutcome>>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses),
                calls: AtomicU32::new(0),
                last_etag: Mutex::new(None),
            })
        }
        #[allow(dead_code)]
        fn calls(&self) -> u32 { self.calls.load(Ordering::SeqCst) }
        fn last_etag(&self) -> Option<String> { self.last_etag.lock().unwrap().clone() }
    }
    #[async_trait]
    impl BridgeHttp for MockHttp {
        async fn fetch(&self, _url: &str, etag: Option<&str>) -> Result<FetchOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_etag.lock().unwrap() = etag.map(String::from);
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn fixture_one() -> Vec<u8> {
        br#"[{ "mal_id": 1, "imdb_id": "tt0213338" }]"#.to_vec()
    }

    fn fixture_two() -> Vec<u8> {
        br#"[
            { "mal_id": 1, "imdb_id": "tt0213338" },
            { "mal_id": 2, "imdb_id": "tt0388629" }
        ]"#.to_vec()
    }

    #[tokio::test]
    async fn refresh_swaps_index_on_200() {
        let bridge = AnimeBridge::new();
        let initial_count = bridge.current().by_mal.len();
        let http = MockHttp::new(vec![Ok(FetchOutcome::Fresh {
            body: fixture_two(),
            etag: Some("\"deadbeef\"".to_string()),
        })]);
        let tmp = tempfile::tempdir().unwrap();

        run_one_refresh(&bridge, &*http, tmp.path()).await.unwrap();

        let after = bridge.current();
        assert_eq!(after.by_mal.len(), 2, "fresh fixture has 2 entries");
        assert_ne!(after.by_mal.len(), initial_count, "index swapped");
    }

    #[tokio::test]
    async fn refresh_noop_on_304() {
        let bridge = AnimeBridge::new();
        let before = Arc::clone(&bridge.current());
        let http = MockHttp::new(vec![Ok(FetchOutcome::NotModified)]);
        let tmp = tempfile::tempdir().unwrap();

        run_one_refresh(&bridge, &*http, tmp.path()).await.unwrap();

        let after = bridge.current();
        assert!(Arc::ptr_eq(&before, &after), "304 should not swap the index");
    }

    #[tokio::test]
    async fn refresh_keeps_old_index_on_5xx() {
        let bridge = AnimeBridge::new();
        let before = Arc::clone(&bridge.current());
        let http = MockHttp::new(vec![Err(anyhow::anyhow!("HTTP 503"))]);
        let tmp = tempfile::tempdir().unwrap();

        let result = run_one_refresh(&bridge, &*http, tmp.path()).await;
        assert!(result.is_err());

        let after = bridge.current();
        assert!(Arc::ptr_eq(&before, &after), "5xx should not swap the index");
    }

    #[tokio::test]
    async fn refresh_keeps_old_index_on_parse_failure() {
        let bridge = AnimeBridge::new();
        let before = Arc::clone(&bridge.current());
        let http = MockHttp::new(vec![Ok(FetchOutcome::Fresh {
            body: b"not json".to_vec(),
            etag: None,
        })]);
        let tmp = tempfile::tempdir().unwrap();

        let result = run_one_refresh(&bridge, &*http, tmp.path()).await;
        assert!(result.is_err());

        let after = bridge.current();
        assert!(Arc::ptr_eq(&before, &after), "parse failure should not swap the index");
    }

    #[tokio::test]
    async fn refresh_writes_cache_files_on_200() {
        let bridge = AnimeBridge::new();
        let http = MockHttp::new(vec![Ok(FetchOutcome::Fresh {
            body: fixture_one(),
            etag: Some("\"abc\"".to_string()),
        })]);
        let tmp = tempfile::tempdir().unwrap();

        run_one_refresh(&bridge, &*http, tmp.path()).await.unwrap();

        let body = tokio::fs::read(tmp.path().join("anime-bridge.json")).await.unwrap();
        assert_eq!(body, fixture_one());
        let etag = tokio::fs::read_to_string(tmp.path().join("anime-bridge.etag")).await.unwrap();
        assert_eq!(etag, "\"abc\"");
    }

    #[tokio::test]
    async fn refresh_sends_etag_when_cached() {
        let bridge = AnimeBridge::new();
        let tmp = tempfile::tempdir().unwrap();
        // Pre-seed an ETag.
        tokio::fs::write(tmp.path().join("anime-bridge.etag"), "\"cached-etag\"")
            .await
            .unwrap();

        let http = MockHttp::new(vec![Ok(FetchOutcome::NotModified)]);
        run_one_refresh(&bridge, &*http, tmp.path()).await.unwrap();

        assert_eq!(http.last_etag().as_deref(), Some("\"cached-etag\""));
    }
}
