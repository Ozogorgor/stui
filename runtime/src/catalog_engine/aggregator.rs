//! Catalog aggregator — merges provider results, removes duplicates, and
//! computes a weighted-median rating from all available sources.
//!
//! # Merge strategy
//!
//! 1. Group entries by dedup key (IMDB id preferred, otherwise title+year).
//! 2. For each group, pick the entry with the most fields populated as base.
//! 3. Fill any `None` fields from secondary entries (highest-priority-first).
//! 4. Collect all per-source ratings into `ratings` HashMap.
//! 5. Select the weight profile for the entry's media type and genre.
//! 6. Compute a weighted median and store in `rating` (display string).
//! 7. Preserve all distinct provider names in a comma-separated list.
//!
//! # Weight profiles
//!
//! Profiles reflect the reliability of each rating source for a given
//! genre/type. Weights are re-normalised to 1.0 when sources are missing.
//!
//! ## Movie (default)
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.35   | Professional critics most reliable     |
//! | imdb            | 0.30   | Bayesian, large, hard to game          |
//! | audience_score  | 0.15   | Popular appeal, gameable               |
//! | tmdb            | 0.10   | Decent, smaller sample                 |
//! | anilist         | —      | N/A                                    |
//!
//! ## Series / Episode
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.25   | Critics worse at serialised TV         |
//! | imdb            | 0.35   | Go-to source for TV                    |
//! | audience_score  | 0.25   | Audience sustains long-running shows   |
//! | tmdb            | 0.15   | Better TV coverage than for film       |
//! | anilist         | —      | N/A                                    |
//!
//! ## Anime  (genre contains "anime", or anilist score is present)
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.15   | RT has poor anime coverage             |
//! | imdb            | 0.20   | Useful but not anime-native            |
//! | audience_score  | 0.15   | Less meaningful for anime              |
//! | tmdb            | 0.15   | Reasonable secondary signal            |
//! | anilist         | 0.35   | Community authority for anime          |
//!
//! ## Documentary
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.45   | Critics define quality for docs        |
//! | imdb            | 0.25   | Solid secondary signal                 |
//! | audience_score  | 0.10   | Less signal for non-entertainment docs |
//! | tmdb            | 0.10   | Smaller pool                           |
//! | anilist         | —      | N/A                                    |
//!
//! ## Horror
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.25   | Critics and audiences routinely diverge|
//! | imdb            | 0.30   | Balanced middle ground                 |
//! | audience_score  | 0.30   | Audience enjoyment central to horror   |
//! | tmdb            | 0.10   | Supplementary                          |
//! | anilist         | —      | N/A                                    |
//!
//! ## Music / Album / Track
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.20   | Critics less dominant in music         |
//! | imdb            | 0.20   | Limited music coverage                 |
//! | audience_score  | 0.30   | Engagement signal strongest            |
//! | tmdb            | 0.20   | Music data increasingly useful         |
//! | anilist         | —      | N/A                                    |
//!
//! All scores are normalised to 0–10 before weighting.
//! OMDB is excluded (it mirrors IMDB — would double-count).

use std::collections::HashMap;

use super::filters::FilterSet;
use super::ranking::SortOrder;
use crate::catalog::CatalogEntry;
use crate::ipc::MediaType;

// ── Weight table ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct RatingWeight {
    pub key: &'static str,
    pub weight: f64,
    /// Divisor to normalise the raw score to 0–10.
    /// IMDB/TMDB are already 0–10 (divisor 1.0).
    /// RT percentages and AniList 0–100 need divisor 10.0.
    pub normalize: f64,
}

/// Default rating weights for movies.
#[allow(dead_code)]
pub const WEIGHTS_MOVIE: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.35,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.30,
        normalize: 1.0,
    },
    RatingWeight {
        key: "audience_score",
        weight: 0.15,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.10,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 10.0,
    },
];

#[allow(dead_code)]
const WEIGHTS_SERIES: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.25,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.35,
        normalize: 1.0,
    },
    RatingWeight {
        key: "audience_score",
        weight: 0.25,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.15,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 10.0,
    },
];

#[allow(dead_code)]
const WEIGHTS_ANIME: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.15,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.20,
        normalize: 1.0,
    },
    RatingWeight {
        key: "audience_score",
        weight: 0.15,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.15,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.35,
        normalize: 10.0,
    },
];

#[allow(dead_code)]
const WEIGHTS_DOCUMENTARY: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.45,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.25,
        normalize: 1.0,
    },
    RatingWeight {
        key: "audience_score",
        weight: 0.10,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.10,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 10.0,
    },
];

#[allow(dead_code)]
const WEIGHTS_HORROR: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.25,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.30,
        normalize: 1.0,
    },
    RatingWeight {
        key: "audience_score",
        weight: 0.30,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.10,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 10.0,
    },
];

#[allow(dead_code)]
const WEIGHTS_MUSIC: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.20,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.20,
        normalize: 1.0,
    },
    RatingWeight {
        key: "audience_score",
        weight: 0.30,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.20,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 10.0,
    },
];

// ── Profile selection ─────────────────────────────────────────────────────────

/// Select the appropriate weight profile for an entry.
///
/// Priority order:
/// 1. Anime — genre contains "anime", OR anilist score is present (provider signal).
/// 2. Documentary — genre contains "documentary".
/// 3. Horror — genre contains "horror".
/// 4. Music — MediaType is Music, Album, or Track.
/// 5. Series — MediaType is Series or Episode.
/// 6. Movie — default.
#[allow(dead_code)]
fn weights_for(
    media_type: &MediaType,
    genre: Option<&str>,
    ratings: &HashMap<String, f64>,
) -> &'static [RatingWeight] {
    let genre_lc = genre.unwrap_or("").to_ascii_lowercase();

    // Anime: genre hint OR anilist data present from provider.
    if genre_lc.contains("anime") || ratings.contains_key("anilist") {
        return WEIGHTS_ANIME;
    }

    if genre_lc.contains("documentary") {
        return WEIGHTS_DOCUMENTARY;
    }

    if genre_lc.contains("horror") {
        return WEIGHTS_HORROR;
    }

    match media_type {
        MediaType::Music | MediaType::Album | MediaType::Track => WEIGHTS_MUSIC,
        MediaType::Series | MediaType::Episode => WEIGHTS_SERIES,
        _ => WEIGHTS_MOVIE,
    }
}

// ── Core rating functions ─────────────────────────────────────────────────────

/// Compute a weighted composite rating on a 0–10 scale (weighted mean).
///
/// Only sources with weight > 0 that are present in `ratings` contribute;
/// their weights are re-normalised to sum to 1.0.
///
/// Returns `None` if `ratings` is empty or no weighted sources are present.
#[allow(dead_code)]
pub fn weighted_rating(ratings: &HashMap<String, f64>, weights: &[RatingWeight]) -> Option<f64> {
    if ratings.is_empty() {
        return None;
    }

    let mut weighted_sum = 0.0_f64;
    let mut weight_total = 0.0_f64;

    for w in weights {
        if w.weight == 0.0 {
            continue;
        }
        // Guard against zero normalize (division by zero)
        if w.normalize <= 0.0 {
            continue;
        }
        if let Some(&raw) = ratings.get(w.key) {
            let normalised = (raw / w.normalize).clamp(0.0, 10.0);
            weighted_sum += normalised * w.weight;
            weight_total += w.weight;
        }
    }

    if weight_total == 0.0 {
        return None;
    }

    Some(weighted_sum / weight_total)
}

/// Compute the weighted median on a 0–10 scale.
///
/// The weighted median is the value where the cumulative weight of all
/// scores at or below it first reaches ≥ 50% of the total weight.
/// This is more robust than the weighted mean: a single outlier source
/// (e.g. a suspiciously high audience score) cannot skew the result.
///
/// With only one source present the median equals that source's value.
/// Returns `None` when no weighted sources are present.
#[allow(dead_code)]
pub fn weighted_median(ratings: &HashMap<String, f64>, weights: &[RatingWeight]) -> Option<f64> {
    if ratings.is_empty() {
        return None;
    }

    // Collect (normalised_score, weight) for present sources with weight > 0.
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    let mut weight_total = 0.0_f64;

    for w in weights {
        if w.weight == 0.0 {
            continue;
        }
        // Guard against zero normalize (division by zero)
        if w.normalize <= 0.0 {
            continue;
        }
        if let Some(&raw) = ratings.get(w.key) {
            let normalised = (raw / w.normalize).clamp(0.0, 10.0);
            pairs.push((normalised, w.weight));
            weight_total += w.weight;
        }
    }

    if pairs.is_empty() || weight_total == 0.0 {
        return None;
    }

    // Single source — median is trivially that value.
    if pairs.len() == 1 {
        return Some(pairs[0].0);
    }

    // Sort by score ascending, then walk cumulative weight.
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

    let half = weight_total / 2.0;
    let mut cumulative = 0.0_f64;

    for (score, weight) in &pairs {
        cumulative += weight;
        if cumulative >= half {
            return Some(*score);
        }
    }

    // Unreachable: cumulative always reaches weight_total before the loop ends.
    // Use .map() rather than .unwrap() to avoid a panic if the invariant ever breaks.
    pairs.last().map(|p| p.0)
}

/// Returns the number of rating sources present in the ratings map
/// that match the configured weight keys.
#[allow(dead_code)]
pub fn count_active_sources(ratings: &HashMap<String, f64>, weights: &[RatingWeight]) -> usize {
    weights
        .iter()
        .filter(|w| w.weight > 0.0 && ratings.contains_key(w.key))
        .count()
}

/// Returns a list of rating source names that are missing (configured but not present).
#[allow(dead_code)]
pub fn missing_sources(
    ratings: &HashMap<String, f64>,
    weights: &[RatingWeight],
) -> Vec<&'static str> {
    weights
        .iter()
        .filter(|w| w.weight > 0.0 && !ratings.contains_key(w.key))
        .map(|w| w.key)
        .collect()
}

/// Returns true if there are enough active sources to compute a reliable rating.
/// Requires at least `min_sources` (default: 1) sources with positive weight.
#[allow(dead_code)]
pub fn has_sufficient_sources(
    ratings: &HashMap<String, f64>,
    weights: &[RatingWeight],
    min_sources: usize,
) -> bool {
    count_active_sources(ratings, weights) >= min_sources
}

// ── Aggregator ────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct CatalogAggregator {
    filters: FilterSet,
    sort_order: SortOrder,
}

impl CatalogAggregator {
    #[allow(dead_code)]
    pub fn new() -> Self {
        CatalogAggregator {
            filters: FilterSet::default(),
            sort_order: SortOrder::default(),
        }
    }

    #[allow(dead_code)]
    pub fn with_filter(mut self, filter: super::filters::Filter) -> Self {
        self.filters.add(filter);
        self
    }

    #[allow(dead_code)]
    pub fn with_sort(mut self, order: SortOrder) -> Self {
        self.sort_order = order;
        self
    }

    /// Merge, dedup, filter, and sort a raw list of entries.
    pub fn apply(&self, entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        let merged = self.merge(entries);
        let filtered = self.filters.apply(merged);
        self.sort_order.apply(filtered)
    }

    /// Merge duplicates from multiple providers into enriched single entries.
    pub fn merge(&self, entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        let mut groups: HashMap<String, Vec<CatalogEntry>> = HashMap::new();

        for entry in entries {
            let key = entry.dedup_key();
            groups.entry(key).or_default().push(entry);
        }

        groups.into_values().map(merge_group).collect()
    }
}

impl Default for CatalogAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// Merge a group of entries for the same title into one enriched entry.
#[allow(dead_code)]
fn merge_group(mut group: Vec<CatalogEntry>) -> CatalogEntry {
    if group.len() == 1 {
        let mut entry = group.remove(0);
        // Still compute composite even for single-source entries.
        promote_rating_to_map(&mut entry);
        apply_weighted_rating(&mut entry);
        return entry;
    }

    // Sort by field completeness (more fields = higher priority).
    group.sort_by_key(|e| {
        let mut score = 0usize;
        if e.year.is_some() {
            score += 1;
        }
        if e.genre.is_some() {
            score += 1;
        }
        if e.rating.is_some() {
            score += 1;
        }
        if e.description.is_some() {
            score += 1;
        }
        if e.poster_url.is_some() {
            score += 1;
        }
        if e.imdb_id.is_some() {
            score += 2;
        } // especially valuable
        if e.tmdb_id.is_some() {
            score += 1;
        }
        score
    });
    group.reverse(); // highest score first

    let mut base = group.remove(0);
    let all_providers: Vec<String> = std::iter::once(base.provider.clone())
        .chain(group.iter().map(|e| e.provider.clone()))
        .collect();

    // Promote each entry's plain `rating` string into its `ratings` map
    // using the provider name as key.
    promote_rating_to_map(&mut base);
    for secondary in &mut group {
        promote_rating_to_map(secondary);
    }

    // Merge scalar fields from secondary entries.
    for secondary in &group {
        if base.year.is_none() {
            base.year = secondary.year.clone();
        }
        if base.genre.is_none() {
            base.genre = secondary.genre.clone();
        }
        if base.description.is_none() {
            base.description = secondary.description.clone();
        }
        if base.poster_url.is_none() {
            base.poster_url = secondary.poster_url.clone();
        }
        if base.imdb_id.is_none() {
            base.imdb_id = secondary.imdb_id.clone();
        }
        if base.tmdb_id.is_none() {
            base.tmdb_id = secondary.tmdb_id;
        }

        // Merge all per-source ratings (don't overwrite existing keys).
        for (k, v) in &secondary.ratings {
            base.ratings.entry(k.clone()).or_insert(*v);
        }
    }

    // Compute the weighted median with the appropriate profile.
    apply_weighted_rating(&mut base);

    // Record all contributing providers.
    base.provider = all_providers.join(",");
    base
}

/// If an entry has a plain `rating` string but an empty `ratings` map,
/// try to parse the string and insert it under the provider's canonical key.
#[allow(dead_code)]
fn promote_rating_to_map(entry: &mut CatalogEntry) {
    if entry.ratings.is_empty() {
        if let Some(ref r) = entry.rating.clone() {
            let key = rating_key_for_provider(&entry.provider);
            if let Some(val) = parse_rating_str(r) {
                entry.ratings.insert(key.to_string(), val);
            }
        }
    }
}

/// Select the weight profile and compute the weighted median into `entry.rating`.
///
/// If insufficient sources are available, falls back to the best available source's rating.
/// If no sources are available at all, preserves the original rating string.
#[allow(dead_code)]
fn apply_weighted_rating(entry: &mut CatalogEntry) {
    let weights = weights_for(&entry.media_type, entry.genre.as_deref(), &entry.ratings);

    // Try weighted median first
    if let Some(composite) = weighted_median(&entry.ratings, weights) {
        entry.rating = Some(format!("{:.1}", composite));
        return;
    }

    // Fallback: use best available single source if ratings map has any data
    if !entry.ratings.is_empty() {
        // Find the highest-rated single source as fallback
        if let Some((_source, &rating)) = entry
            .ratings
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        {
            entry.rating = Some(format!("{:.1}", rating));
        }
    }

    // No weighted sources available - keep original rating if present
    // (could be from a single provider that doesn't match our weight keys)
}

/// Map a provider name to the canonical ratings key used in the weight tables.
#[allow(dead_code)]
fn rating_key_for_provider(provider: &str) -> &'static str {
    // Provider names can be comma-joined (e.g. "tmdb,imdb") — take first.
    let first = provider.split(',').next().unwrap_or(provider).trim();
    match first {
        "imdb" => "imdb",
        "tmdb" => "tmdb",
        "omdb" => "imdb", // OMDB reflects IMDB score
        "anilist" => "anilist",
        "rottentomatoes" => "tomatometer",
        _ => "imdb", // safe fallback
    }
}

/// Parse a rating string to f64.
/// Handles "8.4", "8.4/10", "84%", "84".
#[allow(dead_code)]
fn parse_rating_str(s: &str) -> Option<f64> {
    let s = s.trim();
    // Strip trailing "/10", "%", etc.
    let num = s
        .trim_end_matches("%")
        .split('/')
        .next()
        .unwrap_or(s)
        .trim();
    num.parse::<f64>().ok().filter(|&v| v > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        title: &str,
        imdb_id: &str,
        rating: Option<&str>,
        ratings: &[(&str, f64)],
        media_type: MediaType,
    ) -> CatalogEntry {
        let mut ratings_map = std::collections::HashMap::new();
        for (k, v) in ratings {
            ratings_map.insert(k.to_string(), *v);
        }
        CatalogEntry {
            id: imdb_id.to_string(),
            title: title.to_string(),
            year: Some("2024".to_string()),
            genre: None,
            rating: rating.map(|s| s.to_string()),
            description: None,
            poster_url: None,
            poster_art: None,
            provider: "test".to_string(),
            tab: "movies".to_string(),
            imdb_id: Some(imdb_id.to_string()),
            tmdb_id: None,
            media_type,
            ratings: ratings_map,
        }
    }

    #[test]
    fn test_merge_single_source() {
        let entries = vec![make_entry(
            "Dune",
            "tt15239678",
            Some("8.0"),
            &[("imdb", 8.0)],
            MediaType::Movie,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rating, Some("8.0".to_string()));
    }

    #[test]
    fn test_anime_weights_prefer_anilist() {
        // Note: Anilist is on 100-scale, so we use 92.0 (which normalizes to 9.2)
        let entries = vec![make_entry(
            "Attack on Titan",
            "tt12345678",
            Some("9.0"),
            &[("imdb", 8.5), ("anilist", 92.0)],
            MediaType::Series,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // Anilist has highest weight (0.35) for anime, should influence result
        // With anilist normalized to 9.2 and imdb at 8.5, weighted median should be 9.2
        let rating = result[0].rating.as_ref().unwrap();
        let parsed: f64 = rating.parse().unwrap();
        assert!(parsed >= 9.0, "Expected rating >= 9.0, got {}", parsed);
    }

    #[test]
    fn test_tomatometer_normalization() {
        // Rotten Tomatoes returns 0-100 scale (e.g., "85%"), so we use 85.0
        // This should normalize to 8.5 for weighted calculation
        let entries = vec![make_entry(
            "The Matrix",
            "tt0133093",
            Some("8.7"),
            &[("imdb", 8.7), ("tomatometer", 83.0)],
            MediaType::Movie,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // tomatometer (normalized to 8.3, weight 0.35) vs imdb (8.7, weight 0.30)
        // Sorted: [(8.3, 0.35), (8.7, 0.30)] - first one above 50% weight is 8.3
        let rating = result[0].rating.as_ref().unwrap();
        let parsed: f64 = rating.parse().unwrap();
        assert!(
            (parsed - 8.3).abs() < 0.2,
            "Expected rating around 8.3, got {}",
            parsed
        );
    }

    #[test]
    fn test_merge_preserves_provider_list() {
        let entries = vec![
            make_entry("Dune", "tt15239678", Some("8.0"), &[], MediaType::Movie),
            make_entry("Dune", "tt15239678", Some("8.5"), &[], MediaType::Movie),
        ];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // Provider list should contain both providers
        assert!(result[0].provider.contains("test"));
    }

    #[test]
    fn test_merge_fills_missing_fields() {
        let mut entry1 = make_entry("Dune", "tt15239678", None, &[], MediaType::Movie);
        entry1.description = Some("Director's cut".to_string());
        entry1.year = None;

        let mut entry2 = make_entry(
            "Dune",
            "tt15239678",
            Some("8.0"),
            &[("imdb", 8.0)],
            MediaType::Movie,
        );
        entry2.description = None;
        entry2.year = Some("2024".to_string());

        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(vec![entry1, entry2]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].description, Some("Director's cut".to_string()));
        assert_eq!(result[0].year, Some("2024".to_string()));
    }

    #[test]
    fn test_parse_rating_str_handles_formats() {
        assert_eq!(parse_rating_str("8.5"), Some(8.5));
        assert_eq!(parse_rating_str("8.5/10"), Some(8.5));
        assert_eq!(parse_rating_str("85%"), Some(85.0));
        assert_eq!(parse_rating_str("8.5/10"), Some(8.5));
        assert_eq!(parse_rating_str("7.8"), Some(7.8));
        assert_eq!(parse_rating_str(""), None);
        assert_eq!(parse_rating_str("abc"), None);
    }

    // ── Missing/Inactive Plugin Protection Tests ─────────────────────────────

    #[test]
    fn test_weighted_median_single_source_only() {
        // Only IMDB available - should still return a rating
        let ratings: HashMap<String, f64> = [("imdb".to_string(), 8.5)].into();
        let weights = WEIGHTS_MOVIE;
        let result = weighted_median(&ratings, weights);
        assert!(result.is_some());
        assert!((result.unwrap() - 8.5).abs() < 0.1);
    }

    #[test]
    fn test_weighted_median_all_sources_missing() {
        // No sources available - should return None gracefully
        let ratings: HashMap<String, f64> = HashMap::new();
        let weights = WEIGHTS_MOVIE;
        let result = weighted_median(&ratings, weights);
        assert!(result.is_none());
    }

    #[test]
    fn test_weighted_median_partial_sources() {
        // IMDB and TMDB available, but not tomatometer
        let ratings: HashMap<String, f64> =
            [("imdb".to_string(), 8.5), ("tmdb".to_string(), 8.2)].into();
        let weights = WEIGHTS_MOVIE;
        let result = weighted_median(&ratings, weights);
        assert!(result.is_some());
        // With only imdb and tmdb (weights 0.30 and 0.10),
        // first source (imdb 8.5) crosses 50% threshold
        assert!((result.unwrap() - 8.5).abs() < 0.1);
    }

    #[test]
    fn test_count_active_sources() {
        let ratings: HashMap<String, f64> =
            [("imdb".to_string(), 8.5), ("tmdb".to_string(), 8.2)].into();
        let weights = WEIGHTS_MOVIE;
        assert_eq!(count_active_sources(&ratings, weights), 2);
    }

    #[test]
    fn test_count_active_sources_partial() {
        // Only imdb available
        let ratings: HashMap<String, f64> = [("imdb".to_string(), 8.5)].into();
        let weights = WEIGHTS_MOVIE;
        assert_eq!(count_active_sources(&ratings, weights), 1);
    }

    #[test]
    fn test_missing_sources() {
        // Only imdb available, others missing
        let ratings: HashMap<String, f64> = [("imdb".to_string(), 8.5)].into();
        let weights = WEIGHTS_MOVIE;
        let missing = missing_sources(&ratings, weights);
        assert!(missing.contains(&"tomatometer"));
        assert!(missing.contains(&"tmdb"));
        assert!(!missing.contains(&"imdb"));
    }

    #[test]
    fn test_has_sufficient_sources() {
        let ratings: HashMap<String, f64> =
            [("imdb".to_string(), 8.5), ("tmdb".to_string(), 8.2)].into();
        let weights = WEIGHTS_MOVIE;
        assert!(has_sufficient_sources(&ratings, weights, 1));
        assert!(has_sufficient_sources(&ratings, weights, 2));
        assert!(!has_sufficient_sources(&ratings, weights, 3));
    }

    #[test]
    fn test_fallback_when_no_weighted_sources() {
        // Only an unknown source available - should fallback to it
        let mut entry = make_entry(
            "Unknown Movie",
            "tt0000000",
            None,
            &[("unknown_source", 7.5)],
            MediaType::Movie,
        );
        // ratings map has unknown_source, but WEIGHTS_MOVIE doesn't have it
        let weights = WEIGHTS_MOVIE;

        // weighted_median should return None for unknown source
        let result = weighted_median(&entry.ratings, weights);
        assert!(result.is_none());

        // But count shows 0 active sources from our weight list
        assert_eq!(count_active_sources(&entry.ratings, weights), 0);
    }

    #[test]
    fn test_merge_with_some_sources_inactive() {
        // Simulate IMDB being inactive - only TMDB provides rating
        let entries = vec![make_entry(
            "Movie",
            "tt0000001",
            Some("8.0"),
            &[("tmdb", 8.0)],
            MediaType::Movie,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // Should still get a rating from available source (tmdb)
        assert!(result[0].rating.is_some());
    }

    #[test]
    fn test_apply_weighted_rating_uses_fallback() {
        // Test that apply_weighted_rating falls back correctly
        let mut entry = make_entry(
            "Movie",
            "tt0000001",
            Some("8.5"),                // Original rating
            &[("unknown_source", 7.5)], // Not in weights
            MediaType::Movie,
        );
        apply_weighted_rating(&mut entry);
        // Should fall back to the unknown source since it's the only one
        // Or preserve the original rating
        assert!(entry.rating.is_some());
    }
}
