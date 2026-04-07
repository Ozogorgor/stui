//! imdb-provider — stui plugin that scrapes IMDB chart pages for trending content.

use scraper::{Html, Selector};
use stui_plugin_sdk::{plugin_warn, prelude::*, PluginType, StuiPlugin};

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
        let tab = req.tab.as_str();
        let query = req.query.trim();

        // IMDB only supports trending (empty query)
        if !query.is_empty() {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        let url = match tab {
            "movies" => MOVIE_METER_URL,
            "series" => TV_METER_URL,
            _ => {
                return PluginResult::ok(SearchResponse {
                    items: vec![],
                    total: 0,
                })
            }
        };

        plugin_info!("imdb: scraping {}", url);

        let body = match http_get(url) {
            Ok(b) => b,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let entries = parse_chart(&body);
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

fn parse_chart(html: &str) -> Vec<PluginEntry> {
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

        // Year
        let year = row
            .select(&year_sel)
            .next()
            .map(|e| {
                e.text()
                    .collect::<String>()
                    .split_whitespace()
                    .find(|s| s.len() == 4 && s.chars().all(|c| c.is_numeric()))
                    .map(|s| s.to_string())
            })
            .flatten()
            .filter(|y| !y.is_empty());

        // Rating
        let rating = row
            .select(&rating_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|r| !r.is_empty() && r != "Rate");

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
            title,
            year,
            genre: None,
            rating,
            description: None,
            poster_url,
            imdb_id,
            duration: None,
        });
    }

    entries
}

// ── WASM Exports ──────────────────────────────────────────────────────────────

stui_export_plugin!(ImdbProvider);
