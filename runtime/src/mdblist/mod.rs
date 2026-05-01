//! mdblist.com integration — runtime-level catalog source.
//!
//! Not a plugin: mdblist ships as part of the runtime binary so the catalog
//! grids (Movies / Series tabs) seed from a curated, ID-rich list instead of
//! TMDB's trending fan-out. Each mdblist list item carries `imdb_id`,
//! `tmdb_id`, and `tvdb_id` already, which lets downstream per-card
//! enrichment (TMDB / OMDB / TVDB) hit each provider with its native id —
//! no OMDB-by-title backfill, no TMDB-id-only OMDB calls.
//!
//! # API surface used today
//!
//! Only `GET https://api.mdblist.com/lists/{username}/{slug}/items?apikey=…`.
//! Response shape (with empty buckets omitted by the server, defaulted to
//! empty Vec by serde):
//!
//! ```json
//! { "movies": [ {item}, … ], "shows": [ {item}, … ] }
//! ```
//!
//! Each item has `ids.imdb / ids.tmdb / ids.tvdb`, `title`, `release_year`,
//! `mediatype`, `runtime`, plus a few language/region fields we don't yet
//! surface. No poster, overview, or rating — that's the lazy-enrichment
//! windfall: catalog rows are sparse, the existing per-card TMDB enrich
//! (with the bundle-cache from 2026-04-30) fills in visuals only on demand.
//!
//! # Authentication
//!
//! v1 reads the API key from `MDBLIST_API_KEY` (env or `secrets.env`). No
//! build.rs embedding yet — mirrors the TMDB / OMDB user-supplied-key
//! pattern. Switch to TVDB-style XOR-obfuscated embedding later if/when
//! stui ships binaries to other users.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

const BASE_URL: &str = "https://api.mdblist.com";
const USER_AGENT: &str = concat!("stui-runtime/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Default list slugs for the catalog grid. Configurable via
/// `runtime.toml [mdblist] movies_list / series_list` (added in
/// `config::types`). Changing the list at runtime requires a restart for
/// now — the catalog cache keys by slug so a stale list won't shadow the
/// new one once invalidated.
pub const DEFAULT_MOVIES_LIST: &str = "snoak/latest-movies-digital-release";
pub const DEFAULT_SERIES_LIST: &str = "garycrawfordgc/latest-tv-shows";

/// What flavour of items to keep from a list response. mdblist returns
/// `{movies, shows}` regardless of which list you query — most lists are
/// pure (one bucket populated, the other empty) but some mix both. The
/// caller picks a kind so dedupe downstream doesn't see TV in a movies
/// catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Movies,
    Shows,
}

#[derive(Clone)]
pub struct MdblistClient {
    http: Client,
    api_key: String,
}

impl MdblistClient {
    /// Build a client from an API key. Returns `None` if the key is empty
    /// (matches the TVDB pattern: missing key = source silently disabled,
    /// not a hard failure).
    pub fn new(api_key: String) -> Option<Self> {
        if api_key.trim().is_empty() {
            return None;
        }
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self { http, api_key })
    }

    /// Fetch the items in a public mdblist list. `slug` is the
    /// `username/list-slug` form (e.g. `"snoak/latest-movies-digital-release"`).
    /// Returns the bucket selected by `kind`; the other bucket is dropped.
    pub async fn fetch_list(&self, slug: &str, kind: ListKind) -> Result<Vec<MdblistItem>> {
        let url = format!("{BASE_URL}/lists/{slug}/items?apikey={key}", key = &self.api_key);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("mdblist: fetch list {slug}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .context("mdblist: read response body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "mdblist: list {slug} returned HTTP {status} — {body}",
                body = body.chars().take(160).collect::<String>(),
            ));
        }
        let parsed: MdblistListResponse =
            serde_json::from_str(&body).with_context(|| format!("mdblist: parse list {slug}"))?;
        Ok(match kind {
            ListKind::Movies => parsed.movies,
            ListKind::Shows => parsed.shows,
        })
    }
}

/// Wire-shape of a list response. Both buckets default to empty — many
/// lists are single-kind and the absent bucket is just elided server-side.
#[derive(Debug, Deserialize)]
struct MdblistListResponse {
    #[serde(default)]
    movies: Vec<MdblistItem>,
    #[serde(default)]
    shows: Vec<MdblistItem>,
}

/// One row of an mdblist list.
///
/// Only fields we actually use are deserialised. Unrecognised fields are
/// dropped (default serde behaviour). The flat-level `imdb_id` / `tvdb_id`
/// fields duplicate `ids.imdb` / `ids.tvdb`; we read from `ids` because
/// it's the canonical place per mdblist's docs. TMDB id only lives in
/// `ids.tmdb` (no flat-level mirror in their schema).
#[derive(Debug, Clone, Deserialize)]
pub struct MdblistItem {
    pub title: String,
    #[serde(default)]
    pub release_year: Option<u32>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub mediatype: Option<String>,
    #[serde(default)]
    pub runtime: Option<u32>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub adult: u8,
    #[serde(default)]
    pub ids: MdblistIds,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MdblistIds {
    #[serde(default)]
    pub imdb: Option<String>,
    /// TMDB ids come back as JSON numbers. Stringify here so downstream
    /// `CatalogEntry.tmdb_id: Option<String>` gets the canonical form.
    #[serde(default, deserialize_with = "stringify_number_opt")]
    pub tmdb: Option<String>,
    #[serde(default, deserialize_with = "stringify_number_opt")]
    pub tvdb: Option<String>,
}

fn stringify_number_opt<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::IgnoredAny;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum E {
        N(u64),
        S(String),
        Other(IgnoredAny),
    }
    Ok(match E::deserialize(d)? {
        E::N(n) => Some(n.to_string()),
        E::S(s) if !s.is_empty() => Some(s),
        _ => None,
    })
}

/// Build a client from the loaded secrets. Returns `None` when
/// `MDBLIST_API_KEY` is missing/empty, matching the TVDB pattern (no key
/// = source silently disabled, plugins still flow normally).
pub fn from_secrets() -> Option<Arc<MdblistClient>> {
    let key = crate::config::secrets::Secrets::load()
        .get("MDBLIST_API_KEY")
        .filter(|k| !k.trim().is_empty())?;
    MdblistClient::new(key).map(Arc::new)
}
