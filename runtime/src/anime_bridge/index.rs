//! In-memory cross-id index for anime entries, parsed from Fribb's
//! `anime-list-full.json`. The index is built once at startup (or on
//! refresh) and exposed via the `AnimeBridge`'s `lookup_by_*` methods.
//!
//! Each anime is represented by exactly one `Arc<AnimeRecord>` shared
//! across the per-id-type HashMaps. Lookups are O(1) and clone the
//! `Arc` (cheap).

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;

/// One row in Fribb's anime-list-full.json. Fribb's JSON uses
/// `themoviedb_id` (not `tmdb_id`); we rename via serde. The TVDB
/// id field is named `tvdb_id` directly in Fribb (verified against
/// the live dataset), so NO rename for that one. Numeric ids
/// (`mal_id`, `anilist_id`, etc.) come across as integers; `imdb_id`
/// is a string ("tt..."). All fields are optional — Fribb ships
/// entries with only some ids populated.
#[derive(Debug, Deserialize)]
struct RawFribbRecord {
    #[serde(default, deserialize_with = "id_from_num_or_str")]
    mal_id: Option<String>,
    #[serde(default, deserialize_with = "id_from_num_or_str")]
    anilist_id: Option<String>,
    #[serde(default, deserialize_with = "id_from_num_or_str")]
    kitsu_id: Option<String>,
    #[serde(default, deserialize_with = "id_from_num_or_str")]
    imdb_id: Option<String>,
    #[serde(default, rename = "themoviedb_id", deserialize_with = "id_from_num_or_str")]
    tmdb_id: Option<String>,
    // Note: Fribb's JSON key is `tvdb_id` (verified against the live
    // dataset 2026-04-26, 42k entries). NO rename needed — the field
    // name already matches. The reviewer caught an earlier draft that
    // mistakenly renamed to `thetvdb_id`, which would have silently
    // produced an empty `by_tvdb` index.
    #[serde(default, deserialize_with = "id_from_num_or_str")]
    tvdb_id: Option<String>,
}

/// Public record returned by `AnimeBridge::lookup_by_*`. Carries all
/// known cross-ids for one anime; consumers fill missing ids on a
/// `MediaEntry` from this record.
#[derive(Debug, Clone)]
pub struct AnimeRecord {
    pub mal_id:     Option<String>,
    pub anilist_id: Option<String>,
    pub kitsu_id:   Option<String>,
    pub imdb_id:    Option<String>,
    pub tmdb_id:    Option<String>,
    pub tvdb_id:    Option<String>,
}

/// Per-id-type indexes pointing at the same `Arc<AnimeRecord>`. Built
/// once at parse time and never mutated — refresh creates a fresh
/// `AnimeIndex` and atomic-swaps the parent `Arc<AnimeIndex>`.
pub struct AnimeIndex {
    pub(crate) by_mal:     HashMap<String, Arc<AnimeRecord>>,
    pub(crate) by_anilist: HashMap<String, Arc<AnimeRecord>>,
    pub(crate) by_kitsu:   HashMap<String, Arc<AnimeRecord>>,
    pub(crate) by_imdb:    HashMap<String, Arc<AnimeRecord>>,
    pub(crate) by_tmdb:    HashMap<String, Arc<AnimeRecord>>,
    pub(crate) by_tvdb:    HashMap<String, Arc<AnimeRecord>>,
}

impl AnimeIndex {
    /// Empty index — used as a fallback when the bundled snapshot fails
    /// to decompress or parse, so the bridge stays operational and
    /// degrades gracefully to milestone-α behaviour (no cross-tier
    /// merge, but search still works).
    pub fn empty() -> Self {
        Self {
            by_mal:     HashMap::new(),
            by_anilist: HashMap::new(),
            by_kitsu:   HashMap::new(),
            by_imdb:    HashMap::new(),
            by_tmdb:    HashMap::new(),
            by_tvdb:    HashMap::new(),
        }
    }

    /// Parse Fribb's `anime-list-full.json` byte stream into the
    /// per-id indexes. Records that have NO anime-tier id (mal /
    /// anilist / kitsu) are dropped — they're useless for our
    /// cross-tier dedup use case and pollute the western-id indexes.
    pub fn from_json(raw: &[u8]) -> anyhow::Result<Self> {
        let raw_records: Vec<RawFribbRecord> = serde_json::from_slice(raw)
            .map_err(|e| anyhow::anyhow!("Fribb top-level shape mismatch: {e}"))?;

        let mut by_mal     = HashMap::new();
        let mut by_anilist = HashMap::new();
        let mut by_kitsu   = HashMap::new();
        let mut by_imdb    = HashMap::new();
        let mut by_tmdb    = HashMap::new();
        let mut by_tvdb    = HashMap::new();

        for raw in raw_records {
            // Drop records with no anime-tier id — they're not useful
            // for the cross-tier collapse use case.
            let has_anime_tier =
                raw.mal_id.is_some() || raw.anilist_id.is_some() || raw.kitsu_id.is_some();
            if !has_anime_tier {
                continue;
            }

            let record = Arc::new(AnimeRecord {
                mal_id:     raw.mal_id.clone(),
                anilist_id: raw.anilist_id.clone(),
                kitsu_id:   raw.kitsu_id.clone(),
                imdb_id:    raw.imdb_id.clone(),
                tmdb_id:    raw.tmdb_id.clone(),
                tvdb_id:    raw.tvdb_id.clone(),
            });

            if let Some(id) = &raw.mal_id     { by_mal.insert(id.clone(),     Arc::clone(&record)); }
            if let Some(id) = &raw.anilist_id { by_anilist.insert(id.clone(), Arc::clone(&record)); }
            if let Some(id) = &raw.kitsu_id   { by_kitsu.insert(id.clone(),   Arc::clone(&record)); }
            if let Some(id) = &raw.imdb_id    { by_imdb.insert(id.clone(),    Arc::clone(&record)); }
            if let Some(id) = &raw.tmdb_id    { by_tmdb.insert(id.clone(),    Arc::clone(&record)); }
            if let Some(id) = &raw.tvdb_id    { by_tvdb.insert(id.clone(),    Arc::clone(&record)); }
        }

        Ok(Self {
            by_mal, by_anilist, by_kitsu,
            by_imdb, by_tmdb, by_tvdb,
        })
    }
}

/// Custom deserializer mirroring the `tmdb_id_from_num_or_str` pattern
/// already in `runtime/src/catalog.rs`. Accepts integer, string, or
/// null; coerces to `Option<String>`. Empty strings and zero are
/// treated as missing (Fribb sometimes ships `"mal_id": 0` or
/// `"imdb_id": ""` for entries where the upstream lookup failed).
fn id_from_num_or_str<'de, D>(de: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_json::Value::deserialize(de)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) if s.trim().is_empty() => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        serde_json::Value::Number(n) => {
            // Fribb uses 0 as a sentinel for "no upstream id". Treat as missing.
            if n.as_u64() == Some(0) || n.as_i64() == Some(0) {
                Ok(None)
            } else {
                Ok(Some(n.to_string()))
            }
        }
        _ => Err(D::Error::custom("expected string, integer, or null")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_one_full() -> &'static str {
        r#"[
            {
                "anidb_id": 1,
                "anilist_id": 1,
                "kitsu_id": 1,
                "mal_id": 1,
                "imdb_id": "tt0213338",
                "themoviedb_id": 30991,
                "tvdb_id": 76885
            }
        ]"#
    }

    #[test]
    fn from_json_parses_realistic_fribb_entry() {
        let idx = AnimeIndex::from_json(fixture_one_full().as_bytes()).unwrap();
        assert_eq!(idx.by_mal.len(), 1);
        assert_eq!(idx.by_anilist.len(), 1);
        assert_eq!(idx.by_kitsu.len(), 1);
        assert_eq!(idx.by_imdb.len(), 1);
        assert_eq!(idx.by_tmdb.len(), 1);
        assert_eq!(idx.by_tvdb.len(), 1);

        let r = idx.by_mal.get("1").unwrap();
        assert_eq!(r.mal_id.as_deref(), Some("1"));
        assert_eq!(r.imdb_id.as_deref(), Some("tt0213338"));
        assert_eq!(r.tmdb_id.as_deref(), Some("30991"));
        assert_eq!(r.tvdb_id.as_deref(), Some("76885"));
    }

    #[test]
    fn from_json_filters_entries_with_no_anime_tier_id() {
        let raw = r#"[
            { "imdb_id": "tt0001", "themoviedb_id": 100 },
            { "mal_id": 1, "imdb_id": "tt0002" }
        ]"#;
        let idx = AnimeIndex::from_json(raw.as_bytes()).unwrap();
        // Only the second record (has mal_id) should index.
        assert_eq!(idx.by_imdb.len(), 1);
        assert_eq!(idx.by_imdb.get("tt0002").map(|r| r.imdb_id.as_deref().unwrap()), Some("tt0002"));
        assert!(idx.by_imdb.get("tt0001").is_none());
    }

    #[test]
    fn from_json_handles_missing_optional_ids() {
        let raw = r#"[{ "mal_id": 1, "imdb_id": "tt0001" }]"#;
        let idx = AnimeIndex::from_json(raw.as_bytes()).unwrap();
        assert_eq!(idx.by_mal.len(), 1);
        assert_eq!(idx.by_imdb.len(), 1);
        assert_eq!(idx.by_anilist.len(), 0);
        assert_eq!(idx.by_kitsu.len(), 0);
        assert_eq!(idx.by_tmdb.len(), 0);
        assert_eq!(idx.by_tvdb.len(), 0);
    }

    #[test]
    fn from_json_dedups_within_a_single_record() {
        let idx = AnimeIndex::from_json(fixture_one_full().as_bytes()).unwrap();
        // Same record indexed under all 6 id types must point at one Arc.
        let r1 = Arc::clone(idx.by_mal.get("1").unwrap());
        let r2 = Arc::clone(idx.by_imdb.get("tt0213338").unwrap());
        assert!(Arc::ptr_eq(&r1, &r2), "all index entries for one record must share the same Arc");
    }

    #[test]
    fn from_json_treats_zero_and_empty_string_as_missing() {
        let raw = r#"[
            { "mal_id": 1, "imdb_id": "", "themoviedb_id": 0, "tvdb_id": 100 }
        ]"#;
        let idx = AnimeIndex::from_json(raw.as_bytes()).unwrap();
        assert_eq!(idx.by_mal.len(), 1);
        assert_eq!(idx.by_imdb.len(), 0); // empty string -> missing
        assert_eq!(idx.by_tmdb.len(), 0); // 0 -> missing
        assert_eq!(idx.by_tvdb.len(), 1); // 100 -> kept
    }

    #[test]
    fn from_json_tolerates_unexpected_fields() {
        // Fribb periodically adds new id types. Our struct must ignore them.
        let raw = r#"[{ "mal_id": 1, "notify_moe_id": "abc", "anisearch_id": 99, "livechart_id": 42 }]"#;
        let idx = AnimeIndex::from_json(raw.as_bytes()).unwrap();
        assert_eq!(idx.by_mal.len(), 1);
    }

    #[test]
    fn from_json_returns_err_on_top_level_shape_change() {
        // Fribb might one day ship an object instead of an array. Our parser
        // strict-fails so the caller can fall back to `empty()`.
        let raw = r#"{ "data": [] }"#;
        let res = AnimeIndex::from_json(raw.as_bytes());
        assert!(res.is_err());
    }

    #[test]
    fn empty_returns_zero_sized_index() {
        let idx = AnimeIndex::empty();
        assert_eq!(idx.by_mal.len(), 0);
        assert!(idx.by_imdb.get("tt0001").is_none());
    }
}
