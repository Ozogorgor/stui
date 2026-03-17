//! Last.fm provider — music catalog via the Last.fm REST API.
//!
//! Docs: https://www.last.fm/api
//!
//! API key required — obtain one at https://www.last.fm/api/account/create
//! Set via `api_keys.lastfm` in stui.toml (or via the plugin settings TUI).
//! Environment variable `LASTFM_API_KEY` is also accepted as a fallback.
//!
//! Endpoints used:
//!   chart.getTopArtists  → trending artists
//!   artist.search        → artist search
//!   album.search         → album search (fan out with artist.search)
//!
//! Tabs served: `Music`.

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, MediaType};
use crate::providers::Provider;

const BASE_URL: &str = "https://ws.audioscrobbler.com/2.0/";

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TopArtistsPage {
    artists: ArtistList,
}

#[derive(Debug, Deserialize)]
struct ArtistList {
    artist: Vec<LfmArtist>,
}

#[derive(Debug, Deserialize)]
struct ArtistSearchPage {
    results: ArtistSearchResults,
}

#[derive(Debug, Deserialize)]
struct ArtistSearchResults {
    #[serde(rename = "artistmatches")]
    matches: ArtistMatches,
}

#[derive(Debug, Deserialize)]
struct ArtistMatches {
    artist: Vec<LfmArtist>,
}

#[derive(Debug, Deserialize)]
struct LfmArtist {
    name:      String,
    mbid:      Option<String>,
    #[serde(default)]
    listeners: String,
    image:     Option<Vec<LfmImage>>,
}

#[derive(Debug, Deserialize)]
struct LfmImage {
    #[serde(rename = "#text")]
    url:  String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct AlbumSearchPage {
    results: AlbumSearchResults,
}

#[derive(Debug, Deserialize)]
struct AlbumSearchResults {
    #[serde(rename = "albummatches")]
    matches: AlbumMatches,
}

#[derive(Debug, Deserialize)]
struct AlbumMatches {
    album: Vec<LfmAlbum>,
}

#[derive(Debug, Deserialize)]
struct LfmAlbum {
    name:   String,
    artist: String,
    mbid:   Option<String>,
    image:  Option<Vec<LfmImage>>,
}

// ── Provider ──────────────────────────────────────────────────────────────────

pub struct LastFmProvider {
    api_key: String,
    client:  Client,
}

impl LastFmProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        LastFmProvider {
            api_key: api_key.into(),
            client: Client::builder()
                .user_agent(concat!("stui/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let key = std::env::var("LASTFM_API_KEY").ok()?;
        if key.is_empty() { return None; }
        Some(Self::new(key))
    }

    pub fn from_config(api_keys: &crate::config::types::ApiKeysConfig) -> Option<Self> {
        let key = api_keys.lastfm.clone()
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("LASTFM_API_KEY").ok().filter(|k| !k.is_empty()))?;
        Some(Self::new(key))
    }

    fn artist_poster(images: &Option<Vec<LfmImage>>) -> Option<String> {
        let images = images.as_deref()?;
        // Last.fm image sizes: small, medium, large, extralarge, mega
        for size in &["extralarge", "large", "mega"] {
            if let Some(img) = images.iter().find(|i| &i.size == size) {
                if !img.url.is_empty() {
                    return Some(img.url.clone());
                }
            }
        }
        None
    }

    fn artist_to_entry(&self, a: LfmArtist) -> CatalogEntry {
        let id = if let Some(ref mbid) = a.mbid {
            if mbid.is_empty() {
                format!("lfm-artist-{}", urlencoding::encode(&a.name))
            } else {
                format!("lfm-artist-{}", mbid)
            }
        } else {
            format!("lfm-artist-{}", urlencoding::encode(&a.name))
        };

        // listeners is a formatted number string like "3,456,789"
        let description = if !a.listeners.is_empty() {
            Some(format!("{} listeners", a.listeners))
        } else {
            None
        };

        CatalogEntry {
            id,
            title:       a.name,
            year:        None,
            genre:       None,
            rating:      None,
            description,
            poster_url:  Self::artist_poster(&a.image),
            poster_art:  None,
            provider:    "lastfm".to_string(),
            tab:         "music".to_string(),
            imdb_id:     None,
            tmdb_id:     None,
            media_type:  MediaType::Music,
            ratings:     std::collections::HashMap::new(),
        }
    }

    fn album_to_entry(&self, al: LfmAlbum) -> CatalogEntry {
        let id = if let Some(ref mbid) = al.mbid {
            if mbid.is_empty() {
                format!("lfm-album-{}-{}", urlencoding::encode(&al.artist), urlencoding::encode(&al.name))
            } else {
                format!("lfm-album-{}", mbid)
            }
        } else {
            format!("lfm-album-{}-{}", urlencoding::encode(&al.artist), urlencoding::encode(&al.name))
        };

        let title = format!("{} — {}", al.name, al.artist);

        CatalogEntry {
            id,
            title,
            year:        None,
            genre:       None,
            rating:      None,
            description: None,
            poster_url:  Self::artist_poster(&al.image),
            poster_art:  None,
            provider:    "lastfm".to_string(),
            tab:         "music".to_string(),
            imdb_id:     None,
            tmdb_id:     None,
            media_type:  MediaType::Album,
            ratings:     std::collections::HashMap::new(),
        }
    }
}

#[async_trait]
impl Provider for LastFmProvider {
    fn name(&self) -> &str { "lastfm" }
    fn display_name(&self) -> &str { "Last.fm" }
    fn description(&self) -> &str { "Last.fm — music catalog, trending artists & albums" }

    fn config_schema(&self) -> Vec<crate::ipc::ProviderField> {
        vec![crate::ipc::ProviderField {
            key:        "api_keys.lastfm".to_string(),
            label:      "API Key".to_string(),
            hint:       "Free at last.fm/api/account/create".to_string(),
            masked:     true,
            configured: !self.api_key.is_empty(),
        }]
    }

    fn is_active(&self) -> bool { !self.api_key.is_empty() }

    fn supported_tabs(&self) -> Option<Vec<MediaTab>> {
        Some(vec![MediaTab::Music])
    }

    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        if !matches!(tab, MediaTab::Music) {
            return Ok(vec![]);
        }

        let url = format!(
            "{BASE_URL}?method=chart.getTopArtists&api_key={}&format=json&page={}&limit=20",
            self.api_key, page
        );
        debug!(provider = "lastfm", page, "fetching top artists");

        let resp: TopArtistsPage = match self.client.get(&url).send().await?.json().await {
            Ok(r)  => r,
            Err(e) => {
                warn!(provider = "lastfm", error = %e, "trending parse failed");
                return Ok(vec![]);
            }
        };

        Ok(resp.artists.artist.into_iter().map(|a| self.artist_to_entry(a)).collect())
    }

    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        if !matches!(tab, MediaTab::Music) {
            return Ok(vec![]);
        }

        debug!(provider = "lastfm", q = query, page, "searching");

        let q = urlencoding::encode(query);

        let artist_url = format!(
            "{BASE_URL}?method=artist.search&artist={q}&api_key={}&format=json&page={}&limit=12",
            self.api_key, page
        );
        let album_url = format!(
            "{BASE_URL}?method=album.search&album={q}&api_key={}&format=json&page={}&limit=12",
            self.api_key, page
        );

        let (artists_resp, albums_resp) = tokio::join!(
            self.client.get(&artist_url).send(),
            self.client.get(&album_url).send(),
        );

        let mut entries = vec![];

        match artists_resp {
            Ok(r) => match r.json::<ArtistSearchPage>().await {
                Ok(body) => {
                    for a in body.results.matches.artist {
                        entries.push(self.artist_to_entry(a));
                    }
                }
                Err(e) => warn!(provider = "lastfm", error = %e, "artist parse failed"),
            },
            Err(e) => warn!(provider = "lastfm", error = %e, "artist request failed"),
        }

        match albums_resp {
            Ok(r) => match r.json::<AlbumSearchPage>().await {
                Ok(body) => {
                    for al in body.results.matches.album {
                        entries.push(self.album_to_entry(al));
                    }
                }
                Err(e) => warn!(provider = "lastfm", error = %e, "album parse failed"),
            },
            Err(e) => warn!(provider = "lastfm", error = %e, "album request failed"),
        }

        Ok(entries)
    }
}
