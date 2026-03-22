//! Stremio addon adapter — wraps a `StremioClient` as a `dyn Provider`.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, MediaType, SubtitleTrack};
use crate::providers::{Provider, Stream, StreamQuality};
use super::client::StremioClient;
use super::manifest::StremioManifest;

#[allow(dead_code)]
pub struct StremioAddon {
    client:   StremioClient,
    manifest: StremioManifest,
}

impl StremioAddon {
    /// Fetch the manifest and build the adapter.
    pub async fn from_url(manifest_url: &str) -> Result<Self> {
        let client = StremioClient::from_manifest_url(manifest_url);
        let manifest = client.fetch_manifest().await?;
        info!(
            "stremio: loaded addon '{}' v{} from {}",
            manifest.name, manifest.version, manifest_url
        );
        Ok(StremioAddon { client, manifest })
    }

    /// Load all addons from the `STUI_STREMIO_ADDONS` env var.
    pub async fn from_env() -> Vec<Self> {
        let urls = match std::env::var("STUI_STREMIO_ADDONS") {
            Ok(v) if !v.is_empty() => v,
            _ => return vec![],
        };

        let mut addons = vec![];
        for url in urls.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match StremioAddon::from_url(url).await {
                Ok(a)  => addons.push(a),
                Err(e) => warn!("stremio: failed to load addon {url}: {e}"),
            }
        }
        addons
    }

    #[allow(dead_code)]
    fn stremio_type_for_tab(tab: &MediaTab) -> &'static str {
        match tab {
            MediaTab::Movies   => "movie",
            MediaTab::Series   => "series",
            MediaTab::Music    => "other",
            MediaTab::Library  => "other",
            MediaTab::Radio | MediaTab::Podcasts | MediaTab::Videos => "other",
        }
    }

    #[allow(dead_code)]
    fn supports_type(&self, t: &str) -> bool {
        self.manifest.types.iter().any(|mt| mt == t)
    }
}

#[async_trait]
impl Provider for StremioAddon {
    fn name(&self) -> &str { &self.manifest.name }

    fn has_streams(&self) -> bool {
        self.manifest.resources.iter().any(|r| {
            r.as_str() == Some("stream")
                || r.get("name").and_then(|n| n.as_str()) == Some("stream")
        })
    }

    fn has_subtitles(&self) -> bool {
        self.manifest.resources.iter().any(|r| {
            r.as_str() == Some("subtitles")
                || r.get("name").and_then(|n| n.as_str()) == Some("subtitles")
        })
    }

    async fn fetch_trending(&self, tab: &MediaTab, _page: u32) -> Result<Vec<CatalogEntry>> {
        let stype = Self::stremio_type_for_tab(tab);
        if !self.supports_type(stype) { return Ok(vec![]); }

        // Use the first catalog that matches this type
        let catalog = self.manifest.catalogs.iter()
            .find(|c| c.r#type == stype);

        let Some(cat) = catalog else { return Ok(vec![]); };

        let resp = self.client.catalog(stype, &cat.id, &[]).await?;

        Ok(resp.metas.into_iter().map(|m| meta_to_entry(m, tab, self.name())).collect())
    }

    async fn search(&self, tab: &MediaTab, query: &str, _page: u32) -> Result<Vec<CatalogEntry>> {
        let stype = Self::stremio_type_for_tab(tab);
        if !self.supports_type(stype) { return Ok(vec![]); }

        // Find a catalog that supports the "search" extra
        let catalog = self.manifest.catalogs.iter().find(|c| {
            c.r#type == stype
                && c.extra.iter().any(|e| e.name == "search")
        });

        let Some(cat) = catalog else { return Ok(vec![]); };

        let resp = self.client.catalog(stype, &cat.id, &[("search", query)]).await?;

        Ok(resp.metas.into_iter().map(|m| meta_to_entry(m, tab, self.name())).collect())
    }

    async fn streams(&self, id: &str) -> Result<Vec<Stream>> {
        // id is expected to be an IMDB id like "tt0816692" or "tt0816692:1:1"
        // Determine type from id format
        let media_type = if id.contains(':') { "series" } else { "movie" };

        let resp = self.client.streams(media_type, id).await?;

        Ok(resp.streams.into_iter().filter_map(|s| stremio_stream_to_stream(s, self.name())).collect())
    }

    async fn subtitles(&self, id: &str) -> Result<Vec<SubtitleTrack>> {
        let media_type = if id.contains(':') { "series" } else { "movie" };
        let resp = self.client.subtitles(media_type, id).await?;
        Ok(resp.subtitles.into_iter().map(|s| SubtitleTrack {
            language: s.lang,
            url:      s.url,
            format:   "srt".into(),
        }).collect())
    }
}

// ── Conversion helpers ────────────────────────────────────────────────────────

#[allow(dead_code)]
fn meta_to_entry(m: super::manifest::StremioMeta, tab: &MediaTab, provider: &str) -> CatalogEntry {
    let year = m.year.as_ref().and_then(|v| {
        v.as_u64().map(|n| n.to_string())
            .or_else(|| v.as_str().map(str::to_string))
    });

    let rating = m.rating.map(|r| format!("{:.1}", r))
        .or_else(|| m.imdb_rating.clone());

    let media_type = match m.r#type.as_str() {
        "movie"  => MediaType::Movie,
        "series" => MediaType::Series,
        "anime"  => MediaType::Series,
        _        => MediaType::Unknown,
    };

    CatalogEntry {
        id:          m.id,
        title:       m.name,
        year,
        genre:       None,
        rating,
        description: m.description,
        poster_url:  m.poster,
        poster_art:  None,
        provider:    provider.to_string(),
        tab:         format!("{:?}", tab).to_lowercase(),
        imdb_id:     m.imdb_id,
        tmdb_id:     None,
        media_type,
        ratings:     std::collections::HashMap::new(),
    }
}

fn stremio_stream_to_stream(
    s: super::manifest::StremioStream,
    provider: &str,
) -> Option<Stream> {
    // Derive a playable URL from either url or infoHash
    let url = if let Some(u) = s.url {
        u
    } else if let Some(hash) = s.info_hash {
        // Convert infoHash → magnet URI
        let magnet = format!("magnet:?xt=urn:btih:{}", hash);
        if let Some(idx) = s.file_idx {
            format!("{}&so={}", magnet, idx)
        } else {
            magnet
        }
    } else {
        return None;
    };

    let name = s.title.or(s.name).unwrap_or_else(|| "Stream".to_string());
    let quality = StreamQuality::from_label(&name);

    Some(Stream {
        id:       url.clone(),
        name,
        url,
        mime:     None,
        quality,
        provider: provider.to_string(),
        ..Default::default()
    })
}
