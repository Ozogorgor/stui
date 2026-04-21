use musicbrainz_provider::MusicbrainzPlugin;
use stui_plugin_sdk::{CatalogPlugin, SearchRequest, SearchScope};

/// Empty query is the "no trending" path — must return 0 results without
/// hitting the network. (MB has no trending endpoint.)
#[test]
fn search_empty_query_returns_zero_results_without_network() {
    let plugin = MusicbrainzPlugin::new();
    let req = SearchRequest {
        query: String::new(),
        scope: SearchScope::Artist,
        page: 1,
        limit: 10,
        per_scope_limit: None,
        locale: None,
    };
    let resp = match plugin.search(req) {
        stui_plugin_sdk::PluginResult::Ok(r) => r,
        stui_plugin_sdk::PluginResult::Err(e) => panic!("search returned an error: {}", e.message),
    };
    assert_eq!(resp.items.len(), 0);
    assert_eq!(resp.total, 0);
}
