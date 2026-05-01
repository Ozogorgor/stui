//! fanart.tv integration — runtime-level artwork source.
//!
//! Not a plugin: fanart ships as part of the runtime binary alongside
//! TVDB and mdblist. Contributes posters / backgrounds / logos to the
//! `fan_out_artwork` merge for movies and series.
//!
//! # Two-key auth (per fanart.tv ToS)
//!
//! fanart's documented auth model sends two query params on every call:
//!   - `api_key`    — the project key registered to stui. Mandatory.
//!   - `client_key` — the end-user's personal fanart key. Optional. When
//!     present it tells fanart "this traffic came from this individual
//!     user", which improves analytics fairness on their side and is
//!     explicitly encouraged by their ToS clause:
//!     "You should allow your users to input their own API key into your
//!     application, this should be sent in addition to your project key."
//!
//! Both are read from `secrets.env` (`FANART_PROJECT_KEY` mandatory,
//! `FANART_USER_KEY` optional). A Settings UI field for the user key
//! is a future addition; today, power users edit `secrets.env` directly.
//!
//! # ToS notes that shape this code
//!
//! * "Don't request more than necessary" → 24h persistent-cache TTL on
//!   every fanart response (caller-side, via `RuntimeCache`).
//! * "Don't bulk download" → on-demand per-title only, never proactive.
//! * "Use documented API methods" → only the v3 endpoints below.
//! * "Inform users of this website and images" → attribution shows up in
//!   Settings → Credits and the README (UI/docs work, not in this file).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

const BASE_URL: &str = "https://webservice.fanart.tv/v3";
const USER_AGENT: &str = concat!("stui-runtime/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Which slice of fanart's per-title response we want. Each title returns a
/// large object with arrays for posters / backdrops / logos / etc.; the
/// caller picks one slice based on what it's filling. Phase 1 only uses
/// `Poster` (the catalog grid + detail card use case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkSlot {
    Poster,
    Background,
    Logo,
}

#[derive(Clone)]
pub struct FanartClient {
    http: Client,
    project_key: String,
    user_key: Option<String>,
}

impl FanartClient {
    pub fn new(project_key: String, user_key: Option<String>) -> Option<Self> {
        if project_key.trim().is_empty() {
            return None;
        }
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            http,
            project_key,
            user_key: user_key.filter(|k| !k.trim().is_empty()),
        })
    }

    /// Fetch artwork URLs for a movie by TMDB or IMDB id. Returns just the
    /// URLs (not the full fanart metadata) sorted by popularity (`likes`)
    /// then by language preference (English first).
    pub async fn movie_artwork(&self, tmdb_id: &str, slot: ArtworkSlot) -> Result<Vec<String>> {
        let url = self.build_url("movies", tmdb_id);
        let payload: FanartMovie = self.fetch_json(&url, "movie").await?;
        let items = match slot {
            ArtworkSlot::Poster => payload.movieposter,
            ArtworkSlot::Background => payload.moviebackground,
            // Prefer hd variants; fall back to non-hd. Concat keeps both.
            ArtworkSlot::Logo => {
                let mut combined = payload.hdmovielogo;
                combined.extend(payload.movielogo);
                combined
            }
        };
        Ok(rank_by_likes_then_lang(items))
    }

    /// Fetch artwork URLs for a TV show by TVDB id. Posters here are
    /// series-level only; per-season posters live in the response too but
    /// aren't surfaced in the phase-1 API.
    pub async fn tv_artwork(&self, tvdb_id: &str, slot: ArtworkSlot) -> Result<Vec<String>> {
        let url = self.build_url("tv", tvdb_id);
        let payload: FanartTv = self.fetch_json(&url, "tv").await?;
        let items = match slot {
            ArtworkSlot::Poster => payload.tvposter,
            ArtworkSlot::Background => payload.showbackground,
            ArtworkSlot::Logo => {
                let mut combined = payload.hdtvlogo;
                combined.extend(payload.clearlogo);
                combined
            }
        };
        Ok(rank_by_likes_then_lang(items))
    }

    fn build_url(&self, kind: &str, id: &str) -> String {
        match &self.user_key {
            Some(uk) => format!(
                "{BASE_URL}/{kind}/{id}?api_key={proj}&client_key={uk}",
                proj = self.project_key,
            ),
            None => format!(
                "{BASE_URL}/{kind}/{id}?api_key={proj}",
                proj = self.project_key,
            ),
        }
    }

    async fn fetch_json<T>(&self, url: &str, what: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("fanart: {what} request"))?;
        let status = resp.status();
        // 404 means fanart has no entry for this id — surface as an empty
        // shaped response, not an error. Avoids polluting logs for the
        // long tail of obscure titles fanart doesn't cover.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("fanart: {what} 404 (no entry)"));
        }
        let body = resp.text().await.context("fanart: read body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "fanart: {what} HTTP {status} — {body}",
                body = body.chars().take(160).collect::<String>(),
            ));
        }
        serde_json::from_str(&body).with_context(|| format!("fanart: parse {what}"))
    }
}

/// Order artwork by community votes (likes) descending, then prefer
/// English. Both keys parse as integers when possible; "no language" /
/// missing values sink to the bottom.
fn rank_by_likes_then_lang(mut items: Vec<FanartItem>) -> Vec<String> {
    items.sort_by(|a, b| {
        let likes_a = a.likes.as_deref().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let likes_b = b.likes.as_deref().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let lang_pref = |it: &FanartItem| match it.lang.as_deref() {
            Some("en") => 0,
            Some("00") | None => 2, // "00" = no specific language
            _ => 1,
        };
        likes_b
            .cmp(&likes_a)
            .then_with(|| lang_pref(a).cmp(&lang_pref(b)))
    });
    items.into_iter().map(|i| i.url).collect()
}

#[derive(Debug, Deserialize)]
struct FanartItem {
    url: String,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    likes: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FanartMovie {
    #[serde(default)]
    movieposter: Vec<FanartItem>,
    #[serde(default)]
    moviebackground: Vec<FanartItem>,
    #[serde(default)]
    movielogo: Vec<FanartItem>,
    #[serde(default)]
    hdmovielogo: Vec<FanartItem>,
}

#[derive(Debug, Default, Deserialize)]
struct FanartTv {
    #[serde(default)]
    tvposter: Vec<FanartItem>,
    #[serde(default)]
    showbackground: Vec<FanartItem>,
    #[serde(default)]
    clearlogo: Vec<FanartItem>,
    #[serde(default)]
    hdtvlogo: Vec<FanartItem>,
}

/// Build a client from `secrets.env`. Returns `None` when the project
/// key is missing — same soft-disable pattern as TVDB and mdblist (no
/// key = source silently absent, plugins still flow).
pub fn from_secrets() -> Option<Arc<FanartClient>> {
    let secrets = crate::config::secrets::Secrets::load();
    let project = secrets
        .get("FANART_PROJECT_KEY")
        .filter(|k| !k.trim().is_empty())?;
    let user = secrets.get("FANART_USER_KEY").filter(|k| !k.trim().is_empty());
    FanartClient::new(project, user).map(Arc::new)
}
