//! Per-scope dispatch map built from plugin manifests.
//!
//! For each `SearchScope`, maintains the ordered list of plugin ids whose
//! manifest's `capabilities.catalog.kinds` contains the corresponding
//! `EntryKind`. Plugins that declared no kinds (legacy `catalog = true`
//! form, or no catalog capability at all) are excluded from every scope
//! — strict opt-in per design §4.2.

use std::collections::HashMap;
use stui_plugin_sdk::{EntryKind, SearchScope};

pub struct PluginEntryInfo {
    pub id: String,
    pub kinds: Vec<EntryKind>,
}

#[derive(Default, Debug, Clone)]
pub struct DispatchMap {
    by_scope: HashMap<SearchScope, Vec<String>>,
}

impl DispatchMap {
    pub fn build(plugins: &[PluginEntryInfo]) -> Self {
        let mut by_scope: HashMap<SearchScope, Vec<String>> = HashMap::new();
        for p in plugins {
            for k in &p.kinds {
                by_scope.entry(scope_of(*k)).or_default().push(p.id.clone());
            }
        }
        Self { by_scope }
    }

    pub fn plugins_for(&self, scope: SearchScope) -> Vec<String> {
        self.by_scope.get(&scope).cloned().unwrap_or_default()
    }

    pub fn is_empty_for(&self, scope: SearchScope) -> bool {
        self.by_scope.get(&scope).map_or(true, Vec::is_empty)
    }
}

fn scope_of(k: EntryKind) -> SearchScope {
    match k {
        EntryKind::Artist  => SearchScope::Artist,
        EntryKind::Album   => SearchScope::Album,
        EntryKind::Track   => SearchScope::Track,
        EntryKind::Movie   => SearchScope::Movie,
        EntryKind::Series  => SearchScope::Series,
        EntryKind::Episode => SearchScope::Episode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, kinds: &[EntryKind]) -> PluginEntryInfo {
        PluginEntryInfo { id: id.into(), kinds: kinds.to_vec() }
    }

    #[test]
    fn groups_by_scope_preserving_order() {
        let plugins = vec![
            info("discogs", &[EntryKind::Artist, EntryKind::Album, EntryKind::Track]),
            info("tmdb",    &[EntryKind::Movie, EntryKind::Series]),
            info("lastfm",  &[EntryKind::Artist, EntryKind::Track]),
        ];
        let m = DispatchMap::build(&plugins);
        assert_eq!(m.plugins_for(SearchScope::Artist), vec!["discogs", "lastfm"]);
        assert_eq!(m.plugins_for(SearchScope::Track),  vec!["discogs", "lastfm"]);
        assert_eq!(m.plugins_for(SearchScope::Album),  vec!["discogs"]);
        assert_eq!(m.plugins_for(SearchScope::Movie),  vec!["tmdb"]);
        assert!(m.plugins_for(SearchScope::Episode).is_empty());
    }

    #[test]
    fn plugin_with_empty_kinds_excluded_from_every_scope() {
        let plugins = vec![info("legacy-bool", &[])];
        let m = DispatchMap::build(&plugins);
        for s in [SearchScope::Artist, SearchScope::Album, SearchScope::Track,
                  SearchScope::Movie, SearchScope::Series, SearchScope::Episode] {
            assert!(m.plugins_for(s).is_empty());
            assert!(m.is_empty_for(s));
        }
    }

    #[test]
    fn empty_plugin_list_is_empty_everywhere() {
        let m = DispatchMap::build(&[]);
        assert!(m.plugins_for(SearchScope::Movie).is_empty());
    }
}
