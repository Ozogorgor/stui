//! r18 — stui plugin for R18 (r18.com)
//!
//! R18 is a major adult video site with both Japanese and Western content.
//! No official API - uses web scraping.
//!
//! ## Site structure
//!   https://www.r18.com/search/?searchtext={query} → search results
//!   https://www.r18.com/videos/vod/{id}/           → movie detail
//!
//! ## Notes
//!   R18 may require age verification and may be blocked in some regions.

use stui_plugin_sdk::prelude::*;
use stui_plugin_sdk::{error_codes, EntryKind, SearchScope};

const BASE_URL: &str = "https://www.r18.com";

pub struct R18Provider;

impl R18Provider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for R18Provider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for R18Provider {
    fn name(&self) -> &str {
        "r18"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // r18 only covers adult movies
        if req.scope != SearchScope::Movie {
            return PluginResult::err(
                error_codes::UNSUPPORTED_SCOPE,
                "r18 only supports movie scope",
            );
        }

        let query = req.query.trim();
        if query.is_empty() {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        plugin_info!("r18: searching '{}'", query);

        let page = req.page.max(1);
        let url = format!(
            "{}/search/videos?searchtext={}&page={}",
            BASE_URL,
            url_encode(query),
            page
        );

        let html = match http_get(&url) {
            Ok(h) => h,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let items = parse_search_results(&html, req.limit, EntryKind::Movie);
        // r18 search doesn't provide total count. Use u32::MAX when we hit the
        // limit to signal that more results may exist.
        let total = if req.limit > 0 && items.len() >= req.limit as usize {
            u32::MAX
        } else {
            items.len() as u32
        };

        plugin_info!("r18: found {} results", items.len());
        PluginResult::ok(SearchResponse { items, total })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "r18 is a metadata provider only")
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

fn parse_search_results(html: &str, limit: u32, kind: EntryKind) -> Vec<PluginEntry> {
    let mut entries = Vec::new();

    for line in html.lines() {
        let line = line.trim();

        if line.contains("/videos/vod/") && line.contains("data-id") {
            if let Some(id) = extract_id(line) {
                if let Some(title) = extract_title(line) {
                    entries.push(PluginEntry {
                        id,
                        kind,
                        source: "r18".to_string(),
                        title,
                        year: None,
                        genre: None,
                        rating: None,
                        description: None,
                        poster_url: extract_poster(line),
                        imdb_id: None,
                        duration: None,
                        ..Default::default()
                    });

                    if limit > 0 && entries.len() >= limit as usize {
                        break;
                    }
                }
            }
        }
    }

    entries
}

fn extract_id(line: &str) -> Option<String> {
    // Prefer the explicit data-id attribute when present.
    if let Some(start) = line.find("data-id=\"") {
        let start = start + 9;
        if let Some(end) = line[start..].find('"') {
            let id = &line[start..start + end];
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    // Fall back to extracting the ID segment from the /videos/vod/{id}/ URL.
    // "/videos/vod/" is 12 chars; the ID starts immediately after the trailing slash.
    if let Some(start) = line.find("/videos/vod/") {
        let rest = &line[start + 12..];
        let mut id = String::new();
        for c in rest.chars() {
            if c == '/' {
                break;
            }
            if c.is_ascii_alphanumeric() || c == '-' {
                id.push(c);
            } else {
                break;
            }
        }
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
}

fn extract_title(line: &str) -> Option<String> {
    fn decode_html_entities(s: &str) -> String {
        s.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
    }
    if let Some(start) = line.find("data-title=\"") {
        let start = start + 12;
        if let Some(end) = line[start..].find('"') {
            let title = &line[start..start + end];
            if !title.trim().is_empty() {
                return Some(decode_html_entities(title));
            }
        }
    }
    if let Some(start) = line.find("title=\"") {
        let start = start + 7;
        if let Some(end) = line[start..].find('"') {
            let title = &line[start..start + end];
            if !title.trim().is_empty() {
                return Some(decode_html_entities(title));
            }
        }
    }
    None
}

fn is_valid_r18_url(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        let authority_end = rest
            .find(|c| c == '/' || c == '?' || c == '#')
            .unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if authority.contains('@') {
            return false;
        }
        let host = authority.split(':').next().unwrap_or(authority);
        return host == "r18.com" || host == "www.r18.com" || host.ends_with(".r18.com");
    }
    false
}

fn extract_poster(line: &str) -> Option<String> {
    if let Some(start) = line.find("data-src=\"") {
        let start = start + 10;
        if let Some(end) = line[start..].find('"') {
            let url = &line[start..start + end];
            if !url.is_empty() && is_valid_r18_url(url) {
                return Some(url.to_string());
            }
        }
    }
    if let Some(start) = line.find("src=\"") {
        let start = start + 5;
        if let Some(end) = line[start..].find('"') {
            let url = &line[start..start + end];
            if !url.is_empty() && url.contains("thumb") && is_valid_r18_url(url) {
                return Some(url.to_string());
            }
        }
    }
    None
}

stui_plugin_sdk::stui_export_plugin!(R18Provider);
