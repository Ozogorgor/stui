//! Per-verb plugin routing maps.
//!
//! `Dispatcher` is the broader successor to `engine::dispatch_map::DispatchMap`,
//! covering all six CatalogPlugin verbs (search, lookup, enrich, artwork,
//! credits, related). The existing scope-only `DispatchMap` is still used by
//! `search_scoped`; they live side-by-side for now and may be merged as a
//! follow-up.

#![allow(dead_code)]

use std::collections::HashMap;

use stui_plugin_sdk::{EntryKind, SearchScope};

use super::manifest::CatalogCapability;

// ── LoadedPluginSummary ───────────────────────────────────────────────────────

/// Lightweight view of a loaded plugin, sufficient for `Dispatcher::rebuild`
/// to construct routing maps. The full `LoadedPlugin` is kept in the engine's
/// registry; this summary is a projection.
#[derive(Debug, Clone)]
pub struct LoadedPluginSummary {
    pub name: String,
    pub capabilities: CatalogCapability,
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct Dispatcher {
    by_scope: HashMap<SearchScope, Vec<String>>,
    by_lookup: HashMap<(String /* id_source */, EntryKind), Vec<String>>,
    by_enrich: HashMap<EntryKind, Vec<String>>,
    by_artwork: HashMap<EntryKind, Vec<String>>,
    by_credits: HashMap<EntryKind, Vec<String>>,
    by_related: HashMap<EntryKind, Vec<String>>,
    by_episodes: HashMap<EntryKind, Vec<String>>,
    by_trailers: HashMap<EntryKind, Vec<String>>,
    by_release_info: HashMap<EntryKind, Vec<String>>,
    by_keywords: HashMap<EntryKind, Vec<String>>,
    by_box_office: HashMap<EntryKind, Vec<String>>,
    by_alternative_titles: HashMap<EntryKind, Vec<String>>,
    by_bulk_enrich: HashMap<EntryKind, Vec<String>>,
}

impl Dispatcher {
    /// Build a dispatcher from a list of plugin summaries. Declaration order
    /// is preserved within each routing bucket.
    pub fn rebuild(plugins: &[LoadedPluginSummary]) -> Self {
        let mut d = Self::default();

        for p in plugins {
            let CatalogCapability::Typed {
                kinds,
                search,
                lookup,
                enrich,
                artwork,
                credits,
                related,
                episodes,
                trailers,
                release_info,
                keywords,
                box_office,
                alternative_titles,
                bulk_enrich,
            } = &p.capabilities
            else {
                // Legacy bool / disabled plugins aren't routed through the new
                // per-verb maps. They continue to be reached via the legacy
                // catalog fan-out in engine/mod.rs.
                continue;
            };

            // Only plugins with search = true are enumerated per-scope. A
            // plugin must opt in to each kind explicitly.
            if search.unwrap_or(false) {
                for k in kinds {
                    d.by_scope
                        .entry(scope_of(*k))
                        .or_default()
                        .push(p.name.clone());
                }
            }

            if let Some(lookup) = lookup {
                if lookup.is_enabled() && !lookup.is_stub() {
                    for id_source in &lookup.id_sources {
                        for k in kinds {
                            d.by_lookup
                                .entry((id_source.clone(), *k))
                                .or_default()
                                .push(p.name.clone());
                        }
                    }
                }
            }

            // enrich / credits / related / artwork each just need the plugin
            // to have declared the verb for any of its kinds.
            if let Some(vc) = enrich {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_enrich.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(ac) = artwork {
                if ac.is_enabled() && !ac.is_stub() {
                    for k in kinds {
                        d.by_artwork.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = credits {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_credits.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = related {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_related.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = episodes {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_episodes.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = trailers {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_trailers.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = release_info {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_release_info
                            .entry(*k)
                            .or_default()
                            .push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = keywords {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_keywords.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = box_office {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_box_office.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = alternative_titles {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_alternative_titles
                            .entry(*k)
                            .or_default()
                            .push(p.name.clone());
                    }
                }
            }
            if let Some(vc) = bulk_enrich {
                if vc.is_enabled() && !vc.is_stub() {
                    for k in kinds {
                        d.by_bulk_enrich.entry(*k).or_default().push(p.name.clone());
                    }
                }
            }
        }

        d
    }

    pub fn plugins_for_bulk_enrich(&self, kind: EntryKind) -> Vec<String> {
        self.by_bulk_enrich.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_scope(&self, scope: SearchScope) -> Vec<String> {
        self.by_scope.get(&scope).cloned().unwrap_or_default()
    }

    pub fn plugins_for_lookup(&self, id_source: &str, kind: EntryKind) -> Vec<String> {
        self.by_lookup
            .get(&(id_source.to_string(), kind))
            .cloned()
            .unwrap_or_default()
    }

    pub fn plugins_for_enrich(&self, kind: EntryKind) -> Vec<String> {
        self.by_enrich.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_artwork(&self, kind: EntryKind) -> Vec<String> {
        self.by_artwork.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_credits(&self, kind: EntryKind) -> Vec<String> {
        self.by_credits.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_related(&self, kind: EntryKind) -> Vec<String> {
        self.by_related.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_episodes(&self, kind: EntryKind) -> Vec<String> {
        self.by_episodes.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_trailers(&self, kind: EntryKind) -> Vec<String> {
        self.by_trailers.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_release_info(&self, kind: EntryKind) -> Vec<String> {
        self.by_release_info.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_keywords(&self, kind: EntryKind) -> Vec<String> {
        self.by_keywords.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_box_office(&self, kind: EntryKind) -> Vec<String> {
        self.by_box_office.get(&kind).cloned().unwrap_or_default()
    }

    pub fn plugins_for_alternative_titles(&self, kind: EntryKind) -> Vec<String> {
        self.by_alternative_titles
            .get(&kind)
            .cloned()
            .unwrap_or_default()
    }
}

fn scope_of(k: EntryKind) -> SearchScope {
    match k {
        EntryKind::Artist => SearchScope::Artist,
        EntryKind::Album => SearchScope::Album,
        EntryKind::Track => SearchScope::Track,
        EntryKind::Movie => SearchScope::Movie,
        EntryKind::Series => SearchScope::Series,
        EntryKind::Episode => SearchScope::Episode,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifest::{ArtworkConfig, LookupConfig, VerbConfig};

    fn typed(
        kinds: &[EntryKind],
        search: bool,
        lookup: Option<LookupConfig>,
        enrich: Option<VerbConfig>,
        artwork: Option<ArtworkConfig>,
        credits: Option<VerbConfig>,
        related: Option<VerbConfig>,
    ) -> CatalogCapability {
        typed_v2(
            kinds, search, lookup, enrich, artwork, credits, related, None, None, None, None, None,
            None, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn typed_v2(
        kinds: &[EntryKind],
        search: bool,
        lookup: Option<LookupConfig>,
        enrich: Option<VerbConfig>,
        artwork: Option<ArtworkConfig>,
        credits: Option<VerbConfig>,
        related: Option<VerbConfig>,
        episodes: Option<VerbConfig>,
        trailers: Option<VerbConfig>,
        release_info: Option<VerbConfig>,
        keywords: Option<VerbConfig>,
        box_office: Option<VerbConfig>,
        alternative_titles: Option<VerbConfig>,
        bulk_enrich: Option<VerbConfig>,
    ) -> CatalogCapability {
        CatalogCapability::Typed {
            kinds: kinds.to_vec(),
            search: Some(search),
            lookup,
            enrich,
            artwork,
            credits,
            related,
            episodes,
            trailers,
            release_info,
            keywords,
            box_office,
            alternative_titles,
            bulk_enrich,
        }
    }

    fn plugin(name: &str, caps: CatalogCapability) -> LoadedPluginSummary {
        LoadedPluginSummary {
            name: name.into(),
            capabilities: caps,
        }
    }

    #[test]
    fn search_routes_by_scope_in_declaration_order() {
        let a = plugin(
            "a",
            typed(&[EntryKind::Movie], true, None, None, None, None, None),
        );
        let b = plugin(
            "b",
            typed(
                &[EntryKind::Movie, EntryKind::Series],
                true,
                None,
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[a, b]);
        assert_eq!(d.plugins_for_scope(SearchScope::Movie), vec!["a", "b"]);
        assert_eq!(d.plugins_for_scope(SearchScope::Series), vec!["b"]);
        assert!(d.plugins_for_scope(SearchScope::Track).is_empty());
    }

    #[test]
    fn lookup_routes_by_id_source_and_kind() {
        let tmdb = plugin(
            "tmdb",
            typed(
                &[EntryKind::Movie, EntryKind::Series],
                true,
                Some(LookupConfig {
                    id_sources: vec!["tmdb".into(), "imdb".into()],
                    stub: false,
                    reason: None,
                }),
                None,
                None,
                None,
                None,
            ),
        );
        let omdb = plugin(
            "omdb",
            typed(
                &[EntryKind::Movie],
                true,
                Some(LookupConfig {
                    id_sources: vec!["imdb".into()],
                    stub: false,
                    reason: None,
                }),
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[tmdb, omdb]);
        assert_eq!(d.plugins_for_lookup("tmdb", EntryKind::Movie), vec!["tmdb"]);
        assert_eq!(
            d.plugins_for_lookup("imdb", EntryKind::Movie),
            vec!["tmdb", "omdb"]
        );
        assert_eq!(
            d.plugins_for_lookup("imdb", EntryKind::Series),
            vec!["tmdb"]
        );
        assert!(d.plugins_for_lookup("tvdb", EntryKind::Movie).is_empty());
    }

    #[test]
    fn enrich_credits_related_artwork_routes_by_kind() {
        let p = plugin(
            "p",
            typed(
                &[EntryKind::Movie],
                true,
                None,
                Some(VerbConfig::Bool(true)),
                Some(ArtworkConfig {
                    sizes: vec!["standard".into()],
                    stub: false,
                    reason: None,
                }),
                Some(VerbConfig::Bool(true)),
                Some(VerbConfig::Bool(true)),
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(d.plugins_for_enrich(EntryKind::Movie), vec!["p"]);
        assert_eq!(d.plugins_for_artwork(EntryKind::Movie), vec!["p"]);
        assert_eq!(d.plugins_for_credits(EntryKind::Movie), vec!["p"]);
        assert_eq!(d.plugins_for_related(EntryKind::Movie), vec!["p"]);
    }

    #[test]
    fn stub_verbs_excluded_from_routing() {
        let p = plugin(
            "p",
            typed(
                &[EntryKind::Movie],
                true,
                Some(LookupConfig {
                    id_sources: vec!["tmdb".into()],
                    stub: true,
                    reason: Some("upstream lacks it".into()),
                }),
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert!(d.plugins_for_lookup("tmdb", EntryKind::Movie).is_empty());
    }

    #[test]
    fn search_false_excludes_from_scope_routing() {
        let p = plugin(
            "p",
            typed(&[EntryKind::Movie], false, None, None, None, None, None),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert!(d.plugins_for_scope(SearchScope::Movie).is_empty());
    }

    #[test]
    fn legacy_bool_catalog_excluded_from_all_routing() {
        let p = plugin("legacy", CatalogCapability::Enabled(true));
        let d = Dispatcher::rebuild(&[p]);
        assert!(d.plugins_for_scope(SearchScope::Movie).is_empty());
        assert!(d.plugins_for_lookup("tmdb", EntryKind::Movie).is_empty());
    }

    #[test]
    fn empty_lookup_config_not_routed() {
        // A plugin declares `[capabilities.catalog.lookup]` with an empty body
        // (no id_sources). is_enabled() returns false → must NOT be in by_lookup.
        let p = plugin(
            "p",
            typed(
                &[EntryKind::Movie],
                true,
                Some(LookupConfig {
                    id_sources: vec![],
                    stub: false,
                    reason: None,
                }),
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert!(
            d.plugins_for_lookup("tmdb", EntryKind::Movie).is_empty(),
            "empty LookupConfig (no id_sources) must not appear in by_lookup"
        );
    }

    #[test]
    fn empty_artwork_config_not_routed() {
        // A plugin declares `[capabilities.catalog.artwork]` with an empty body
        // (no sizes). is_enabled() returns false → must NOT be in by_artwork.
        let p = plugin(
            "p",
            typed(
                &[EntryKind::Movie],
                true,
                None,
                None,
                Some(ArtworkConfig {
                    sizes: vec![],
                    stub: false,
                    reason: None,
                }),
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert!(
            d.plugins_for_artwork(EntryKind::Movie).is_empty(),
            "empty ArtworkConfig (no sizes) must not appear in by_artwork"
        );
    }

    // ── New v2 verb routing tests ─────────────────────────────────────────────

    #[test]
    fn episodes_routes_when_enabled() {
        let p = plugin(
            "tvdb",
            typed_v2(
                &[EntryKind::Series],
                true,
                None,
                None,
                None,
                None,
                None,
                Some(VerbConfig::Bool(true)), // episodes
                None,
                None,
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(d.plugins_for_episodes(EntryKind::Series), vec!["tvdb"]);
        assert!(d.plugins_for_episodes(EntryKind::Movie).is_empty());
    }

    #[test]
    fn trailers_routes_when_enabled() {
        let p = plugin(
            "tmdb",
            typed_v2(
                &[EntryKind::Movie],
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(VerbConfig::Bool(true)), // trailers
                None,
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(d.plugins_for_trailers(EntryKind::Movie), vec!["tmdb"]);
        assert!(d.plugins_for_trailers(EntryKind::Series).is_empty());
    }

    #[test]
    fn release_info_routes_when_enabled() {
        let p = plugin(
            "omdb",
            typed_v2(
                &[EntryKind::Movie],
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(VerbConfig::Bool(true)), // release_info
                None,
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(d.plugins_for_release_info(EntryKind::Movie), vec!["omdb"]);
        assert!(d.plugins_for_release_info(EntryKind::Series).is_empty());
    }

    #[test]
    fn keywords_routes_when_enabled() {
        let p = plugin(
            "tmdb",
            typed_v2(
                &[EntryKind::Movie, EntryKind::Series],
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(VerbConfig::Bool(true)), // keywords
                None,
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(d.plugins_for_keywords(EntryKind::Movie), vec!["tmdb"]);
        assert_eq!(d.plugins_for_keywords(EntryKind::Series), vec!["tmdb"]);
        assert!(d.plugins_for_keywords(EntryKind::Track).is_empty());
    }

    #[test]
    fn box_office_routes_when_enabled() {
        let p = plugin(
            "omdb",
            typed_v2(
                &[EntryKind::Movie],
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(VerbConfig::Bool(true)), // box_office
                None,
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(d.plugins_for_box_office(EntryKind::Movie), vec!["omdb"]);
        assert!(d.plugins_for_box_office(EntryKind::Series).is_empty());
    }

    #[test]
    fn alternative_titles_routes_when_enabled() {
        let p = plugin(
            "tmdb",
            typed_v2(
                &[EntryKind::Movie],
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(VerbConfig::Bool(true)), // alternative_titles
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert_eq!(
            d.plugins_for_alternative_titles(EntryKind::Movie),
            vec!["tmdb"]
        );
        assert!(d
            .plugins_for_alternative_titles(EntryKind::Series)
            .is_empty());
    }

    #[test]
    fn stub_new_verbs_excluded_from_routing() {
        // A plugin with stub=true on all new verbs must NOT appear in any
        // of the new routing maps.
        let stub = VerbConfig::Stub {
            stub: true,
            reason: Some("not yet implemented".into()),
        };
        let p = plugin(
            "p",
            typed_v2(
                &[EntryKind::Movie],
                false,
                None,
                None,
                None,
                None,
                None,
                Some(stub.clone()),
                Some(stub.clone()),
                Some(stub.clone()),
                Some(stub.clone()),
                Some(stub.clone()),
                Some(stub.clone()),
                None,
            ),
        );
        let d = Dispatcher::rebuild(&[p]);
        assert!(
            d.plugins_for_episodes(EntryKind::Movie).is_empty(),
            "stub episodes must not route"
        );
        assert!(
            d.plugins_for_trailers(EntryKind::Movie).is_empty(),
            "stub trailers must not route"
        );
        assert!(
            d.plugins_for_release_info(EntryKind::Movie).is_empty(),
            "stub release_info must not route"
        );
        assert!(
            d.plugins_for_keywords(EntryKind::Movie).is_empty(),
            "stub keywords must not route"
        );
        assert!(
            d.plugins_for_box_office(EntryKind::Movie).is_empty(),
            "stub box_office must not route"
        );
        assert!(
            d.plugins_for_alternative_titles(EntryKind::Movie)
                .is_empty(),
            "stub alternative_titles must not route"
        );
    }
}
