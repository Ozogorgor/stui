//! TVDB v4 HTTP client with JWT caching + auto-relogin.
//!
//! Auth flow:
//!   1. POST `/login` with `{"apikey": "..."}` → JWT
//!   2. `Authorization: Bearer JWT` on every subsequent call
//!   3. On 401 we drop the cached JWT and re-login once before giving up.
//!
//! JWTs are valid "for a month" per TVDB; we don't parse the exp claim, we
//! just trust the server's 401 to tell us when to refresh.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::types::{Envelope, LoginData, SearchItem};

const BASE_URL: &str = "https://api4.thetvdb.com/v4";
const USER_AGENT: &str = concat!("stui-runtime/", env!("CARGO_PKG_VERSION"));

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

/// Fields TVDB can supply for enrichment. Every one is optional so callers
/// can fill only the holes plugins left behind.
#[derive(Debug, Default, Clone)]
pub struct EnrichedFields {
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub rating: Option<f64>,
    pub year: Option<String>,
    pub genres: Vec<String>,
    pub imdb_id: Option<String>,
}

pub struct TvdbClient {
    http: reqwest::Client,
    api_key: String,
    jwt: RwLock<Option<String>>,
}

impl TvdbClient {
    pub fn new(api_key: String) -> Result<Arc<Self>> {
        if api_key.trim().is_empty() {
            return Err(anyhow!("tvdb api_key is empty"));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .context("building tvdb http client")?;
        Ok(Arc::new(Self {
            http,
            api_key,
            jwt: RwLock::new(None),
        }))
    }

    async fn login(&self) -> Result<String> {
        let url = format!("{BASE_URL}/login");
        let resp = self
            .http
            .post(&url)
            .json(&json!({ "apikey": self.api_key }))
            .send()
            .await
            .context("tvdb login request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("tvdb /login returned {status}: {body}"));
        }
        let env: Envelope<LoginData> = resp.json().await.context("tvdb login parse")?;
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
                .get(&url)
                .bearer_auth(&jwt)
                .send()
                .await
                .context("tvdb get request")?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                debug!("tvdb: 401 — refreshing JWT and retrying once");
                self.clear_jwt().await;
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!("tvdb {path} returned {status}: {body}"));
            }
            let env: Envelope<T> = resp.json().await.context("tvdb parse envelope")?;
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
    pub async fn search(&self, query: &str, kind: SearchKind, limit: u32) -> Result<Vec<TvdbEntry>> {
        let qs = format!(
            "/search?query={}&type={}&limit={}",
            urlencoding::encode(query),
            kind.as_str(),
            limit,
        );
        let items: Vec<SearchItem> = self.get(&qs).await?;
        Ok(items.into_iter().filter_map(|i| flatten(i, kind)).collect())
    }

    /// Enrichment by IMDB id (preferred — precise) or by title fallback.
    /// Returns None when TVDB has no match or the lookup fails — callers
    /// should treat this as "no enrichment available" and move on.
    pub async fn enrich_by_imdb(&self, imdb_id: &str) -> Option<EnrichedFields> {
        let path = format!("/search/remoteid/{}", urlencoding::encode(imdb_id));
        match self.get::<Vec<SearchItem>>(&path).await {
            Ok(items) => items.into_iter().next().map(|s| item_to_enriched(s)),
            Err(e) => {
                warn!(imdb_id = %imdb_id, err = %e, "tvdb enrich_by_imdb failed");
                None
            }
        }
    }

    /// Enrichment by title + optional year. Picks the top search result as
    /// the best match. Less precise than remoteid lookup — use only when
    /// IMDB/TMDB ids aren't known.
    pub async fn enrich_by_title(
        &self,
        title: &str,
        year: Option<&str>,
        kind: SearchKind,
    ) -> Option<EnrichedFields> {
        let mut query = title.to_string();
        if let Some(y) = year {
            query.push(' ');
            query.push_str(y);
        }
        match self.search(&query, kind, 1).await {
            Ok(mut v) => v.pop().map(|e| TvdbEntry_to_enriched(e)),
            Err(e) => {
                warn!(title = %title, err = %e, "tvdb enrich_by_title failed");
                None
            }
        }
    }
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

fn item_to_enriched(item: SearchItem) -> EnrichedFields {
    let (imdb_id, _) = extract_remote_ids(&item.remote_ids);
    EnrichedFields {
        overview: item.overview,
        poster_url: item.image_url.or(item.thumbnail),
        year: item.year,
        genres: item.genres,
        imdb_id,
        rating: None,
    }
}

#[allow(non_snake_case)]
fn TvdbEntry_to_enriched(e: TvdbEntry) -> EnrichedFields {
    EnrichedFields {
        overview: e.overview,
        poster_url: e.image_url,
        year: e.year,
        genres: e.genres,
        imdb_id: e.imdb_id,
        rating: None,
    }
}

