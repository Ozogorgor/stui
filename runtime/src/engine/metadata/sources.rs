//! [`SourceResolver`] — picks capable plugins per (verb, kind).
//!
//! Given a [`MetadataVerb`] (enrich / credits / artwork / related) and a
//! lowercase kind-hint (`"movies" | "series" | "anime" | "music"`), the
//! resolver consults the user's [`MetadataSources`] priority list for that
//! kind, filters it through a [`SourceCapabilityProbe`] (which knows which
//! plugins implement which verb), and returns an ordered `Vec<String>` of
//! plugin ids to consult — primary first, then fallbacks.
//!
//! Unknown kind hints return an empty list (no sources).

use crate::cache::metadata_key::MetadataVerb;
use crate::config::types::MetadataSources;

/// Abstraction over "which plugins implement which verb for which kind".
///
/// The real implementation (landing in Chunk 5) inspects the plugin
/// registry's declared capabilities; tests use a mock that returns a fixed
/// set of supported plugin ids.
pub trait SourceCapabilityProbe: Send + Sync {
    fn supports(&self, plugin: &str, verb: MetadataVerb, kind_hint: &str) -> bool;
}

/// Ordered-source resolver — wraps the config-driven priority list and
/// filters it through a capability probe.
pub struct SourceResolver {
    config: MetadataSources,
    probe: Box<dyn SourceCapabilityProbe>,
}

impl SourceResolver {
    pub fn new(config: MetadataSources, probe: Box<dyn SourceCapabilityProbe>) -> Self {
        SourceResolver { config, probe }
    }

    /// `kind_hint` is a lowercase TUI-tab label: `"movies" | "series" | "anime" | "music"`.
    /// Unknown hints return an empty list (no sources).
    pub fn resolve(&self, verb: MetadataVerb, kind_hint: &str) -> Vec<String> {
        let priority: &[String] = match kind_hint {
            "movies" => &self.config.movies,
            "series" => &self.config.series,
            "anime" => &self.config.anime,
            "music" => &self.config.music,
            _ => return Vec::new(),
        };
        priority
            .iter()
            .filter(|p| self.probe.supports(p, verb, kind_hint))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_defaults() -> crate::config::types::MetadataSources {
        crate::config::types::MetadataSources::default()
    }

    struct MockProbe(Vec<String>);
    impl SourceCapabilityProbe for MockProbe {
        fn supports(&self, plugin: &str, _verb: MetadataVerb, _kind_hint: &str) -> bool {
            self.0.iter().any(|p| p == plugin)
        }
    }

    #[test]
    fn movies_returns_primary_then_fallbacks_filtered_by_capability() {
        let probe = MockProbe(vec!["tmdb".into(), "tvdb".into()]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Credits, "movies");
        assert_eq!(ordered, vec!["tmdb".to_string(), "tvdb".into()]);
    }

    #[test]
    fn anime_routes_to_anime_source_list_not_movies() {
        let probe = MockProbe(vec!["anilist".into(), "kitsu".into()]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Credits, "anime");
        assert_eq!(ordered, vec!["anilist".to_string(), "kitsu".into()]);
    }

    #[test]
    fn unknown_kind_hint_returns_empty() {
        let probe = MockProbe(vec!["tmdb".into()]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        assert!(r.resolve(MetadataVerb::Credits, "books").is_empty());
    }
}
