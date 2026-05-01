//! Elfhosted Stremio rating-aggregator addon — runtime-level details enricher.
//!
//! Hits a single fixed endpoint per IMDb id and returns the addon's
//! pre-formatted multi-line ratings block (IMDb / TMDb / Metacritic /
//! Rotten Tomatoes / Common Sense age rating / CringeMDB parent-safe
//! flag) which the TUI renders verbatim in the detail screen.
//!
//! Why runtime-native, not a plugin:
//! * single source, no key, no user config — there is nothing to expose
//!   through the plugin manifest.
//! * single-shot per detail open with a 24h cache, doesn't fit the
//!   per-verb fan-out / merge / source-list shape that justifies the
//!   plugin host.
//!
//! Why we don't parse the description into per-source values:
//! * the addon's own format is presentation, not data — we'd be
//!   regex-matching emoji prefixes for IMDb / TMDb scores we already
//!   collect from first-party sources via OMDb / TMDB. The genuinely new
//!   signals (parent-safe, age rating) can be lifted out as structured
//!   chips later; v1 just renders the block as-is.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

const BASE_URL: &str = "https://rating-aggregator.elfhosted.com";
const USER_AGENT: &str = concat!("stui-runtime/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct RatingAggregatorClient {
    http: Client,
}

impl RatingAggregatorClient {
    pub fn new() -> Option<Self> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self { http })
    }

    /// Fetch the formatted ratings block + `imdb.com` external URL for an
    /// IMDb id. `kind` is `"movie"` or `"series"` per the addon's manifest.
    /// Returns `None` when the addon has no entry for this id (no streams
    /// in the response) — distinct from network errors which propagate up.
    pub async fn fetch(
        &self,
        imdb_id: &str,
        kind: &str,
    ) -> Result<Option<RatingsAggregatorBlock>> {
        let url = format!("{BASE_URL}/stream/{kind}/{imdb_id}.json");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("rating_aggregator: GET {url}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = resp.text().await.context("rating_aggregator: read body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "rating_aggregator: HTTP {status} — {snippet}",
                snippet = body.chars().take(160).collect::<String>(),
            ));
        }
        let parsed: StreamResponse =
            serde_json::from_str(&body).context("rating_aggregator: parse body")?;
        // The addon emits exactly one stream per id when it has data, none
        // otherwise. Take the first non-empty description we see.
        let entry = parsed
            .streams
            .into_iter()
            .find(|s| !s.description.trim().is_empty());
        Ok(entry.map(|s| RatingsAggregatorBlock {
            description: s.description,
            external_url: s.external_url,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct RatingsAggregatorBlock {
    pub description: String,
    pub external_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    #[serde(default)]
    streams: Vec<StreamEntry>,
}

#[derive(Debug, Deserialize)]
struct StreamEntry {
    #[serde(default)]
    description: String,
    #[serde(rename = "externalUrl", default)]
    external_url: Option<String>,
}
