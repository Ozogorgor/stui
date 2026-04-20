//! imdb-provider — stui plugin that scrapes IMDB chart pages for trending content.

use scraper::{Html, Selector};
use stui_plugin_sdk::{error_codes, plugin_warn, prelude::*, EntryKind, PluginType, SearchScope, StuiPlugin};

const MOVIE_METER_URL: &str = "https://www.imdb.com/chart/moviemeter/";
const TV_METER_URL: &str = "https://www.imdb.com/chart/tvmeter/";
const MAX_ITEMS: usize = 50;

pub struct ImdbProvider;

impl ImdbProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ImdbProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for ImdbProvider {
    fn name(&self) -> &str {
        "imdb"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let query = req.query.trim();

        // IMDB chart scraper only supports trending (empty query)
        if !query.is_empty() {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        let (url, entry_kind) = match req.scope {
            SearchScope::Movie => (MOVIE_METER_URL, EntryKind::Movie),
            SearchScope::Series => (TV_METER_URL, EntryKind::Series),
            SearchScope::Episode => (TV_METER_URL, EntryKind::Episode),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "imdb only supports movie, series, and episode scopes",
                );
            }
        };

        plugin_info!("imdb: scraping {}", url);

        let body = match http_get(url) {
            Ok(b) => b,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let entries = parse_chart(&body, entry_kind);
        let total = entries.len() as u32;
        plugin_info!("imdb: {} entries", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "imdb provider does not resolve streams")
    }
}

fn parse_chart(html: &str, kind: EntryKind) -> Vec<PluginEntry> {
    let mut entries = vec![];

    // IMDB chart rows — try multiple selector patterns for resilience
    let row_sel = match Selector::parse(
        "li.ipc-metadata-list-summary-item, \
         .lister-list tr, \
         .chart tbody tr",
    ) {
        Ok(sel) => sel,
        Err(e) => {
            plugin_warn!("imdb: failed to parse row selector: {}", e);
            return entries;
        }
    };

    let title_sel = match Selector::parse("h3.ipc-title__text, .titleColumn a, td.titleColumn a") {
        Ok(sel) => sel,
        Err(e) => {
            plugin_warn!("imdb: failed to parse title selector: {}", e);
            return entries;
        }
    };
    let year_sel =
        match Selector::parse("span.cli-title-metadata-item, .titleColumn span, .secondaryInfo") {
            Ok(sel) => sel,
            Err(e) => {
                plugin_warn!("imdb: failed to parse year selector: {}", e);
                return entries;
            }
        };
    let rating_sel = match Selector::parse("span.ipc-rating-star--imdb, td.ratingColumn strong") {
        Ok(sel) => sel,
        Err(e) => {
            plugin_warn!("imdb: failed to parse rating selector: {}", e);
            return entries;
        }
    };
    let poster_sel = match Selector::parse("img.ipc-image, td.posterColumn img") {
        Ok(sel) => sel,
        Err(e) => {
            plugin_warn!("imdb: failed to parse poster selector: {}", e);
            return entries;
        }
    };
    let link_sel = match Selector::parse("a.ipc-title-link-wrapper, .titleColumn a") {
        Ok(sel) => sel,
        Err(e) => {
            plugin_warn!("imdb: failed to parse link selector: {}", e);
            return entries;
        }
    };

    let document = Html::parse_document(html);

    for row in document.select(&row_sel).take(MAX_ITEMS) {
        // Title
        let title = row
            .select(&title_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if title.is_empty() || title.chars().all(|c| c.is_numeric()) {
            continue;
        }

        // IMDB id from href
        let imdb_id = row
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .and_then(|href| {
                href.split('/')
                    .find(|s| s.starts_with("tt"))
                    .map(|s| s.to_string())
            });

        // Year as u32
        let year = row
            .select(&year_sel)
            .next()
            .and_then(|e| {
                e.text()
                    .collect::<String>()
                    .split_whitespace()
                    .find(|s| s.len() == 4 && s.chars().all(|c| c.is_numeric()))
                    .and_then(|s| s.parse::<u32>().ok())
            });

        // Rating as f32 (IMDB ratings are e.g. "7.5")
        let rating = row
            .select(&rating_sel)
            .next()
            .and_then(|e| {
                let text = e.text().collect::<String>();
                let trimmed = text.trim();
                if trimmed.is_empty() || trimmed == "Rate" {
                    None
                } else {
                    trimmed.parse::<f32>().ok()
                }
            });

        // Poster
        let poster_url = row
            .select(&poster_sel)
            .next()
            .and_then(|img| {
                img.value()
                    .attr("src")
                    .or_else(|| img.value().attr("loadlate"))
            })
            .map(|u| u.to_string())
            .filter(|u| u.starts_with("https"));

        let id = imdb_id
            .clone()
            .unwrap_or_else(|| format!("imdb-{}", title.to_lowercase().replace(' ', "-")));

        entries.push(PluginEntry {
            id,
            kind,
            source: "imdb".to_string(),
            title,
            year,
            genre: None,
            rating,
            description: None,
            poster_url,
            imdb_id,
            duration: None,
            ..Default::default()
        });
    }

    entries
}

// ── WASM Exports ──────────────────────────────────────────────────────────────

stui_export_plugin!(ImdbProvider);
