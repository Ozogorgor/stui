//! Pre-dedup enrichment helper. Called from `search_catalog_entries`
//! (catalog/recommendation grid path) and `run_one_scope` (scoped
//! music-tab search path) before the merge functions run, so the
//! `dedup_key` precedence collapses cross-tier dupes that were
//! previously stranded on different keys.
//!
//! Looks up the bridge by any of mal/imdb/tmdb/anilist/kitsu ids and
//! fills the missing peers on the entry. Western-tier ids (imdb,
//! tmdb) feeding Series-tab entries enables the spine-merge path —
//! cours of the same show share a parent series' tmdb_id in Fribb,
//! so all cours collapse into one bucket once enriched.
//!
//! Defensive: never overwrites a value the provider already supplied.

use crate::anime_bridge::{AnimeBridge, AnimeRecord};
use crate::ipc::v1::MediaEntry;
use std::sync::Arc;

/// Look up the bridge for `entry` using whichever foreign id is
/// present; fill any missing ids on `entry` from the bridge's record.
/// Idempotent — running twice is a no-op the second time.
///
/// Lookup order (first-hit-wins):
///   1. mal_id      → bridge.lookup_by_mal
///   2. anilist_id  → bridge.lookup_by_anilist
///   3. kitsu_id    → bridge.lookup_by_kitsu
///   4. imdb_id     → bridge.lookup_by_imdb
///   5. tmdb_id     → bridge.lookup_by_tmdb
///
/// Anime-tier ids come first because they're more discriminating —
/// kitsu's catalog search omits MAL mappings for some entries, but
/// the kitsu_id itself is always present and lets us pull the parent
/// series' tmdb_id from Fribb. Without anilist/kitsu lookups, the
/// "Sousou no Frieren" Kitsu entry stayed at title:year dedup and
/// failed to collapse with the AniList cours that had bridge-set
/// tmdb_id.
///
/// If no foreign id is present, or none resolves in the bridge,
/// the entry is left unchanged.
pub fn enrich_entry(entry: &mut MediaEntry, bridge: &AnimeBridge) {
    let record: Option<Arc<AnimeRecord>> =
        entry.mal_id.as_deref().and_then(|id| bridge.lookup_by_mal(id))
        .or_else(|| entry.anilist_id.as_deref().and_then(|id| bridge.lookup_by_anilist(id)))
        .or_else(|| entry.kitsu_id.as_deref().and_then(|id| bridge.lookup_by_kitsu(id)))
        .or_else(|| entry.imdb_id.as_deref().and_then(|id| bridge.lookup_by_imdb(id)))
        .or_else(|| entry.tmdb_id.as_deref().and_then(|id| bridge.lookup_by_tmdb(id)));

    let Some(r) = record else { return };

    // Fill ONLY missing fields. Provider-supplied values always win
    // (defensive — never trust the bridge's data over a provider's
    // own).
    if entry.mal_id.is_none()     { entry.mal_id     = r.mal_id.clone(); }
    if entry.anilist_id.is_none() { entry.anilist_id = r.anilist_id.clone(); }
    if entry.kitsu_id.is_none()   { entry.kitsu_id   = r.kitsu_id.clone(); }
    if entry.imdb_id.is_none()    { entry.imdb_id    = r.imdb_id.clone(); }
    if entry.tmdb_id.is_none()    { entry.tmdb_id    = r.tmdb_id.clone(); }
}

/// Spine selector consulted by both merge functions
/// (`search_scoped::merge_dedupe` and `catalog_engine::merge_group`).
///
/// When the dedup key starts with `mal:` the merge is anime-tier
/// (collapsed via MAL); the spine should be the anime-tier provider
/// that ships richer anime metadata (English titles, anime-specific
/// genres, normalized ratings).
///
/// For all other keys (`imdb:`, `title:`) the existing α priority
/// applies — TMDB / TVDB / OMDb are the right spines for western
/// titles.
pub fn provider_priority_for_key(provider: &str, key: &str) -> u8 {
    if key.starts_with("mal:") {
        // Anime-tier merge: AniList > Kitsu > western tier.
        match provider {
            "anilist" => 0,
            "kitsu"   => 1,
            "tvdb"    => 2,
            "tmdb"    => 3,
            "omdb"    => 4,
            _         => 5,
        }
    } else {
        // Existing α priority for western-tier and title-fallback merges.
        match provider {
            "tmdb"           => 0,
            "tvdb"           => 1,
            "xmdb"           => 2,   // beats omdb for IMDb id + ratings
            "rottentomatoes" => 3,   // NEW — beats omdb for IMDb-keyed merges
            "omdb"           => 4,
            "anilist"        => 5,
            "kitsu"          => 6,
            _                => 7,
        }
    }
}

#[cfg(test)]
mod priority_tests {
    use super::provider_priority_for_key;

    #[test]
    fn mal_keyed_prioritizes_anilist() {
        assert!(
            provider_priority_for_key("anilist", "mal:1")
                < provider_priority_for_key("tvdb", "mal:1"),
        );
        assert!(
            provider_priority_for_key("kitsu", "mal:1")
                < provider_priority_for_key("tmdb", "mal:1"),
        );
    }

    #[test]
    fn imdb_keyed_prioritizes_tmdb_over_anilist() {
        assert!(
            provider_priority_for_key("tmdb", "imdb:tt0001")
                < provider_priority_for_key("anilist", "imdb:tt0001"),
        );
    }

    #[test]
    fn title_keyed_prioritizes_tmdb() {
        assert!(
            provider_priority_for_key("tmdb", "title:foo:2024")
                < provider_priority_for_key("anilist", "title:foo:2024"),
        );
    }

    #[test]
    fn provider_priority_xmdb_beats_omdb_in_western_tier() {
        let xmdb = provider_priority_for_key("xmdb", "imdb:tt1");
        let omdb = provider_priority_for_key("omdb", "imdb:tt1");
        assert!(xmdb < omdb, "xmdb={xmdb} should beat omdb={omdb}");
    }

    #[test]
    fn provider_priority_anime_tier_unaffected_by_xmdb() {
        // xmdb has no MAL bridge — falls into the catch-all bucket on
        // the anime arm. AniList and Kitsu must still beat it.
        let xmdb = provider_priority_for_key("xmdb", "mal:1");
        let anilist = provider_priority_for_key("anilist", "mal:1");
        let kitsu = provider_priority_for_key("kitsu", "mal:1");
        assert!(anilist < xmdb, "anilist={anilist} should beat xmdb={xmdb}");
        assert!(kitsu < xmdb, "kitsu={kitsu} should beat xmdb={xmdb}");
    }

    #[test]
    fn provider_priority_rottentomatoes_beats_omdb() {
        let rt   = provider_priority_for_key("rottentomatoes", "imdb:tt1");
        let omdb = provider_priority_for_key("omdb", "imdb:tt1");
        assert!(rt < omdb, "rt={rt} should beat omdb={omdb}");
    }

    #[test]
    fn provider_priority_rottentomatoes_loses_to_xmdb() {
        let rt   = provider_priority_for_key("rottentomatoes", "imdb:tt1");
        let xmdb = provider_priority_for_key("xmdb", "imdb:tt1");
        assert!(xmdb < rt, "xmdb={xmdb} should beat rottentomatoes={rt}");
    }

    #[test]
    fn provider_priority_rottentomatoes_anime_arm_unchanged() {
        let rt = provider_priority_for_key("rottentomatoes", "mal:1");
        let anilist = provider_priority_for_key("anilist", "mal:1");
        assert!(anilist < rt, "anilist={anilist} should beat rt={rt} on mal: keys");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anime_bridge::index::{AnimeIndex, AnimeRecord};
    use std::collections::HashMap;
    use std::sync::Arc;
    use arc_swap::ArcSwap;

    /// Build a bridge with one canned record under specified id keys.
    fn bridge_with(record: AnimeRecord) -> Arc<AnimeBridge> {
        let r = Arc::new(record);
        let mut by_mal     = HashMap::new();
        let mut by_anilist = HashMap::new();
        let mut by_kitsu   = HashMap::new();
        let mut by_imdb    = HashMap::new();
        let mut by_tmdb    = HashMap::new();
        let mut by_tvdb    = HashMap::new();
        if let Some(id) = &r.mal_id     { by_mal.insert(id.clone(),     Arc::clone(&r)); }
        if let Some(id) = &r.anilist_id { by_anilist.insert(id.clone(), Arc::clone(&r)); }
        if let Some(id) = &r.kitsu_id   { by_kitsu.insert(id.clone(),   Arc::clone(&r)); }
        if let Some(id) = &r.imdb_id    { by_imdb.insert(id.clone(),    Arc::clone(&r)); }
        if let Some(id) = &r.tmdb_id    { by_tmdb.insert(id.clone(),    Arc::clone(&r)); }
        if let Some(id) = &r.tvdb_id    { by_tvdb.insert(id.clone(),    Arc::clone(&r)); }
        let idx = AnimeIndex { by_mal, by_anilist, by_kitsu, by_imdb, by_tmdb, by_tvdb };
        // Construct AnimeBridge directly (bypass `new()` which loads
        // the bundled snapshot) so this test isolates `enrich_entry`'s
        // behaviour from the bundled data.
        Arc::new(AnimeBridge {
            index: ArcSwap::from(Arc::new(idx)),
        })
    }

    fn aot_record() -> AnimeRecord {
        AnimeRecord {
            mal_id:     Some("16498".into()),
            anilist_id: Some("16498".into()),
            kitsu_id:   Some("7442".into()),
            imdb_id:    Some("tt2560140".into()),
            tmdb_id:    Some("1429".into()),
            tvdb_id:    Some("267440".into()),
        }
    }

    fn make_entry(provider: &str) -> MediaEntry {
        MediaEntry {
            id: format!("{provider}-test"),
            title: "Attack on Titan".into(),
            provider: provider.into(),
            ..Default::default()
        }
    }

    #[test]
    fn enriches_anilist_entry_with_western_ids() {
        let bridge = bridge_with(aot_record());
        let mut e = make_entry("anilist");
        e.mal_id = Some("16498".into());
        enrich_entry(&mut e, &bridge);
        assert_eq!(e.mal_id.as_deref(),  Some("16498"));
        assert_eq!(e.imdb_id.as_deref(), Some("tt2560140"));
        assert_eq!(e.tmdb_id.as_deref(), Some("1429"));
    }

    #[test]
    fn enriches_tvdb_entry_with_anime_ids() {
        let bridge = bridge_with(aot_record());
        let mut e = make_entry("tvdb");
        e.imdb_id = Some("tt2560140".into());
        enrich_entry(&mut e, &bridge);
        assert_eq!(e.mal_id.as_deref(),  Some("16498"));
        assert_eq!(e.imdb_id.as_deref(), Some("tt2560140"));
        assert_eq!(e.tmdb_id.as_deref(), Some("1429"));
    }

    #[test]
    fn does_not_overwrite_existing_ids() {
        let bridge = bridge_with(aot_record());
        let mut e = make_entry("custom");
        e.imdb_id = Some("tt9999999".into()); // intentionally wrong
        e.tmdb_id = Some("99999".into());
        e.mal_id  = Some("16498".into()); // present
        enrich_entry(&mut e, &bridge);
        // Existing imdb/tmdb values preserved despite bridge's record.
        assert_eq!(e.imdb_id.as_deref(), Some("tt9999999"));
        assert_eq!(e.tmdb_id.as_deref(), Some("99999"));
        assert_eq!(e.mal_id.as_deref(),  Some("16498"));
    }

    #[test]
    fn noop_for_entry_with_no_foreign_ids() {
        let bridge = bridge_with(aot_record());
        let mut e = make_entry("tmdb");
        e.title = "Random Movie".into();
        let before = e.clone();
        enrich_entry(&mut e, &bridge);
        assert_eq!(e.mal_id, before.mal_id);
        assert_eq!(e.imdb_id, before.imdb_id);
        assert_eq!(e.tmdb_id, before.tmdb_id);
    }

    #[test]
    fn noop_for_non_anime_entry() {
        // Entry has imdb_id, but bridge has no record under that id.
        let bridge = bridge_with(aot_record());
        let mut e = make_entry("omdb");
        e.imdb_id = Some("tt0111161".into()); // Shawshank — not in bridge
        let before = e.clone();
        enrich_entry(&mut e, &bridge);
        assert_eq!(e.mal_id, before.mal_id);
        assert_eq!(e.imdb_id, before.imdb_id);
    }

    #[test]
    fn idempotent() {
        let bridge = bridge_with(aot_record());
        let mut e = make_entry("anilist");
        e.mal_id = Some("16498".into());
        enrich_entry(&mut e, &bridge);
        let after_first = e.clone();
        enrich_entry(&mut e, &bridge);
        assert_eq!(e.mal_id, after_first.mal_id);
        assert_eq!(e.imdb_id, after_first.imdb_id);
        assert_eq!(e.tmdb_id, after_first.tmdb_id);
    }
}
