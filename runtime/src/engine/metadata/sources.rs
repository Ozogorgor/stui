//! [`SourceResolver`] — picks capable plugins per (verb, kind).
//!
//! Given a [`MetadataVerb`] (enrich / credits / artwork / related) and a
//! lowercase kind-hint (`"movies" | "series" | "anime" | "music"`), the
//! resolver returns an ordered `Vec<String>` of plugin ids to consult —
//! the user's priority list first, then any auto-discovered plugin whose
//! manifest tags it for the kind, minus anything in the user's per-kind
//! disabled list.
//!
//! Auto-discovery is what makes new plugin install zero-config: drop a
//! plugin into `~/.config/stui/plugins/`, mark it with the appropriate
//! `tags = ["movies"]` (or `series`/`anime`/`music`) in its plugin.toml,
//! and it joins the fan-out automatically. The user can still curate via
//! the priority list (preferred order) and the disabled list (opt-out).
//!
//! Unknown kind hints return an empty list (no sources).

use crate::cache::metadata_key::MetadataVerb;
use crate::config::types::MetadataSources;

/// Abstraction over "which plugins implement which verb for which kind".
///
/// The host implementation ([`super::dispatch::ManifestCapabilityProbe`])
/// snapshots the plugin registry at metadata-dispatch construction; tests
/// use a mock that returns a fixed set.
pub trait SourceCapabilityProbe: Send + Sync {
    /// Returns true when `plugin` advertises support for `verb` on `kind_hint`.
    /// Used to filter the user's priority list before fan-out.
    fn supports(&self, plugin: &str, verb: MetadataVerb, kind_hint: &str) -> bool;

    /// Returns every plugin in the registry that should auto-join the
    /// fan-out for (verb, kind_hint). The host probe filters by
    /// manifest `tags` containing the kind-hint string and by per-verb
    /// capability declarations. Order is unspecified — the resolver
    /// preserves the user's priority list and appends discovered
    /// plugins after.
    fn discover(&self, verb: MetadataVerb, kind_hint: &str) -> Vec<String>;
}

/// Ordered-source resolver — wraps the config-driven priority + disabled
/// lists and unions them with manifest-tagged auto-discovery.
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
    ///
    /// Order:
    ///   1. Plugins from the user's priority list — preserved as-is, but
    ///      filtered by `probe.supports()` and the disabled list.
    ///   2. Plugins from `probe.discover()` that aren't already in the
    ///      priority list and aren't disabled — appended in probe order.
    ///
    /// This means a freshly-installed plugin tagged for the kind shows
    /// up at the end of the fan-out without the user touching
    /// `runtime.toml`. To pin it earlier, the user adds it to the
    /// priority list. To exclude it, they add it to the disabled list.
    pub fn resolve(&self, verb: MetadataVerb, kind_hint: &str) -> Vec<String> {
        let (priority, disabled): (&[String], &[String]) = match kind_hint {
            "movies" => (&self.config.movies, &self.config.movies_disabled),
            "series" => (&self.config.series, &self.config.series_disabled),
            "anime"  => (&self.config.anime,  &self.config.anime_disabled),
            "music"  => (&self.config.music,  &self.config.music_disabled),
            _ => return Vec::new(),
        };

        // Cheap-on-string-eq lookup helpers for both lists. `Vec.contains`
        // is O(n) per check but n is tiny (single-digit plugins per kind).
        let is_disabled = |id: &str| disabled.iter().any(|d| d == id);
        let in_priority = |id: &str| priority.iter().any(|p| p == id);

        let mut result: Vec<String> = priority
            .iter()
            .filter(|p| !is_disabled(p) && self.probe.supports(p, verb, kind_hint))
            .cloned()
            .collect();

        for discovered in self.probe.discover(verb, kind_hint) {
            if !in_priority(&discovered) && !is_disabled(&discovered) {
                result.push(discovered);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_defaults() -> crate::config::types::MetadataSources {
        crate::config::types::MetadataSources::default()
    }

    /// Mock probe: `supports()` returns true for any name in the supported
    /// list; `discover()` returns the discoverable list verbatim. Tests
    /// configure both independently.
    struct MockProbe {
        supported: Vec<String>,
        discoverable: Vec<String>,
    }
    impl MockProbe {
        fn new(supported: Vec<&str>) -> Self {
            MockProbe {
                supported: supported.into_iter().map(String::from).collect(),
                discoverable: Vec::new(),
            }
        }
        fn with_discoverable(mut self, discoverable: Vec<&str>) -> Self {
            self.discoverable = discoverable.into_iter().map(String::from).collect();
            self
        }
    }
    impl SourceCapabilityProbe for MockProbe {
        fn supports(&self, plugin: &str, _verb: MetadataVerb, _kind_hint: &str) -> bool {
            self.supported.iter().any(|p| p == plugin)
        }
        fn discover(&self, _verb: MetadataVerb, _kind_hint: &str) -> Vec<String> {
            self.discoverable.clone()
        }
    }

    #[test]
    fn movies_returns_primary_then_fallbacks_filtered_by_capability() {
        let probe = MockProbe::new(vec!["tmdb", "tvdb"]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Credits, "movies");
        assert_eq!(ordered, vec!["tmdb".to_string(), "tvdb".into()]);
    }

    #[test]
    fn anime_routes_to_anime_source_list_not_movies() {
        let probe = MockProbe::new(vec!["anilist", "kitsu"]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Credits, "anime");
        assert_eq!(ordered, vec!["anilist".to_string(), "kitsu".into()]);
    }

    #[test]
    fn unknown_kind_hint_returns_empty() {
        let probe = MockProbe::new(vec!["tmdb"]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        assert!(r.resolve(MetadataVerb::Credits, "books").is_empty());
    }

    #[test]
    fn discovered_plugins_appended_after_priority_list() {
        // A user with default priority [tmdb, omdb, tvdb] installs a
        // hypothetical "letterboxd-rating" plugin tagged for movies. It
        // should show up at the end of the fan-out without any toml
        // edit.
        let probe = MockProbe::new(vec!["tmdb", "omdb", "tvdb", "letterboxd"])
            .with_discoverable(vec!["letterboxd"]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Enrich, "movies");
        assert_eq!(
            ordered,
            vec!["tmdb".to_string(), "omdb".into(), "tvdb".into(), "letterboxd".into()]
        );
    }

    #[test]
    fn discovered_plugins_skipped_when_already_in_priority_list() {
        // No duplicates: priority list takes precedence; discover() can
        // surface the same plugin too without breaking ordering.
        let probe = MockProbe::new(vec!["tmdb", "omdb", "tvdb"])
            .with_discoverable(vec!["tmdb", "omdb", "tvdb"]);
        let r = SourceResolver::new(cfg_with_defaults(), Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Enrich, "movies");
        assert_eq!(ordered, vec!["tmdb".to_string(), "omdb".into(), "tvdb".into()]);
    }

    #[test]
    fn disabled_plugin_excluded_from_priority_list() {
        let probe = MockProbe::new(vec!["tmdb", "omdb", "tvdb"]);
        let mut cfg = cfg_with_defaults();
        cfg.movies_disabled = vec!["omdb".into()];
        let r = SourceResolver::new(cfg, Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Enrich, "movies");
        assert_eq!(ordered, vec!["tmdb".to_string(), "tvdb".into()]);
    }

    #[test]
    fn disabled_plugin_excluded_from_discovered_list() {
        // User installed a plugin and explicitly opted it out via the
        // disabled list. Even though discover() would surface it, it
        // must not appear in the result.
        let probe = MockProbe::new(vec!["tmdb", "letterboxd"])
            .with_discoverable(vec!["letterboxd"]);
        let mut cfg = cfg_with_defaults();
        cfg.movies_disabled = vec!["letterboxd".into()];
        let r = SourceResolver::new(cfg, Box::new(probe));
        let ordered = r.resolve(MetadataVerb::Enrich, "movies");
        // tmdb stays from priority defaults; letterboxd discovery is suppressed.
        assert!(ordered.contains(&"tmdb".to_string()));
        assert!(!ordered.contains(&"letterboxd".to_string()));
    }
}
