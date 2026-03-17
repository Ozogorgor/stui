//! IMDB provider — scrapes IMDB chart pages for trending content.
//!
//! Sources:
//!   Movies : https://www.imdb.com/chart/moviemeter/  (most popular movies)
//!   Series : https://www.imdb.com/chart/tvmeter/     (most popular TV)
//!
//! This scraper is intentionally minimal and resilient — IMDB occasionally
//! changes its HTML structure, so we use multiple selector fallbacks and
//! degrade gracefully rather than crashing.
//!
//! NOTE: Web scraping must respect robots.txt and rate limits.
//! stui adds a 500ms delay between requests and caps at 50 items.

use anyhow::{Context, Result};
use async_trait::async_trait;
use scraper::{Html, Selector};
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::MediaTab;
use crate::providers::Provider;

const MOVIE_METER_URL: &str = "https://www.imdb.com/chart/moviemeter/";
const TV_METER_URL:    &str = "https://www.imdb.com/chart/tvmeter/";
const MAX_ITEMS: usize = 50;

pub struct ImdbProvider {
    client: reqwest::Client,
}

impl ImdbProvider {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(
                // Identify as a browser to avoid bot blocks
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
            )
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("HTTP client build failed");
        Self { client }
    }

    async fn scrape_chart(&self, url: &str, tab: &str) -> Result<Vec<CatalogEntry>> {
        debug!(provider = "imdb", url, "scraping chart");

        let html = self.client
            .get(url)
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .context("HTTP request failed")?
            .text()
            .await?;

        // Respect server — small delay before any further requests
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        parse_chart(&html, tab)
    }
}

#[async_trait]
impl Provider for ImdbProvider {
    fn name(&self) -> &str { "imdb" }
    fn display_name(&self) -> &str { "IMDB" }
    fn description(&self) -> &str { "IMDB chart scraper — trending movies & TV, no API key needed" }

    async fn fetch_trending(&self, tab: &MediaTab, _page: u32) -> Result<Vec<CatalogEntry>> {
        match tab {
            MediaTab::Movies => self.scrape_chart(MOVIE_METER_URL, "movies").await,
            MediaTab::Series => self.scrape_chart(TV_METER_URL, "series").await,
            _ => Ok(vec![]),
        }
    }

    async fn search(&self, _tab: &MediaTab, _query: &str, _page: u32) -> Result<Vec<CatalogEntry>> {
        // IMDB search requires more complex handling (JavaScript-rendered results).
        // For now, search is handled by OMDB/TMDB. This provider is trending-only.
        Ok(vec![])
    }
}

// ── HTML parser ───────────────────────────────────────────────────────────────

fn parse_chart(html: &str, tab: &str) -> Result<Vec<CatalogEntry>> {
    let document = Html::parse_document(html);
    let mut entries = vec![];

    // IMDB chart rows — selector targets the list items in the chart table
    // We try multiple selectors for resilience across IMDB HTML changes
    let row_sel = Selector::parse(
        "li.ipc-metadata-list-summary-item, \
         .lister-list tr, \
         .chart tbody tr"
    ).unwrap();

    let title_sel   = Selector::parse("h3.ipc-title__text, .titleColumn a, td.titleColumn a").unwrap();
    let year_sel    = Selector::parse("span.cli-title-metadata-item, .titleColumn span, .secondaryInfo").unwrap();
    let rating_sel  = Selector::parse("span.ipc-rating-star--imdb, td.ratingColumn strong").unwrap();
    let poster_sel  = Selector::parse("img.ipc-image, td.posterColumn img").unwrap();
    let link_sel    = Selector::parse("a.ipc-title-link-wrapper, .titleColumn a").unwrap();

    for row in document.select(&row_sel).take(MAX_ITEMS) {
        // Title
        let title = row.select(&title_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if title.is_empty() || title.chars().all(|c| c.is_numeric()) {
            continue; // skip rank numbers mistakenly selected
        }

        // IMDB id from href
        let imdb_id = row.select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .and_then(|href| {
                // href looks like /title/tt1234567/...
                href.split('/').find(|s| s.starts_with("tt")).map(|s| s.to_string())
            });

        // Year
        let year = row.select(&year_sel)
            .next()
            .map(|e| {
                let t = e.text().collect::<String>();
                // Extract 4-digit year
                t.chars()
                    .collect::<String>()
                    .split_whitespace()
                    .find(|s| s.len() == 4 && s.chars().all(|c| c.is_numeric()))
                    .unwrap_or("")
                    .to_string()
            })
            .filter(|y| !y.is_empty());

        // Rating
        let rating = row.select(&rating_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|r| !r.is_empty() && r != "Rate");

        // Poster
        let poster_url = row.select(&poster_sel)
            .next()
            .and_then(|img| {
                // Prefer `src` over `loadlate` — IMDB lazy-loads posters
                img.value().attr("src")
                    .or_else(|| img.value().attr("loadlate"))
            })
            .map(|u| u.to_string())
            .filter(|u| u.starts_with("https"));

        let id = imdb_id.clone().unwrap_or_else(|| {
            format!("imdb-{}", title.to_lowercase().replace(' ', "-"))
        });

        entries.push(CatalogEntry {
            id,
            title,
            year,
            genre: None, // IMDB chart page doesn't include genre inline
            rating,
            description: None,
            poster_url,
            poster_art: None,
            provider: "imdb".to_string(),
            tab: tab.to_string(),
            imdb_id,
            tmdb_id: None,
            media_type: crate::ipc::MediaType::default(),
            ratings: std::collections::HashMap::new(),
        });
    }

    if entries.is_empty() {
        warn!(provider = "imdb", "parser returned 0 entries — IMDB HTML may have changed");
    } else {
        debug!(provider = "imdb", count = entries.len(), "parsed chart");
    }

    Ok(entries)
}
