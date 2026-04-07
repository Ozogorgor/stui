//! javdb — stui plugin for JAVDatabase (javdb.com)
//!
//! JAVDatabase is a free community database for Japanese adult videos.
//! No official API - uses web scraping.
//!
//! ## Site structure
//!   https://javdb.com/search?q={query}      → search results
//!   https://javdb.com/v/{code}              → movie detail
//!   https://javdb.com/actors/{actress}      → actress page
//!
//! ## Search flow
//!   - If query looks like a JAV code (e.g., ABC-123), search directly
//!   - Otherwise search by title
//!
//! ## Notes
//!   JAVDB may require access via certain methods (VPN/proxy) in some regions.

use stui_plugin_sdk::prelude::*;

const BASE_URL: &str = "https://javdb.com";

pub struct JavdbProvider;

impl JavdbProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JavdbProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for JavdbProvider {
    fn name(&self) -> &str {
        "javdb"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let query = req.query.trim();
        if query.is_empty() {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        plugin_info!("javdb: searching '{}'", query);

        let url = format!("{}/search?q={}", BASE_URL, url_encode(query));
        let html = match http_get(&url) {
            Ok(h) => h,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let items = parse_search_results(&html, req.limit);
        // When we hit the limit, indicate there may be more results
        // by adding 1 to signal pagination is needed
        let total = if req.limit > 0 && items.len() as u32 >= req.limit {
            req.limit + 1
        } else {
            items.len() as u32
        };

        plugin_info!(
            "javdb: found {} results (limited: {})",
            items.len(),
            req.limit > 0 && items.len() as u32 >= req.limit
        );
        PluginResult::ok(SearchResponse { items, total })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "javdb is a metadata provider only")
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

fn parse_search_results(html: &str, limit: u32) -> Vec<PluginEntry> {
    let mut entries = Vec::new();

    let mut in_movie_box = false;
    let mut current_code = String::new();
    let mut current_title = String::new();
    let mut current_poster = String::new();

    for line in html.lines() {
        let line = line.trim();

        if line.contains("movie-box") {
            in_movie_box = true;
            current_code.clear();
            current_title.clear();
            current_poster.clear();
            continue;
        }

        if in_movie_box {
            // End of movie-box section when we see closing tags
            if line.contains("</a>") || line.contains("</div>") {
                if !current_code.is_empty() && !current_title.is_empty() {
                    entries.push(PluginEntry {
                        id: current_code.clone(),
                        title: current_title.clone(),
                        year: None,
                        genre: None,
                        rating: None,
                        description: None,
                        poster_url: if current_poster.is_empty() {
                            None
                        } else {
                            Some(current_poster.clone())
                        },
                        imdb_id: None,
                        duration: None,
                    });

                    if limit > 0 && entries.len() >= limit as usize {
                        break;
                    }
                }
                in_movie_box = false;
            } else if line.contains("/v/") {
                if let Some(code) = extract_code(line) {
                    current_code = code;
                }
            } else if line.contains("data-src=") {
                if let Some(poster) = extract_poster(line) {
                    current_poster = poster;
                }
            } else if line.contains("title=\"") {
                if let Some(title) = extract_title(line) {
                    current_title = title;
                }
            }
        }
    }

    entries
}

fn extract_code(line: &str) -> Option<String> {
    if let Some(start) = line.find("/v/") {
        let rest = &line[start + 3..];
        let mut code = String::new();
        for c in rest.chars() {
            if c.is_ascii_alphanumeric() || c == '-' {
                code.push(c);
            } else {
                break;
            }
        }
        if !code.is_empty() {
            return Some(code);
        }
    }
    None
}

fn extract_poster(line: &str) -> Option<String> {
    let marker = "data-src=\"";
    if let Some(rest) = line.split_once(marker).map(|(_, r)| r) {
        if let Some(url) = rest.split('"').next() {
            if !url.is_empty() {
                let full_url = if url.starts_with("http") {
                    url.to_string()
                } else if url.starts_with("//") {
                    format!("https:{}", url)
                } else if url.starts_with('/') {
                    format!("{}{}", BASE_URL, url)
                } else {
                    format!("{}/{}", BASE_URL, url)
                };
                if is_valid_javdb_url(&full_url) {
                    return Some(full_url);
                }
            }
        }
    }
    None
}

fn is_valid_javdb_url(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        let authority = if let Some(slash_pos) = rest.find('/') {
            &rest[..slash_pos]
        } else {
            rest
        };
        if authority.contains('@') || authority.contains("%40") {
            return false;
        }
        let host = authority.split(':').next().unwrap_or(authority);
        return host == "javdb.com" || host == "www.javdb.com";
    }
    false
}

fn extract_title(line: &str) -> Option<String> {
    let marker = "title=\"";
    if let Some(rest) = line.split_once(marker).map(|(_, r)| r) {
        if let Some(raw_title) = rest.split('"').next() {
            let title = decode_html_entities(raw_title).trim().to_string();
            if !title.is_empty() {
                return Some(title);
            }
        }
    }
    None
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

stui_plugin_sdk::stui_export_plugin!(JavdbProvider);
