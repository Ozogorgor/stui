//! TVDB v4 HTTP client with JWT caching + auto-relogin.
//!
//! Auth flow:
//!   1. POST `/login` with `{"apikey": "..."}` → JWT
//!   2. `Authorization: Bearer JWT` on every subsequent call
//!   3. On 401 we drop the cached JWT and re-login once before giving up.
//!
//! JWTs are valid "for a month" per TVDB; we don't parse the exp claim, we
//! just trust the server's 401 to tell us when to refresh.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use lru::LruCache;
use serde_json::json;
use tokio::sync::{OnceCell, RwLock};
use tracing::{debug, warn};

use super::http::{HttpFetch, ReqwestFetcher};
use super::types::{
    Envelope, EpisodeRecord, EpisodesPayload, ExtendedMovie, ExtendedSeries, LoginData, SearchItem,
};

const BASE_URL: &str = "https://api4.thetvdb.com/v4";
const USER_AGENT: &str = concat!("stui-runtime/", env!("CARGO_PKG_VERSION"));

/// Round-trip dedup cache for `/extended` payloads. User-facing freshness
/// is governed by the orchestrator's `MetadataCache` downstream, so this
/// layer just collapses per-detail-card-open thrash. Capacity-only
/// eviction (no TTL) — `OnceCell` slots are one-shot and the LRU
/// capacity bounds memory.
const EXTENDED_CACHE_CAPACITY: usize = 256;

const ID_RESOLUTION_CACHE_CAPACITY: usize = 256;
const ID_RESOLUTION_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// Manual TTL wrapper for `LruCache` slots — the `lru` crate evicts by
/// capacity only. On read we treat aged slots as missing.
struct CacheEntry<T> {
    value: T,
    inserted_at: Instant,
}

impl<T> CacheEntry<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            inserted_at: Instant::now(),
        }
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        self.inserted_at.elapsed() < ttl
    }
}

/// Search category — TVDB's `type` query param.
#[derive(Debug, Clone, Copy)]
pub enum SearchKind {
    Movie,
    Series,
}

impl SearchKind {
    fn as_str(self) -> &'static str {
        match self {
            SearchKind::Movie => "movie",
            SearchKind::Series => "series",
        }
    }
}

/// Flattened TVDB entry in the shape the catalog expects. Converted to a
/// `catalog::CatalogEntry` at the engine boundary.
#[derive(Debug, Clone)]
pub struct TvdbEntry {
    pub tvdb_id: String,
    pub title: String,
    pub year: Option<String>,
    pub overview: Option<String>,
    pub image_url: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub genres: Vec<String>,
    pub kind: SearchKind,
    /// Primary language (ISO 639-1). TVDB's SearchItem exposes this as
    /// `primary_language`; forwarded so the engine's anime-mix classifier
    /// can tell Japanese animation apart from western animation.
    pub original_language: Option<String>,
}

/// One episode flattened into the shape the IPC layer wants. Built by
/// `TvdbClient::episodes` from `EpisodeRecord`s after season filtering.
#[derive(Debug, Clone)]
pub struct TvdbEpisode {
    pub id: String,
    pub season: u32,
    pub episode: u32,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub runtime_mins: Option<u32>,
}

pub struct TvdbClient {
    http: Arc<dyn HttpFetch>,
    api_key: String,
    jwt: RwLock<Option<String>>,
    extended_cache:
        Mutex<LruCache<u64, Arc<OnceCell<Result<Arc<ExtendedSeries>, Arc<anyhow::Error>>>>>>,
    extended_movie_cache:
        Mutex<LruCache<u64, Arc<OnceCell<Result<Arc<ExtendedMovie>, Arc<anyhow::Error>>>>>>,
    /// Foreign-id → tvdb_id resolution. Negative results (`None`) cache
    /// so we don't re-query for entries TVDB doesn't index. Key is
    /// `(source, id)` where source is "imdb" or "tmdb".
    id_resolution_cache: Mutex<LruCache<(String, String), CacheEntry<Option<String>>>>,
}

impl TvdbClient {
    pub fn new(api_key: String) -> Result<Arc<Self>> {
        let http = Arc::new(ReqwestFetcher::new(USER_AGENT)?);
        Self::with_http(api_key, http)
    }

    /// Crate-internal constructor that accepts a custom `HttpFetch`. Primary
    /// use is dependency injection from the cache-logic tests in this
    /// module; production callers should use [`Self::new`].
    pub(crate) fn with_http(api_key: String, http: Arc<dyn HttpFetch>) -> Result<Arc<Self>> {
        if api_key.trim().is_empty() {
            return Err(anyhow!("tvdb api_key is empty"));
        }
        let cap = NonZeroUsize::new(EXTENDED_CACHE_CAPACITY).unwrap();
        let id_cap = NonZeroUsize::new(ID_RESOLUTION_CACHE_CAPACITY).unwrap();
        Ok(Arc::new(Self {
            http,
            api_key,
            jwt: RwLock::new(None),
            extended_cache: Mutex::new(LruCache::new(cap)),
            extended_movie_cache: Mutex::new(LruCache::new(cap)),
            id_resolution_cache: Mutex::new(LruCache::new(id_cap)),
        }))
    }

    async fn login(&self) -> Result<String> {
        let url = format!("{BASE_URL}/login");
        let body = json!({ "apikey": self.api_key }).to_string();
        let resp = self
            .http
            .post_json(&url, &body)
            .await
            .context("tvdb login request")?;
        if !(200..300).contains(&resp.status) {
            return Err(anyhow!(
                "tvdb /login returned {}: {}",
                resp.status,
                resp.body
            ));
        }
        let env: Envelope<LoginData> =
            serde_json::from_str(&resp.body).context("tvdb login parse")?;
        let data = env.data.ok_or_else(|| {
            anyhow!(
                "tvdb login: missing data field (status={}, message={:?})",
                env.status,
                env.message
            )
        })?;
        debug!("tvdb: obtained JWT");
        Ok(data.token)
    }

    /// Return a cached JWT if one is present; otherwise log in and cache one.
    async fn get_jwt(&self) -> Result<String> {
        if let Some(tok) = self.jwt.read().await.clone() {
            return Ok(tok);
        }
        // Double-checked lock pattern so concurrent callers don't all log in.
        let mut guard = self.jwt.write().await;
        if let Some(tok) = guard.clone() {
            return Ok(tok);
        }
        let tok = self.login().await?;
        *guard = Some(tok.clone());
        Ok(tok)
    }

    /// Drop the cached JWT (next call will relogin).
    async fn clear_jwt(&self) {
        *self.jwt.write().await = None;
    }

    /// GET `path` with Bearer auth, deserializing the envelope's `data`.
    /// On 401 we retry once after forcing a fresh login.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        for attempt in 0..2 {
            let jwt = self.get_jwt().await?;
            let url = format!("{BASE_URL}{path}");
            let resp = self
                .http
                .get_json(&url, &jwt)
                .await
                .with_context(|| format!("tvdb get {path}"))?;
            if resp.status == 401 && attempt == 0 {
                debug!("tvdb: 401 — refreshing JWT and retrying once");
                self.clear_jwt().await;
                continue;
            }
            if !(200..300).contains(&resp.status) {
                return Err(anyhow!(
                    "tvdb {path} returned {}: {}",
                    resp.status,
                    resp.body
                ));
            }
            let env: Envelope<T> =
                serde_json::from_str(&resp.body).context("tvdb parse envelope")?;
            return env.data.ok_or_else(|| {
                anyhow!(
                    "tvdb {path}: missing data field (status={}, message={:?})",
                    env.status,
                    env.message
                )
            });
        }
        Err(anyhow!("tvdb: auth retry exhausted for {path}"))
    }

    /// Free-text search, restricted to movie or series.
    pub async fn search(
        &self,
        query: &str,
        kind: SearchKind,
        limit: u32,
    ) -> Result<Vec<TvdbEntry>> {
        let qs = format!(
            "/search?query={}&type={}&limit={}",
            urlencoding::encode(query),
            kind.as_str(),
            limit,
        );
        let items: Vec<SearchItem> = self.get(&qs).await?;
        Ok(items.into_iter().filter_map(|i| flatten(i, kind)).collect())
    }

    /// Fetch every episode for a series and return only those matching
    /// `requested_season`. TVDB's `default` season-type returns the full
    /// list paginated 500-per-page, so for the vast majority of series
    /// one round trip suffices; we cap pagination at `MAX_EPISODE_PAGES`
    /// to bound very-long-running shows.
    pub async fn episodes(
        &self,
        series_id: &str,
        requested_season: u32,
    ) -> Result<Vec<TvdbEpisode>> {
        let mut raw: Vec<EpisodeRecord> = Vec::new();
        for page in 0..MAX_EPISODE_PAGES {
            let path = format!(
                "/series/{}/episodes/default?page={}",
                urlencoding::encode(series_id),
                page,
            );
            let payload: EpisodesPayload = self.get(&path).await?;
            let n = payload.episodes.len();
            raw.extend(payload.episodes);
            // TVDB's documented page_size is 500. A short page means the
            // last one — anything else and we keep walking. Comparing to
            // the TVDB-documented value rather than `n > 0` keeps us from
            // looping forever if upstream returns an empty page mid-set.
            if n < TVDB_PAGE_SIZE {
                break;
            }
        }
        Ok(flatten_episodes(raw, requested_season))
    }

    /// Fetch `/v4/series/{id}/extended`, cached. Concurrent calls for
    /// the same id de-dupe via per-id `OnceCell` — exactly one HTTP
    /// fetch even if three verbs race.
    ///
    /// Stale or `Err` cache slots are evicted on read; the caller's
    /// next attempt then re-fetches fresh. Failures are NEVER permanent.
    pub async fn extended_series(&self, tvdb_id: u64) -> Result<Arc<ExtendedSeries>> {
        let cell = {
            let mut guard = self
                .extended_cache
                .lock()
                .expect("tvdb cache mutex poisoned");
            // Evict if the slot already resolved to an error so failures
            // aren't permanent — next call gets a fresh fetch.
            if let Some(existing) = guard.get(&tvdb_id) {
                let stale = existing.get().is_some_and(|r| r.is_err());
                if stale {
                    guard.pop(&tvdb_id);
                }
            }
            guard
                .get_or_insert(tvdb_id, || Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cell
            .get_or_init(|| async {
                let path = format!("/series/{tvdb_id}/extended");
                match self.get::<ExtendedSeries>(&path).await {
                    Ok(s) => Ok(Arc::new(s)),
                    Err(e) => Err(Arc::new(e)),
                }
            })
            .await
            .clone();

        result.map_err(|e| anyhow!("tvdb extended_series({tvdb_id}): {:?}", e))
    }

    /// Fetch `/v4/movies/{id}/extended`, cached. Mirror of
    /// [`Self::extended_series`] for the movie shape.
    pub async fn extended_movie(&self, tvdb_id: u64) -> Result<Arc<ExtendedMovie>> {
        let cell = {
            let mut guard = self
                .extended_movie_cache
                .lock()
                .expect("tvdb cache mutex poisoned");
            if let Some(existing) = guard.get(&tvdb_id) {
                if existing.get().is_some_and(|r| r.is_err()) {
                    guard.pop(&tvdb_id);
                }
            }
            guard
                .get_or_insert(tvdb_id, || Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cell
            .get_or_init(|| async {
                let path = format!("/movies/{tvdb_id}/extended");
                match self.get::<ExtendedMovie>(&path).await {
                    Ok(m) => Ok(Arc::new(m)),
                    Err(e) => Err(Arc::new(e)),
                }
            })
            .await
            .clone();

        result.map_err(|e| anyhow!("tvdb extended_movie({tvdb_id}): {:?}", e))
    }

    /// Resolve a foreign id (imdb tt-id or tmdb numeric) to a TVDB id
    /// via `/v4/search/remoteid/{id}`. Both positive and negative
    /// results cache; TTL bounds staleness.
    ///
    /// `source` is "imdb" or "tmdb"; carried for cache-key disambiguation
    /// (the same numeric id can mean different things across sources).
    pub async fn resolve_remote_id(&self, source: &str, id: &str) -> Result<Option<String>> {
        let key = (source.to_string(), id.to_string());

        // Fast path: cache hit on a fresh slot.
        {
            let mut guard = self
                .id_resolution_cache
                .lock()
                .expect("tvdb cache mutex poisoned");
            if let Some(entry) = guard.get(&key) {
                if entry.is_fresh(ID_RESOLUTION_CACHE_TTL) {
                    return Ok(entry.value.clone());
                }
                guard.pop(&key);
            }
        }

        // Miss. Fetch.
        let path = format!("/search/remoteid/{}", urlencoding::encode(id));
        let resolved = match self.get::<Vec<SearchItem>>(&path).await {
            Ok(items) => items.into_iter().find_map(|i| i.tvdb_id),
            // 404 → not in TVDB. Treat as Ok(None) and cache.
            // The error string from get<T> is "tvdb {path} returned 404: {body}".
            // String-matching the status here is intentional: the format
            // string is owned by `get<T>` in this same file, so the
            // coupling is local rather than cross-module.
            Err(e) if e.to_string().contains("returned 404") => None,
            Err(e) => {
                debug!(source = %source, id = %id, err = %e, "tvdb resolve_remote_id failed");
                return Err(e);
            }
        };

        let mut guard = self
            .id_resolution_cache
            .lock()
            .expect("tvdb cache mutex poisoned");
        guard.put(key, CacheEntry::new(resolved.clone()));
        Ok(resolved)
    }
}

// ── Episode pagination ────────────────────────────────────────────────────────

/// TVDB's default `/episodes/default` page size. A response with fewer
/// rows means we're on the last page and pagination can stop.
const TVDB_PAGE_SIZE: usize = 500;

/// Hard cap on episode-pagination round trips. 5 × 500 = 2500 episodes
/// covers everything except long-running shounen, where the per-cour
/// browser UX is the wrong abstraction anyway.
const MAX_EPISODE_PAGES: u32 = 5;

/// Convert raw TVDB rows to the flattened `TvdbEpisode` shape, filtered
/// to one season and sorted by episode number. Pure — split out so the
/// behaviour is testable without an HTTP fixture.
///
/// Rows that lack either an `id` or a `number` are dropped: both are
/// load-bearing (id → wire entry_id, number → ordering), and a row
/// missing them isn't usable as an episode descriptor regardless.
fn flatten_episodes(raw: Vec<EpisodeRecord>, requested_season: u32) -> Vec<TvdbEpisode> {
    let mut out: Vec<TvdbEpisode> = raw
        .into_iter()
        .filter(|e| e.season_number.unwrap_or(0) == requested_season)
        .filter_map(|e| {
            let id = e.id?;
            let number = e.number?;
            Some(TvdbEpisode {
                id: id.to_string(),
                season: requested_season,
                episode: number,
                title: e.name,
                air_date: e.aired,
                runtime_mins: e.runtime,
            })
        })
        .collect();
    out.sort_by_key(|e| e.episode);
    out
}

// ── Conversions ───────────────────────────────────────────────────────────────

fn flatten(item: SearchItem, kind: SearchKind) -> Option<TvdbEntry> {
    let tvdb_id = item.tvdb_id?;
    let title = item.name?;
    let (imdb_id, tmdb_id) = extract_remote_ids(&item.remote_ids);
    Some(TvdbEntry {
        tvdb_id,
        title,
        year: item.year,
        overview: item.overview,
        image_url: item.image_url.or(item.thumbnail),
        imdb_id,
        tmdb_id,
        genres: item.genres,
        kind,
        original_language: item.primary_language,
    })
}

fn extract_remote_ids(ids: &[super::types::RemoteId]) -> (Option<String>, Option<String>) {
    let mut imdb = None;
    let mut tmdb = None;
    for rid in ids {
        match rid.source_name.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("IMDB") => imdb = Some(rid.id.clone()),
            Some(s) if s.contains("MovieDB") || s.eq_ignore_ascii_case("TMDB") => {
                tmdb = Some(rid.id.clone())
            }
            _ => {}
        }
    }
    (imdb, tmdb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        id: Option<u64>,
        number: Option<u32>,
        season: Option<u32>,
        name: Option<&str>,
        aired: Option<&str>,
        runtime: Option<u32>,
    ) -> EpisodeRecord {
        EpisodeRecord {
            id,
            number,
            season_number: season,
            name: name.map(String::from),
            aired: aired.map(String::from),
            runtime,
        }
    }

    #[test]
    fn flatten_filters_to_requested_season_and_sorts_by_number() {
        let raw = vec![
            rec(
                Some(2),
                Some(2),
                Some(1),
                Some("E2"),
                Some("2020-01-08"),
                Some(45),
            ),
            rec(Some(11), Some(1), Some(2), Some("S2E1"), None, None),
            rec(
                Some(1),
                Some(1),
                Some(1),
                Some("E1"),
                Some("2020-01-01"),
                Some(45),
            ),
            rec(Some(3), Some(3), Some(1), Some("E3"), None, None),
        ];
        let s1 = flatten_episodes(raw, 1);
        let nums: Vec<u32> = s1.iter().map(|e| e.episode).collect();
        assert_eq!(nums, vec![1, 2, 3]);
        assert!(s1.iter().all(|e| e.season == 1));
    }

    #[test]
    fn flatten_drops_rows_missing_id_or_number() {
        let raw = vec![
            rec(None, Some(1), Some(1), Some("no id"), None, None),
            rec(Some(1), None, Some(1), Some("no number"), None, None),
            rec(Some(2), Some(7), Some(1), Some("real"), None, None),
        ];
        let eps = flatten_episodes(raw, 1);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].id, "2");
        assert_eq!(eps[0].episode, 7);
    }

    #[test]
    fn flatten_skips_episodes_with_missing_season_when_filtering_a_real_season() {
        // A row without a seasonNumber gets bucketed into season 0 (the
        // unwrap_or default), so it won't accidentally collide with a
        // real season filter.
        let raw = vec![rec(Some(1), Some(1), None, Some("orphan"), None, None)];
        assert!(flatten_episodes(raw, 1).is_empty());
    }

    #[test]
    fn flatten_passes_through_aired_and_runtime() {
        let raw = vec![rec(
            Some(99),
            Some(1),
            Some(1),
            Some("T"),
            Some("2024-01-01"),
            Some(23),
        )];
        let eps = flatten_episodes(raw, 1);
        assert_eq!(eps[0].air_date.as_deref(), Some("2024-01-01"));
        assert_eq!(eps[0].runtime_mins, Some(23));
    }

    // ── Cache-logic tests ─────────────────────────────────────────────────────
    //
    // These tests exercise the public methods on `TvdbClient` introduced in
    // Task 3 (`extended_series`, `extended_movie`, `resolve_remote_id`) by
    // injecting an `HttpFetch` mock via the `with_http` test seam.

    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::http::{HttpFetch, HttpOk};

    /// Counter-based mock — every successful response returns the same
    /// fixture body, but the call count is observable.
    struct CountingFetcher {
        body: String,
        calls: AtomicU32,
    }

    impl CountingFetcher {
        fn new(body: &str) -> Arc<Self> {
            Arc::new(Self {
                body: body.to_string(),
                calls: AtomicU32::new(0),
            })
        }

        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl HttpFetch for CountingFetcher {
        async fn get_json(&self, _url: &str, _jwt: &str) -> Result<HttpOk> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpOk {
                status: 200,
                body: self.body.clone(),
            })
        }

        async fn post_json(&self, _url: &str, _body: &str) -> Result<HttpOk> {
            // Login responses are wrapped in a TVDB envelope.
            Ok(HttpOk {
                status: 200,
                body: r#"{"status":"success","data":{"token":"fake-jwt"}}"#.into(),
            })
        }
    }

    /// 404 mock for negative-cache testing. Returns Ok(HttpOk{status:404})
    /// per the post-Task-1 contract — `HttpFetch` returns `Err` only on
    /// transport failure; status-code dispatch lives in `TvdbClient::get`.
    struct NotFoundFetcher {
        calls: AtomicU32,
    }

    impl NotFoundFetcher {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicU32::new(0),
            })
        }

        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl HttpFetch for NotFoundFetcher {
        async fn get_json(&self, _url: &str, _jwt: &str) -> Result<HttpOk> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpOk {
                status: 404,
                body: "not found".into(),
            })
        }

        async fn post_json(&self, _url: &str, _body: &str) -> Result<HttpOk> {
            Ok(HttpOk {
                status: 200,
                body: r#"{"status":"success","data":{"token":"fake"}}"#.into(),
            })
        }
    }

    #[tokio::test]
    async fn extended_series_dedups_concurrent_fetches() {
        let fixture = r#"{
            "status": "success",
            "data": { "id": 1, "name": "X", "seasons": [], "characters": [], "artworks": [] }
        }"#;
        let mock = CountingFetcher::new(fixture);
        let client = TvdbClient::with_http("fake-key".into(), mock.clone()).unwrap();

        // Race 5 fetches for the same id.
        let mut handles = Vec::new();
        for _ in 0..5 {
            let c = Arc::clone(&client);
            handles.push(tokio::spawn(
                async move { c.extended_series(42).await.unwrap() },
            ));
        }
        for h in handles {
            let s = h.await.unwrap();
            assert_eq!(s.id, 1);
        }

        assert_eq!(
            mock.calls(),
            1,
            "concurrent fetches should de-dupe to one HTTP call"
        );
    }

    #[tokio::test]
    async fn extended_series_serves_second_call_from_cache() {
        let fixture = r#"{
            "status": "success",
            "data": { "id": 7, "name": "X", "seasons": [], "characters": [], "artworks": [] }
        }"#;
        let mock = CountingFetcher::new(fixture);
        let client = TvdbClient::with_http("fake-key".into(), mock.clone()).unwrap();

        let _ = client.extended_series(7).await.unwrap();
        let _ = client.extended_series(7).await.unwrap();

        assert_eq!(mock.calls(), 1, "second call should be served from cache");
    }

    #[tokio::test]
    async fn resolve_remote_id_caches_negative_result() {
        let mock = NotFoundFetcher::new();
        let client = TvdbClient::with_http("fake-key".into(), mock.clone()).unwrap();
        let r1 = client.resolve_remote_id("imdb", "tt0").await.unwrap();
        let r2 = client.resolve_remote_id("imdb", "tt0").await.unwrap();
        assert!(r1.is_none());
        assert!(r2.is_none());
        // Critical assertion: the second call MUST be served from cache.
        // Otherwise every detail-card open re-queries TVDB for entries it
        // already knows are absent.
        assert_eq!(
            mock.calls(),
            1,
            "second resolve_remote_id call should be served from cache"
        );
    }

    #[tokio::test]
    async fn extended_series_different_ids_each_fetch() {
        let fixture = r#"{
            "status": "success",
            "data": { "id": 0, "name": "X", "seasons": [], "characters": [], "artworks": [] }
        }"#;
        let mock = CountingFetcher::new(fixture);
        let client = TvdbClient::with_http("fake-key".into(), mock.clone()).unwrap();

        let _ = tokio::join!(
            client.extended_series(1),
            client.extended_series(2),
            client.extended_series(3),
        );

        assert_eq!(mock.calls(), 3, "distinct ids should each trigger a fetch");
    }
}
