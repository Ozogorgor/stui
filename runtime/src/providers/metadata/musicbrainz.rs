//! MusicBrainz provider — music catalog via the MusicBrainz JSON API.
//!
//! Docs: https://musicbrainz.org/doc/MusicBrainz_API
//!
//! No API key required. Requires a descriptive `User-Agent` header per
//! MusicBrainz policy (anonymous requests are rate-limited hard at ~1 req/s;
//! named clients get a more generous 1 req/s sustained with burst allowance).
//!
//! Tabs served: `Music`.
//!
//! Trending strategy: MusicBrainz has no charts endpoint. We seed the
//! trending view with a curated list of cross-genre artists and return their
//! MusicBrainz entries. For the Music tab, search is the primary use case.

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, MediaType};
use crate::providers::Provider;

const BASE_URL: &str = "https://musicbrainz.org/ws/2";

/// Seed artist names shown in the Music trending view.
/// Deliberately genre-diverse so users get a taste of what MusicBrainz covers.
const SEED_ARTISTS: &[&str] = &[
    "The Beatles", "David Bowie", "Kendrick Lamar", "Radiohead",
    "Björk", "Miles Davis", "Nina Simone", "Daft Punk",
    "Portishead", "Frank Ocean", "Massive Attack", "Aphex Twin",
    "Joni Mitchell", "John Coltrane", "Boards of Canada",
];

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ArtistSearch {
    artists: Vec<MbArtist>,
}

#[derive(Debug, Deserialize)]
struct MbArtist {
    id:            String,
    name:          String,
    /// Artist disambiguation (e.g. "UK rock band" vs "US country singer")
    disambiguation: Option<String>,
    /// Most common tags (genres) attached to this artist.
    tags:          Option<Vec<MbTag>>,
    /// Life span (formed/dissolved years).
    #[serde(rename = "life-span")]
    life_span:     Option<LifeSpan>,
}

#[derive(Debug, Deserialize)]
struct MbTag {
    name:  String,
    count: u32,
}

#[derive(Debug, Deserialize)]
struct LifeSpan {
    begin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseSearch {
    releases: Vec<MbRelease>,
}

#[derive(Debug, Deserialize)]
struct MbRelease {
    id:      String,
    title:   String,
    date:    Option<String>,
    #[serde(rename = "artist-credit")]
    credits: Option<Vec<ArtistCredit>>,
}

#[derive(Debug, Deserialize)]
struct ArtistCredit {
    artist: Option<MbCreditArtist>,
}

#[derive(Debug, Deserialize)]
struct MbCreditArtist {
    name: String,
}

// ── Provider ──────────────────────────────────────────────────────────────────

pub struct MusicBrainzProvider {
    client: Client,
}

impl MusicBrainzProvider {
    pub fn new() -> Self {
        // MusicBrainz policy: User-Agent must identify the application and
        // include a contact URL or email. Anonymous clients are blocked.
        let client = Client::builder()
            .user_agent(concat!(
                "stui/", env!("CARGO_PKG_VERSION"),
                " (https://github.com/stui/stui)"
            ))
            .build()
            .unwrap_or_default();

        MusicBrainzProvider { client }
    }

    fn artist_to_entry(&self, a: MbArtist) -> CatalogEntry {
        // Pick the highest-count tag as genre.
        let genre = a.tags
            .as_deref()
            .and_then(|tags| tags.iter().max_by_key(|t| t.count))
            .map(|t| t.name.clone());

        // Start year from life-span.begin (e.g. "1960" or "1960-01-01").
        let year = a.life_span
            .and_then(|ls| ls.begin)
            .map(|b| b[..4.min(b.len())].to_string());

        let description = a.disambiguation.clone();

        CatalogEntry {
            id:          format!("mb-artist-{}", a.id),
            title:       a.name,
            year,
            genre,
            rating:      None,
            description,
            poster_url:  None, // MusicBrainz has no images; CAA is a separate service
            poster_art:  None,
            provider:    "musicbrainz".to_string(),
            tab:         "music".to_string(),
            imdb_id:     None,
            tmdb_id:     None,
            media_type:  MediaType::Music,
            ratings:     std::collections::HashMap::new(),
        }
    }

    fn release_to_entry(&self, r: MbRelease) -> CatalogEntry {
        let artist = r.credits
            .as_deref()
            .and_then(|c| c.first())
            .and_then(|c| c.artist.as_ref())
            .map(|a| a.name.clone());

        let year = r.date
            .as_deref()
            .map(|d| d[..4.min(d.len())].to_string());

        // Show "Title — Artist" so the card is self-contained.
        let title = if let Some(ref a) = artist {
            format!("{} — {}", r.title, a)
        } else {
            r.title.clone()
        };

        CatalogEntry {
            id:          format!("mb-release-{}", r.id),
            title,
            year,
            genre:       None,
            rating:      None,
            description: None,
            poster_url:  None,
            poster_art:  None,
            provider:    "musicbrainz".to_string(),
            tab:         "music".to_string(),
            imdb_id:     None,
            tmdb_id:     None,
            media_type:  MediaType::Album,
            ratings:     std::collections::HashMap::new(),
        }
    }

    /// Fetch a single artist by name; returns the best-scoring result or None.
    async fn fetch_artist(&self, name: &str) -> Option<CatalogEntry> {
        let q   = urlencoding::encode(name);
        let url = format!("{BASE_URL}/artist?query={q}&limit=1&fmt=json");

        let resp: ArtistSearch = self.client.get(&url).send().await.ok()?.json().await.ok()?;
        resp.artists.into_iter().next().map(|a| self.artist_to_entry(a))
    }
}

impl Default for MusicBrainzProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Provider for MusicBrainzProvider {
    fn name(&self) -> &str { "musicbrainz" }
    fn display_name(&self) -> &str { "MusicBrainz" }
    fn description(&self) -> &str { "MusicBrainz — open music encyclopedia, no API key needed" }

    fn supported_tabs(&self) -> Option<Vec<MediaTab>> {
        Some(vec![MediaTab::Music])
    }

    /// Trending: resolve the seed artist list, one by one, and return results.
    ///
    /// We paginate through the seed list rather than making 15 requests up
    /// front — only the artists needed for the requested page are fetched.
    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        if !matches!(tab, MediaTab::Music) {
            return Ok(vec![]);
        }

        let per_page: usize  = 8;
        let start:    usize  = ((page.saturating_sub(1)) as usize) * per_page;
        let seeds            = &SEED_ARTISTS[start.min(SEED_ARTISTS.len())..
                                             (start + per_page).min(SEED_ARTISTS.len())];

        if seeds.is_empty() {
            return Ok(vec![]);
        }

        debug!(provider = "musicbrainz", count = seeds.len(), "fetching seed artists");

        let mut entries = Vec::with_capacity(seeds.len());
        for seed in seeds {
            if let Some(entry) = self.fetch_artist(seed).await {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Search: query artists first, then releases — merge and return.
    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        if !matches!(tab, MediaTab::Music) {
            return Ok(vec![]);
        }

        debug!(provider = "musicbrainz", q = query, page, "searching");

        let q = urlencoding::encode(query);

        // Artist search
        let artist_url  = format!("{BASE_URL}/artist?query={q}&page={page}&limit=12&fmt=json");
        // Release search
        let release_url = format!("{BASE_URL}/release?query={q}&page={page}&limit=12&fmt=json");

        let (artists_resp, releases_resp) = tokio::join!(
            self.client.get(&artist_url).send(),
            self.client.get(&release_url).send(),
        );

        let mut entries = vec![];

        match artists_resp {
            Ok(r) => match r.json::<ArtistSearch>().await {
                Ok(body) => {
                    for a in body.artists {
                        entries.push(self.artist_to_entry(a));
                    }
                }
                Err(e) => warn!(provider = "musicbrainz", error = %e, "artist parse failed"),
            },
            Err(e) => warn!(provider = "musicbrainz", error = %e, "artist request failed"),
        }

        match releases_resp {
            Ok(r) => match r.json::<ReleaseSearch>().await {
                Ok(body) => {
                    for rel in body.releases {
                        entries.push(self.release_to_entry(rel));
                    }
                }
                Err(e) => warn!(provider = "musicbrainz", error = %e, "release parse failed"),
            },
            Err(e) => warn!(provider = "musicbrainz", error = %e, "release request failed"),
        }

        Ok(entries)
    }
}
