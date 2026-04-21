//! musicbrainz — stui metadata plugin.

use stui_plugin_sdk::{
    Plugin, CatalogPlugin,
    PluginManifest, PluginResult,
    SearchRequest, SearchResponse,
    stui_export_catalog_plugin,
};

pub struct MusicbrainzPlugin {
    manifest: PluginManifest,
}

impl MusicbrainzPlugin {
    pub fn new() -> Self {
        // TODO: replace with an include_manifest!("plugin.toml") macro once
        // the SDK exposes one. Runtime TOML parse works today (option A).
        let manifest: PluginManifest = toml::from_str(include_str!("../plugin.toml"))
            .expect("plugin.toml is invalid TOML");
        Self { manifest }
    }
}

impl Default for MusicbrainzPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for MusicbrainzPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
}

impl CatalogPlugin for MusicbrainzPlugin {
    fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
        // Empty-result stub — replace with real implementation.
        PluginResult::Ok(SearchResponse { items: vec![], total: 0 })
    }
    // Other verbs: default impls return NOT_IMPLEMENTED. Uncomment the
    // declarations in plugin.toml and override here when ready.
}

stui_export_catalog_plugin!(MusicbrainzPlugin);
