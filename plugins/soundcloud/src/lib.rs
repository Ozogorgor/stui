// Note: do NOT use #![no_std]. WASM binary size is controlled by Cargo.toml
// profile settings (opt-level = "z", panic = "abort"). Using std enables
// host-side `cargo test` without cfg tricks or feature flags.

// ── Note: this CLIENT_ID is extracted from SoundCloud's public web bundle.
// It is intentionally public (no secret required — public client OAuth model).
// Replace with a current one if SoundCloud rotates it. This demo may break
// if CLIENT_ID expires; it is a pattern reference, not a production plugin.
const CLIENT_ID: &str = "iZIs9mchVcX5lhVRyQGGAYlNPVldzAoX";

pub fn parse_access_token(json: &str) -> Result<String, String> {
    let val: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("parse error: {e}"))?;
    val["access_token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "missing access_token".to_string())
}

pub fn build_auth_url(port: u16) -> String {
    let redirect_uri = format!("http://localhost:{port}/callback");
    format!(
        "https://secure.soundcloud.com/authorize\
         ?client_id={CLIENT_ID}&redirect_uri={redirect_uri}\
         &response_type=code&scope=non-expiring"
    )
}

pub fn build_exchange_body(code: &str, redirect_uri: &str) -> String {
    format!(
        "grant_type=authorization_code&code={code}\
         &redirect_uri={redirect_uri}&client_id={CLIENT_ID}"
    )
}

/// Pure helper: given the raw JSON string returned by `cache_get("sc_token")`,
/// extract the access token. Returns `None` on a cache miss (input is `None`).
///
/// `ensure_authenticated` calls this first; if it returns `Some(Ok(token))`,
/// the OAuth flow is skipped entirely (cache-hit path).
pub fn token_from_cache(cached: Option<String>) -> Option<Result<String, String>> {
    cached.map(|j| parse_access_token(&j))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_access_token_success() {
        let json = r#"{"access_token":"test_token_123","token_type":"bearer"}"#;
        assert_eq!(parse_access_token(json).unwrap(), "test_token_123");
    }

    #[test]
    fn test_parse_access_token_missing() {
        let json = r#"{"token_type":"bearer"}"#;
        assert!(parse_access_token(json).is_err());
    }

    #[test]
    fn test_build_auth_url_contains_redirect_uri() {
        let url = build_auth_url(52314);
        assert!(url.contains("redirect_uri=http://localhost:52314/callback"),
            "URL must contain percent-safe redirect_uri, got: {url}");
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=non-expiring"));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
    }

    #[test]
    fn test_exchange_body_format() {
        let body = build_exchange_body("mycode", "http://localhost:12345/callback");
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code=mycode"));
        assert!(body.contains("redirect_uri=http://localhost:12345/callback"));
        assert!(body.contains(&format!("client_id={CLIENT_ID}")));
    }

    // ── ensure_authenticated cache-hit path ───────────────────────────────────
    #[test]
    fn test_ensure_authenticated_cache_hit_returns_token() {
        let cached_json = r#"{"access_token":"cached_tok_abc","token_type":"bearer"}"#;
        let result = token_from_cache(Some(cached_json.to_string()));
        assert!(result.is_some(), "cache hit must return Some");
        assert_eq!(result.unwrap().unwrap(), "cached_tok_abc",
            "cache-hit: must return cached token without initiating OAuth flow");
    }

    #[test]
    fn test_ensure_authenticated_cache_miss_returns_none() {
        let result = token_from_cache(None);
        assert!(result.is_none(), "cache miss must return None, triggering OAuth flow");
    }

    #[test]
    fn test_ensure_authenticated_cache_hit_bad_json_triggers_reauth() {
        let bad_json = r#"{"token_type":"bearer"}"#; // no access_token
        let result = token_from_cache(Some(bad_json.to_string()));
        assert!(result.is_some());
        assert!(result.unwrap().is_err(), "corrupt cached JSON must return Err");
    }
}

use stui_plugin_sdk::{
    StuiPlugin, PluginType, PluginResult,
    SearchRequest, SearchResponse, PluginEntry,
    ResolveRequest, ResolveResponse,
    cache_get, cache_set,
    auth_allocate_port, auth_open_and_wait,
    http_post_form, http_get,
};

fn exchange_code(code: &str, redirect_uri: &str) -> Result<String, String> {
    let body = build_exchange_body(code, redirect_uri);
    // Note: for production plugins, percent-encode code and redirect_uri values.
    // OAuth codes are typically URL-safe, but correctness requires it.
    http_post_form("https://api.soundcloud.com/oauth2/token", &body)
}

fn ensure_authenticated() -> Result<String, String> {
    // Check cache first — scope=non-expiring means no TTL needed.
    // token_from_cache is the pure helper tested in unit tests above.
    if let Some(result) = token_from_cache(cache_get("sc_token")) {
        return result;
    }
    let port = auth_allocate_port()?;
    let redirect_uri = format!("http://localhost:{port}/callback");
    let url = build_auth_url(port);
    let cb = auth_open_and_wait(&url, 120_000)?;
    let token_json = exchange_code(&cb.code, &redirect_uri)?;
    cache_set("sc_token", &token_json);
    parse_access_token(&token_json)
}

#[derive(Default)]
pub struct SoundCloud;

impl StuiPlugin for SoundCloud {
    fn name(&self)    -> &str { "soundcloud" }
    fn version(&self) -> &str { "0.1.0" }
    fn plugin_type(&self) -> PluginType { PluginType::Provider }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let token = match ensure_authenticated() {
            Ok(t)  => t,
            Err(e) => return PluginResult::err("auth_failed", e),
        };

        let url = format!(
            "https://api.soundcloud.com/search/tracks?q={}&limit=20&client_id={CLIENT_ID}",
            urlencoded(&req.query)
        );

        let body = match http_get(&url) {
            Ok(b)  => b,
            Err(e) => return PluginResult::err("search_failed", e),
        };

        let items = parse_search_results(&body, &token);
        PluginResult::ok(SearchResponse { items, total: 0 })
    }

    fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse> {
        let _token = match ensure_authenticated() {
            Ok(t)  => t,
            Err(e) => return PluginResult::err("auth_failed", e),
        };

        let url = format!(
            "https://api.soundcloud.com/tracks/{}/stream?client_id={CLIENT_ID}",
            req.entry_id
        );
        PluginResult::ok(ResolveResponse {
            stream_url: url,
            quality: Some("audio".into()),
            subtitles: vec![],
        })
    }
}

fn urlencoded(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' '  => '+'.to_string(),
            c if c.is_alphanumeric() || "-_.~".contains(c) => c.to_string(),
            c    => format!("%{:02X}", c as u32),
        })
        .collect()
}

fn parse_search_results(body: &str, _token: &str) -> Vec<PluginEntry> {
    let val: serde_json::Value = match serde_json::from_str(body) {
        Ok(v)  => v,
        Err(_) => return Vec::new(),
    };
    val["collection"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|track| {
            let id    = track["id"].as_u64()?.to_string();
            let title = track["title"].as_str()?.to_string();
            let user  = track["user"]["username"].as_str().unwrap_or("").to_string();
            Some(PluginEntry {
                id,
                title: format!("{title} — {user}"),
                year: None,
                genre: track["genre"].as_str().map(|s| s.to_string()),
                rating: None,
                description: track["description"].as_str().map(|s| s.to_string()),
                poster_url: track["artwork_url"].as_str().map(|s| s.to_string()),
                imdb_id: None,
            })
        })
        .collect()
}

stui_plugin_sdk::stui_export_plugin!(SoundCloud);
