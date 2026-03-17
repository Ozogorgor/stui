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

use crate::catalog::CatalogEntry;
use crate::ipc::MediaType;
use super::filters::FilterSet;
use super::ranking::SortOrder;

// ── Weight table ─────────────────────────────────────────────────────────────

struct RatingWeight {
    key:       &'static str,
    weight:    f64,
    /// Divisor to normalise the raw score to 0–10.
    /// IMDB/TMDB are already 0–10 (divisor 1.0).
    /// RT percentages and AniList 0–100 need divisor 10.0.
    normalize: f64,
}

// ── Per-profile weight tables ─────────────────────────────────────────────────

const WEIGHTS_MOVIE: &[RatingWeight] = &[
    RatingWeight { key: "tomatometer",    weight: 0.35, normalize: 10.0 },
    RatingWeight { key: "imdb",           weight: 0.30, normalize:  1.0 },
    RatingWeight { key: "audience_score", weight: 0.15, normalize: 10.0 },
    RatingWeight { key: "tmdb",           weight: 0.10, normalize:  1.0 },
    RatingWeight { key: "anilist",        weight: 0.00, normalize: 10.0 },
];

const WEIGHTS_SERIES: &[RatingWeight] = &[
    RatingWeight { key: "tomatometer",    weight: 0.25, normalize: 10.0 },
    RatingWeight { key: "imdb",           weight: 0.35, normalize:  1.0 },
    RatingWeight { key: "audience_score", weight: 0.25, normalize: 10.0 },
    RatingWeight { key: "tmdb",           weight: 0.15, normalize:  1.0 },
    RatingWeight { key: "anilist",        weight: 0.00, normalize: 10.0 },
];

const WEIGHTS_ANIME: &[RatingWeight] = &[
    RatingWeight { key: "tomatometer",    weight: 0.15, normalize: 10.0 },
    RatingWeight { key: "imdb",           weight: 0.20, normalize:  1.0 },
    RatingWeight { key: "audience_score", weight: 0.15, normalize: 10.0 },
    RatingWeight { key: "tmdb",           weight: 0.15, normalize:  1.0 },
    RatingWeight { key: "anilist",        weight: 0.35, normalize: 10.0 },
];

const WEIGHTS_DOCUMENTARY: &[RatingWeight] = &[
    RatingWeight { key: "tomatometer",    weight: 0.45, normalize: 10.0 },
    RatingWeight { key: "imdb",           weight: 0.25, normalize:  1.0 },
    RatingWeight { key: "audience_score", weight: 0.10, normalize: 10.0 },
    RatingWeight { key: "tmdb",           weight: 0.10, normalize:  1.0 },
    RatingWeight { key: "anilist",        weight: 0.00, normalize: 10.0 },
];

const WEIGHTS_HORROR: &[RatingWeight] = &[
    RatingWeight { key: "tomatometer",    weight: 0.25, normalize: 10.0 },
    RatingWeight { key: "imdb",           weight: 0.30, normalize:  1.0 },
    RatingWeight { key: "audience_score", weight: 0.30, normalize: 10.0 },
    RatingWeight { key: "tmdb",           weight: 0.10, normalize:  1.0 },
    RatingWeight { key: "anilist",        weight: 0.00, normalize: 10.0 },
];

const WEIGHTS_MUSIC: &[RatingWeight] = &[
    RatingWeight { key: "tomatometer",    weight: 0.20, normalize: 10.0 },
    RatingWeight { key: "imdb",           weight: 0.20, normalize:  1.0 },
    RatingWeight { key: "audience_score", weight: 0.30, normalize: 10.0 },
    RatingWeight { key: "tmdb",           weight: 0.20, normalize:  1.0 },
    RatingWeight { key: "anilist",        weight: 0.00, normalize: 10.0 },
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
fn weights_for(media_type: &MediaType, genre: Option<&str>, ratings: &HashMap<String, f64>) -> &'static [RatingWeight] {
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
        MediaType::Series | MediaType::Episode                 => WEIGHTS_SERIES,
        _                                                      => WEIGHTS_MOVIE,
    }
}

// ── Core rating functions ─────────────────────────────────────────────────────

/// Compute a weighted composite rating on a 0–10 scale (weighted mean).
///
/// Only sources with weight > 0 that are present in `ratings` contribute;
/// their weights are re-normalised to sum to 1.0.
///
/// Returns `None` if `ratings` is empty or no weighted sources are present.
pub fn weighted_rating(ratings: &HashMap<String, f64>, weights: &[RatingWeight]) -> Option<f64> {
    if ratings.is_empty() {
        return None;
    }

    let mut weighted_sum = 0.0_f64;
    let mut weight_total = 0.0_f64;

    for w in weights {
        if w.weight == 0.0 { continue; }
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
pub fn weighted_median(ratings: &HashMap<String, f64>, weights: &[RatingWeight]) -> Option<f64> {
    if ratings.is_empty() {
        return None;
    }

    // Collect (normalised_score, weight) for present sources with weight > 0.
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    let mut weight_total = 0.0_f64;

    for w in weights {
        if w.weight == 0.0 { continue; }
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

    // Unreachable: cumulative always reaches weight_total.
    Some(pairs.last().unwrap().0)
}

// ── Aggregator ────────────────────────────────────────────────────────────────

pub struct CatalogAggregator {
    filters:    FilterSet,
    sort_order: SortOrder,
}

impl CatalogAggregator {
    pub fn new() -> Self {
        CatalogAggregator {
            filters:    FilterSet::default(),
            sort_order: SortOrder::default(),
        }
    }

    pub fn with_filter(mut self, filter: super::filters::Filter) -> Self {
        self.filters.add(filter);
        self
    }

    pub fn with_sort(mut self, order: SortOrder) -> Self {
        self.sort_order = order;
        self
    }

    /// Merge, dedup, filter, and sort a raw list of entries.
    pub fn apply(&self, entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        let merged   = self.merge(entries);
        let filtered = self.filters.apply(merged);
        self.sort_order.apply(filtered)
    }

    /// Merge duplicates from multiple providers into enriched single entries.
    fn merge(&self, entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        let mut groups: HashMap<String, Vec<CatalogEntry>> = HashMap::new();

        for entry in entries {
            let key = entry.dedup_key();
            groups.entry(key).or_default().push(entry);
        }

        groups
            .into_values()
            .map(merge_group)
            .collect()
    }
}

impl Default for CatalogAggregator {
    fn default() -> Self { Self::new() }
}

/// Merge a group of entries for the same title into one enriched entry.
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
        if e.year.is_some()        { score += 1; }
        if e.genre.is_some()       { score += 1; }
        if e.rating.is_some()      { score += 1; }
        if e.description.is_some() { score += 1; }
        if e.poster_url.is_some()  { score += 1; }
        if e.imdb_id.is_some()     { score += 2; } // especially valuable
        if e.tmdb_id.is_some()     { score += 1; }
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
        if base.year.is_none()        { base.year        = secondary.year.clone(); }
        if base.genre.is_none()       { base.genre       = secondary.genre.clone(); }
        if base.description.is_none() { base.description = secondary.description.clone(); }
        if base.poster_url.is_none()  { base.poster_url  = secondary.poster_url.clone(); }
        if base.imdb_id.is_none()     { base.imdb_id     = secondary.imdb_id.clone(); }
        if base.tmdb_id.is_none()     { base.tmdb_id     = secondary.tmdb_id; }

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
fn apply_weighted_rating(entry: &mut CatalogEntry) {
    let weights = weights_for(&entry.media_type, entry.genre.as_deref(), &entry.ratings);
    if let Some(composite) = weighted_median(&entry.ratings, weights) {
        entry.rating = Some(format!("{:.1}", composite));
    }
}

/// Map a provider name to the canonical ratings key used in the weight tables.
fn rating_key_for_provider(provider: &str) -> &'static str {
    // Provider names can be comma-joined (e.g. "tmdb,imdb") — take first.
    let first = provider.split(',').next().unwrap_or(provider).trim();
    match first {
        "imdb"            => "imdb",
        "tmdb"            => "tmdb",
        "omdb"            => "imdb",  // OMDB reflects IMDB score
        "anilist"         => "anilist",
        "rottentomatoes"  => "tomatometer",
        _                 => "imdb",  // safe fallback
    }
}

/// Parse a rating string to f64.
/// Handles "8.4", "8.4/10", "84%", "84".
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
