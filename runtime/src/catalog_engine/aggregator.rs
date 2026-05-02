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
//! | imdb            | 0.35   | Bayesian, large, hard to game          |
//! | audience_score  | 0.15   | Popular appeal, gameable               |
//! | tmdb            | 0.15   | Decent, smaller sample                 |
//! | anilist         | —      | N/A                                    |
//! | kitsu           | —      | N/A                                    |
//!
//! ## Series / Episode
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.25   | Critics worse at serialised TV         |
//! | imdb            | 0.35   | Go-to source for TV                    |
//! | audience_score  | 0.25   | Audience sustains long-running shows   |
//! | tmdb            | 0.15   | Better TV coverage than for film       |
//! | anilist         | —      | N/A                                    |
//! | kitsu           | —      | N/A                                    |
//!
//! ## Anime  (genre contains "anime", or anilist/kitsu score is present)
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.10   | RT has poor anime coverage             |
//! | imdb            | 0.20   | Useful but not anime-native            |
//! | audience_score  | 0.10   | Less meaningful for anime              |
//! | tmdb            | 0.10   | Reasonable secondary signal            |
//! | anilist         | 0.30   | Community authority for anime          |
//! | kitsu           | 0.20   | Independent anime community; 0–100     |
//!
//! ## Documentary
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.50   | Critics define quality for docs        |
//! | imdb            | 0.25   | Solid secondary signal                 |
//! | audience_score  | 0.10   | Less signal for non-entertainment docs |
//! | tmdb            | 0.15   | Smaller pool                           |
//! | anilist         | —      | N/A                                    |
//! | kitsu           | —      | N/A                                    |
//!
//! ## Horror
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.25   | Critics and audiences routinely diverge|
//! | imdb            | 0.30   | Balanced middle ground                 |
//! | audience_score  | 0.30   | Audience enjoyment central to horror   |
//! | tmdb            | 0.15   | Supplementary                          |
//! | anilist         | —      | N/A                                    |
//! | kitsu           | —      | N/A                                    |
//!
//! ## Music / Album / Track
//! | Source          | Weight | Rationale                              |
//! |-----------------|--------|----------------------------------------|
//! | tomatometer     | 0.20   | Critics less dominant in music         |
//! | imdb            | 0.20   | Limited music coverage                 |
//! | audience_score  | 0.35   | Engagement signal strongest            |
//! | tmdb            | 0.25   | Music data increasingly useful         |
//! | anilist         | —      | N/A                                    |
//! | kitsu           | —      | N/A                                    |
//!
//! All scores are normalised to 0–10 before weighting.
//! OMDB is excluded (it mirrors IMDB — would double-count).
//! Kitsu `averageRating` is 0–100 scale (normalize = 10.0), same as AniList.

use std::collections::HashMap;
use tracing;

use super::filters::FilterSet;
use super::ranking::SortOrder;
use crate::catalog::CatalogEntry;
use crate::ipc::MediaType;

// ── Weight table ─────────────────────────────────────────────────────────────

#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
pub struct RatingWeight {
    pub key: &'static str,
    pub weight: f64,
    /// Divisor to normalise the raw score to 0–10.
    /// IMDB/TMDB are already 0–10 (divisor 1.0).
    /// RT percentages and AniList 0–100 need divisor 10.0.
    pub normalize: f64,
}

/// Default rating weights for movies.
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
pub const WEIGHTS_MOVIE: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.35,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.35,
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
        weight: 0.00,
        normalize: 1.0,
    },
    RatingWeight {
        key: "kitsu",
        weight: 0.00,
        normalize: 1.0,
    },
];

#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
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
        normalize: 1.0,
    },
    RatingWeight {
        key: "kitsu",
        weight: 0.00,
        normalize: 1.0,
    },
];

#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
const WEIGHTS_ANIME: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.10,
        normalize: 10.0,
    },
    RatingWeight {
        key: "imdb",
        weight: 0.20,
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
        weight: 0.30,
        normalize: 1.0,
    },
    RatingWeight {
        key: "kitsu",
        weight: 0.20,
        normalize: 1.0,
    },
];

#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
const WEIGHTS_DOCUMENTARY: &[RatingWeight] = &[
    RatingWeight {
        key: "tomatometer",
        weight: 0.50,
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
        weight: 0.15,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 1.0,
    },
    RatingWeight {
        key: "kitsu",
        weight: 0.00,
        normalize: 1.0,
    },
];

#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
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
        weight: 0.15,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 1.0,
    },
    RatingWeight {
        key: "kitsu",
        weight: 0.00,
        normalize: 1.0,
    },
];

#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
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
        weight: 0.35,
        normalize: 10.0,
    },
    RatingWeight {
        key: "tmdb",
        weight: 0.25,
        normalize: 1.0,
    },
    RatingWeight {
        key: "anilist",
        weight: 0.00,
        normalize: 1.0,
    },
    RatingWeight {
        key: "kitsu",
        weight: 0.00,
        normalize: 1.0,
    },
];

// ── Profile selection ─────────────────────────────────────────────────────────

/// Select the appropriate weight profile for an entry.
///
/// Priority order:
/// 1. Anime — genre contains "anime", OR anilist/kitsu score is present (provider signal).
/// 2. Documentary — genre contains "documentary".
/// 3. Horror — genre contains "horror".
/// 4. Music — MediaType is Music, Album, or Track.
/// 5. Series — MediaType is Series or Episode.
/// 6. Movie — default.
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
fn weights_for(
    media_type: &MediaType,
    genre: Option<&str>,
    ratings: &HashMap<String, f64>,
) -> &'static [RatingWeight] {
    let genre_lc = genre.unwrap_or("").to_ascii_lowercase();

    // Anime: genre hint OR anilist/kitsu data present from provider.
    if genre_lc.contains("anime")
        || ratings.contains_key("anilist")
        || ratings.contains_key("kitsu")
    {
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

/// Bayesian global prior on the 0–10 scale. Single-vote outliers (e.g.
/// "A Poet" with one TMDB user rating it 10/10) shrink toward this when
/// the vote count is far below the per-source cap from `bayesian_cap_for`.
const BAYES_PRIOR: f64 = 6.5;

/// Per-source vote-count thresholds past which raw ratings dominate.
/// Calibrated to each source's vote distribution:
///
/// - **imdb** = 10_000 — popular IMDb titles have 100k–1M+ votes;
///   anything <10k votes still pulls noticeably toward prior;
///   anything >100k is essentially unmoved.
/// - **tmdb** = 1_000 — TMDB's vote distribution is 1–2 orders of
///   magnitude smaller than IMDb's; 1k cap matches its
///   niche-vs-popular split.
/// - **default** = 1_000 — sane fallback for future sources that
///   expose vote counts (Letterboxd, MAL, etc.).
fn bayesian_cap_for(source: &str) -> f64 {
    match source {
        "imdb" => 10_000.0,
        "tmdb" => 1_000.0,
        _      => 1_000.0,
    }
}

/// Bayesian shrinkage: pull a raw rating toward the global prior in
/// proportion to how few votes underpin it.
///
///     shrunk = (v / (v + m)) * raw + (m / (v + m)) * prior
///
/// Used to defang single-vote 10.0s without penalising well-supported
/// scores (10k+ votes are essentially unchanged).
fn bayesian_shrink(raw: f64, votes: u32, prior: f64, cap: f64) -> f64 {
    let v = votes as f64;
    if v <= 0.0 {
        return raw;
    }
    let denom = v + cap;
    (v / denom) * raw + (cap / denom) * prior
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
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
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
pub fn count_active_sources(ratings: &HashMap<String, f64>, weights: &[RatingWeight]) -> usize {
    weights
        .iter()
        .filter(|w| w.weight > 0.0 && ratings.contains_key(w.key))
        .count()
}

/// Returns a list of rating source names that are missing (configured but not present).
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
pub fn has_sufficient_sources(
    ratings: &HashMap<String, f64>,
    weights: &[RatingWeight],
    min_sources: usize,
) -> bool {
    count_active_sources(ratings, weights) >= min_sources
}

// ── Aggregator ────────────────────────────────────────────────────────────────

pub struct CatalogAggregator {
    filters: FilterSet,
    sort_order: SortOrder,
}

impl CatalogAggregator {
    pub fn new() -> Self {
        CatalogAggregator {
            filters: FilterSet::default(),
            sort_order: SortOrder::default(),
        }
    }

    #[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
    pub fn with_filter(mut self, filter: super::filters::Filter) -> Self {
        self.filters.add(filter);
        self
    }

    #[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
    pub fn with_sort(mut self, order: SortOrder) -> Self {
        self.sort_order = order;
        self
    }

    /// Merge, dedup, filter, and sort a raw list of entries in one call.
    ///
    /// The engine uses `merge()` + `apply_search_options()` directly so it can
    /// cache the merged (unfiltered) entries and apply different filter/sort
    /// combinations without re-merging.  This method remains available for
    /// callers that want the full pipeline in a single call.
    #[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
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
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
fn merge_group(mut group: Vec<CatalogEntry>) -> CatalogEntry {
    if group.len() == 1 {
        let mut entry = group.remove(0);
        // Still compute composite even for single-source entries.
        promote_rating_to_map(&mut entry);
        apply_weighted_rating(&mut entry);
        return entry;
    }

    // The bucket key is the same for every entry (they collapsed here).
    let key = group[0].dedup_key();

    // Three-key sort:
    //   1. provider_priority_for_key — lower wins; routes Western
    //      anchors (tmdb/tvdb/omdb) ahead of anime providers for
    //      tmdb:/imdb:/title: keys, and the reverse for mal: keys.
    //   2. mal_id ASC when present — within an anilist-only collapse
    //      (multiple cours of the same show, no Western sibling),
    //      this tie-breaks to the LOWEST mal_id, which is the
    //      original / earliest cour. AniList ships per-cour titles
    //      ("Show", "Show: Season 2", "Show: Final Season Part 1");
    //      the lowest mal_id is usually the parent / canonical title
    //      and matches what users expect to see on the card. Without
    //      this rule, completeness alone picked the latest cour
    //      (more populated fields → "Show: Season 2" as spine title).
    //   3. field_completeness_score DESC — fallback when mal_id is
    //      absent or equal. Reverse so higher completeness wins.
    group.sort_by_key(|e| (
        crate::anime_bridge::enrich::provider_priority_for_key(&e.provider, &key),
        mal_id_sort_key(e),
        std::cmp::Reverse(field_completeness_score(e)),
    ));
    // group[0] is now the spine.

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
            base.tmdb_id = secondary.tmdb_id.clone();
        }
        if base.mal_id.is_none() {
            base.mal_id = secondary.mal_id.clone();
        }
        if base.original_language.is_none() {
            base.original_language = secondary.original_language.clone();
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

/// Field-completeness score, extracted from the prior inline sort.
/// Higher score = more fields populated. Used as a tiebreaker when
/// `provider_priority_for_key` returns equal values (e.g., multiple
/// entries from the same provider — extremely rare).
fn field_completeness_score(e: &CatalogEntry) -> usize {
    let mut score = 0;
    if e.year.is_some()        { score += 1; }
    if e.genre.is_some()       { score += 1; }
    if e.rating.is_some()      { score += 1; }
    if e.description.is_some() { score += 1; }
    if e.poster_url.is_some()  { score += 1; }
    if e.imdb_id.is_some()     { score += 2; }
    if e.mal_id.is_some()      { score += 2; }
    if e.tmdb_id.is_some()     { score += 1; }
    score
}

/// Sort key used by `merge_group` to break ties within a collapse
/// bucket: parses `mal_id` as a u64 so the LOWEST mal id wins the
/// spine slot. Entries without a mal id sort last (`u64::MAX`) so they
/// never win the spine over a mal-tagged sibling. Parsing failures
/// also fall back to `u64::MAX` — defensive against non-numeric ids
/// that shouldn't exist for MAL but might leak in from misconfigured
/// plugins.
fn mal_id_sort_key(e: &CatalogEntry) -> u64 {
    e.mal_id
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

/// If an entry has a plain `rating` string but an empty `ratings` map,
/// try to parse the string and insert it under the provider's canonical key.
/// Skipped for unrecognised providers — storing their values under a wrong key
/// with an incorrect normalization divisor would corrupt the composite score.
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
fn promote_rating_to_map(entry: &mut CatalogEntry) {
    if entry.ratings.is_empty() {
        if let Some(ref r) = entry.rating.clone() {
            if let Some(key) = rating_key_for_provider(&entry.provider) {
                if let Some(val) = parse_rating_str(r) {
                    entry.ratings.insert(key.to_string(), val);
                }
            }
        }
    }
}

/// Select the weight profile and compute the weighted median into `entry.rating`.
///
/// If no recognised sources are present, the original `entry.rating` string
/// (set by the provider) is preserved unchanged. The raw ratings map may contain
/// values on unknown scales so they are never used as a direct fallback.
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
/// Recompute `entry.rating` from `entry.ratings` using the weight
/// profile selected by media_type + genre, with the user's
/// `rating_weights` override applied on top of the static profile.
///
/// Public so post-search enrichment passes (music_enrich, video_enrich)
/// can refresh the composite headline score after injecting
/// per-source values.
pub fn apply_weighted_rating(entry: &mut CatalogEntry) {
    let static_profile = weights_for(&entry.media_type, entry.genre.as_deref(), &entry.ratings);
    let overrides = USER_RATING_WEIGHTS.read().unwrap_or_else(|e| e.into_inner());
    let merged = merge_weights(static_profile, &overrides);

    if !has_sufficient_sources(&entry.ratings, &merged, 1) {
        tracing::debug!(
            title = %entry.title,
            "no recognised rating sources; preserving provider rating"
        );
        return;
    }

    // Apply per-source Bayesian shrinkage where vote counts are
    // available so a single-vote 10.0 can't dominate the composite.
    // Sources without vote data (RT/Metacritic critic scores) pass
    // through unchanged — those represent expert reviews, not
    // user-poll samples.
    let shrunk = shrink_ratings(&entry.ratings, &entry.rating_votes, &merged);

    if let Some(composite) = weighted_median(&shrunk, &merged) {
        entry.rating = Some(format!("{:.1}", composite));
    }
}

/// Returns a copy of the ratings map with Bayesian shrinkage applied to
/// every source that has an associated vote count in `votes`. Sources
/// without vote data carry through unchanged.
fn shrink_ratings(
    ratings: &HashMap<String, f64>,
    votes: &HashMap<String, u32>,
    weights: &[RatingWeight],
) -> HashMap<String, f64> {
    let mut out = ratings.clone();
    for w in weights {
        let Some(&raw) = ratings.get(w.key) else { continue };
        let Some(&v) = votes.get(w.key) else { continue };
        if w.normalize <= 0.0 {
            continue;
        }
        let normalised = (raw / w.normalize).clamp(0.0, 10.0);
        let cap = bayesian_cap_for(w.key);
        let shrunk = bayesian_shrink(normalised, v, BAYES_PRIOR, cap);
        // Re-scale back into the source's native range so weighted_median's
        // own normalization step lands on the shrunk value.
        out.insert(w.key.to_string(), shrunk * w.normalize);
    }
    out
}

/// Merge the user's per-source weight overrides onto the static
/// per-tab profile.
///
/// Rules:
/// - If a key is in both, the user weight wins (overrides static).
/// - If a key is only in the user map, it's appended with
///   `normalize: 1.0` (assuming plugins emit 0–10 scale, which is
///   the convention for pre-normalised plugin output).
/// - If a key is only in the static profile, it carries through
///   unchanged.
///
/// This is what enables third-party / user-authored plugins to
/// contribute to the composite without recompiling the runtime —
/// install the plugin, drop a weight in `runtime.toml`, the
/// aggregator picks the source up.
fn merge_weights(
    static_profile: &[RatingWeight],
    overrides: &std::collections::HashMap<String, f64>,
) -> Vec<RatingWeight> {
    use std::collections::HashSet;
    let static_keys: HashSet<&'static str> = static_profile.iter().map(|w| w.key).collect();
    let mut out: Vec<RatingWeight> = static_profile
        .iter()
        .map(|w| {
            let weight = overrides.get(w.key).copied().unwrap_or(w.weight);
            RatingWeight {
                key: w.key,
                weight,
                normalize: w.normalize,
            }
        })
        .collect();
    for (key, weight) in overrides.iter() {
        if static_keys.contains(key.as_str()) {
            continue;
        }
        if *weight == 0.0 {
            continue;
        }
        // Leak the key string to obtain a 'static lifetime — the
        // RatingWeight struct's `key: &'static str` was designed for
        // compile-time constants; user-config keys arrive at runtime
        // so they need to outlive the merged Vec. The leak is
        // bounded (one-time per unique source name, low cardinality)
        // and hot-reloading config simply re-leaks the same set.
        let leaked: &'static str = Box::leak(key.clone().into_boxed_str());
        out.push(RatingWeight {
            key: leaked,
            weight: *weight,
            normalize: 1.0,
        });
    }
    out
}

/// Process-wide overlay of user rating-source weights, sourced from
/// `RuntimeConfig.rating_weights` at startup (and updatable later
/// from the TUI via IPC config_update — see SCAFFOLD_TODOS §26).
/// Empty by default — `apply_weighted_rating` falls back to pure
/// static profile semantics. Wrapped in LazyLock because
/// `HashMap::new()` isn't const-eligible; the lock initializes once
/// on first read/write.
pub static USER_RATING_WEIGHTS: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<String, f64>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));

/// Replace the in-process rating-weights overlay. Called once at
/// runtime startup and again on any future config_update IPC.
pub fn set_user_rating_weights(weights: std::collections::HashMap<String, f64>) {
    if let Ok(mut guard) = USER_RATING_WEIGHTS.write() {
        *guard = weights;
    }
}

/// Map a provider name to the canonical ratings key used in the weight tables.
///
/// Returns `None` for unrecognised providers so callers can skip promotion
/// rather than storing values under the wrong key with the wrong normalization.
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
fn rating_key_for_provider(provider: &str) -> Option<&'static str> {
    // Provider names can be comma-joined (e.g. "tmdb,imdb") — take first.
    let first = provider.split(',').next().unwrap_or(provider).trim();
    match first {
        "imdb" => Some("imdb"),
        "tmdb" => Some("tmdb"),
        "omdb" => Some("imdb"), // OMDB reflects IMDB score
        "anilist" => Some("anilist"),
        "kitsu" => Some("kitsu"),
        "rottentomatoes" => Some("tomatometer"),
        "rottentomatoes_audience" => Some("audience_score"),
        _ => None,
    }
}

/// Parse a rating string to f64.
/// Handles "8.4", "8.4/10", "84%", "84".
///
/// # Scale contract
///
/// The returned value preserves the *raw* numeric value in the string — no
/// scale conversion is performed here.  The caller is responsible for
/// pairing the result with the correct `RatingWeight::normalize` divisor:
///
/// | Provider string example | Raw value | `normalize` | Normalised (0–10) |
/// |-------------------------|-----------|-------------|-------------------|
/// | `"8.4"` (IMDB)          | 8.4       | 1.0         | 8.4               |
/// | `"84%"` (RT)            | 84.0      | 10.0        | 8.4               |
/// | `"84"` (AniList 0–100)  | 84.0      | 10.0        | 8.4               |
///
/// Storing RT/AniList values without dividing by 10 would produce composite
/// scores an order of magnitude too high.
#[allow(dead_code)] // pub API: used by CatalogEngine and engine/mod.rs
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
            artist: None,
            imdb_id: Some(imdb_id.to_string()),
            tmdb_id: None,
            mal_id: None,
            media_type,
            ratings: ratings_map,
            rating_votes: std::collections::HashMap::new(),
            original_language: None,
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
        // anilist plugin pre-normalises its 0-100 averageScore to
        // 0-10 (lib.rs:1127), so 92 → 9.2 reaches the aggregator
        // already scaled and the weight profile uses normalize=1.0.
        let entries = vec![make_entry(
            "Attack on Titan",
            "tt12345678",
            Some("9.0"),
            &[("imdb", 8.5), ("anilist", 9.2)],
            MediaType::Series,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // anilist (9.2, w=0.30) outweighs imdb (8.5, w=0.20); median should be 9.2
        let rating = result[0].rating.as_ref().unwrap();
        let parsed: f64 = rating.parse().unwrap();
        assert!(parsed >= 9.0, "Expected rating >= 9.0, got {}", parsed);
    }

    #[test]
    fn test_kitsu_triggers_anime_profile() {
        // kitsu plugin pre-normalises its 0-100 averageRating to
        // 0-10 (lib.rs:572), so 88 → 8.8 arrives already scaled.
        let entries = vec![make_entry(
            "Fullmetal Alchemist",
            "tt0421955",
            None,
            &[("imdb", 9.1), ("kitsu", 8.8)],
            MediaType::Series,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // kitsu (8.8, w=0.20) and imdb (9.1, w=0.20): sorted [(8.8,0.20),(9.1,0.20)]
        // cumulative at 8.8 = 0.20 = 0.20 (half of 0.40), so median is 8.8
        let rating = result[0].rating.as_ref().unwrap();
        let parsed: f64 = rating.parse().unwrap();
        assert!(
            parsed > 8.5,
            "Expected anime-weighted rating > 8.5, got {}",
            parsed
        );
    }

    #[test]
    fn test_kitsu_anilist_combined() {
        // Both kitsu and anilist present — their combined weight (0.50) dominates imdb (0.20).
        // The kitsu and anilist plugins pre-normalize their 0-100
        // averageScore/averageRating to 0-10 before publishing
        // (kitsu/lib.rs:572, anilist/lib.rs:1127). The aggregator's
        // weight profile uses normalize=1.0 for both keys, so input
        // here is already on the 0-10 scale.
        let entries = vec![make_entry(
            "Spirited Away",
            "tt0245429",
            None,
            &[("imdb", 8.6), ("anilist", 9.0), ("kitsu", 9.2)],
            MediaType::Movie,
        )];
        let aggregator = CatalogAggregator::new();
        let result = aggregator.merge(entries);
        assert_eq!(result.len(), 1);
        // Sorted by score: imdb=8.6(w=0.20), anilist=9.0(w=0.30), kitsu=9.2(w=0.20)
        // cumulative: 8.6→0.20, 9.0→0.50 — reaches half at anilist score
        let rating = result[0].rating.as_ref().unwrap();
        let parsed: f64 = rating.parse().unwrap();
        assert!(
            (parsed - 9.0).abs() < 0.2,
            "Expected rating around 9.0, got {}",
            parsed
        );
    }

    #[test]
    fn test_kitsu_not_used_for_movies_without_anime_signal() {
        // kitsu key absent, no anime genre → movie profile; kitsu weight is 0.00 anyway
        let ratings: HashMap<String, f64> =
            [("imdb".to_string(), 8.0), ("tmdb".to_string(), 7.8)].into();
        let weights = weights_for(&MediaType::Movie, None, &ratings);
        let kitsu_entry = weights.iter().find(|w| w.key == "kitsu").unwrap();
        assert_eq!(
            kitsu_entry.weight, 0.00,
            "kitsu must have 0 weight in movie profile"
        );
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

    /// Regression test for the user-reported "Jujutsu Kaisen Culling Game
    /// Part 1 appears twice (once from AniList, once from Kitsu)" bug. Both
    /// providers expose `mal_id`; the dedup key now collapses them via the
    /// MAL precedence even when titles differ (English vs romaji).
    #[test]
    /// Within an anilist-only collapse (multiple cours of the same
    /// show, no Western sibling), the spine should be the cour with
    /// the LOWEST mal_id — that's the original / earliest cour and
    /// usually carries the canonical parent title. Without the mal_id
    /// tie-break, completeness alone picked the latest cour and the
    /// merged card said "Frieren: Beyond Journey's End Season 2"
    /// instead of "Frieren: Beyond Journey's End".
    #[test]
    fn test_merge_picks_lowest_mal_id_as_spine_for_anilist_only_collapse() {
        let mut s1 = make_entry(
            "Frieren: Beyond Journey's End",
            "",
            None,
            &[],
            MediaType::Series,
        );
        s1.imdb_id = None;
        // Real MAL ids: 52991 (Frieren — parent) vs 59978 (S2 cour).
        // Parent was registered first → lower id → wins the spine
        // slot under the lowest-mal-id tie-break.
        s1.mal_id = Some("52991".to_string());
        s1.tmdb_id = Some("209867".to_string());
        s1.provider = "anilist".to_string();
        // The S2 cour has more populated fields — without the mal_id
        // tie-break, completeness alone would pick S2 as spine.
        let mut s2 = make_entry(
            "Frieren: Beyond Journey's End Season 2",
            "",
            Some("8.7"),
            &[("anilist", 8.7)],
            MediaType::Series,
        );
        s2.imdb_id = None;
        s2.mal_id = Some("59978".to_string());
        s2.tmdb_id = Some("209867".to_string());
        s2.provider = "anilist".to_string();
        s2.description = Some("Long cour 2 synopsis…".to_string());
        s2.poster_url = Some("https://example/s2.jpg".to_string());
        s2.genre = Some("Fantasy".to_string());

        let merged = CatalogAggregator::new().merge(vec![s2, s1]);
        assert_eq!(merged.len(), 1, "two anilist cours with same tmdb_id should collapse");
        assert_eq!(
            merged[0].title,
            "Frieren: Beyond Journey's End",
            "spine title should be the lowest-mal-id cour, not the latest cour with more fields",
        );
        // Lowest-mal-id wins as spine, but the bucket still records both
        // providers in the comma-joined list (with dedup; identical
        // strings collapse).
        assert!(merged[0].provider.contains("anilist"));
    }

    #[test]
    fn test_merge_collapses_anilist_kitsu_via_mal() {
        let mut anilist = make_entry("JJK Culling Game", "", None, &[], MediaType::Series);
        anilist.imdb_id = None;
        anilist.mal_id = Some("57658".to_string());
        anilist.provider = "anilist".to_string();

        let mut kitsu = make_entry(
            "Jujutsu Kaisen: Shimetsu Kaiyū Zenpen",
            "",
            None,
            &[],
            MediaType::Series,
        );
        kitsu.imdb_id = None;
        kitsu.mal_id = Some("57658".to_string());
        kitsu.provider = "kitsu".to_string();

        let merged = CatalogAggregator::new().merge(vec![anilist, kitsu]);
        assert_eq!(merged.len(), 1, "AniList and Kitsu with same MAL should collapse");
    }

    #[test]
    fn test_merge_collapses_anime_and_western_tiers_via_bridge() {
        use crate::anime_bridge::AnimeBridge;
        use crate::anime_bridge::enrich::enrich_entry;
        use crate::ipc::v1::MediaEntry;

        // Build MediaEntry-shaped inputs first so we can run bridge
        // enrichment, then convert to CatalogEntry as the production
        // search_catalog_entries path does.
        let mut anilist_me = MediaEntry {
            id: "anilist-1".into(),
            title: "Cowboy Bebop".into(),
            year: Some("1998".into()),
            provider: "anilist".into(),
            mal_id: Some("1".into()),
            ..Default::default()
        };
        let mut omdb_me = MediaEntry {
            id: "omdb-tt0213338".into(),
            title: "Cowboy Bebop".into(),
            year: Some("1998".into()),
            provider: "omdb".into(),
            imdb_id: Some("tt0213338".into()),
            ..Default::default()
        };

        let bridge = AnimeBridge::new();
        enrich_entry(&mut anilist_me, &bridge);
        enrich_entry(&mut omdb_me, &bridge);

        // Convert to CatalogEntry — mirror the production conversion at
        // engine/mod.rs:1211 (mal_id, imdb_id, tmdb_id all carry over).
        let to_catalog = |e: MediaEntry| CatalogEntry {
            id: e.id,
            title: e.title,
            year: e.year,
            genre: None,
            rating: None,
            description: None,
            poster_url: None,
            poster_art: None,
            provider: e.provider,
            tab: "movies".into(),
            artist: None,
            imdb_id: e.imdb_id,
            tmdb_id: e.tmdb_id,
            mal_id: e.mal_id,
            media_type: MediaType::default(),
            ratings: HashMap::new(),
            rating_votes: HashMap::new(),
            original_language: None,
        };

        let merged = CatalogAggregator::new().merge(vec![
            to_catalog(anilist_me),
            to_catalog(omdb_me),
        ]);
        assert_eq!(merged.len(), 1, "AniList and OMDb should collapse via bridge");
        // `merge_group` joins all contributing providers into a comma list,
        // spine first → assert starts_with rather than exact match.
        assert!(
            merged[0].provider.starts_with("anilist"),
            "AniList must lead the provider list as spine on mal-keyed merge; got {}",
            merged[0].provider,
        );
    }

    #[test]
    fn test_merge_picks_anilist_over_tvdb_on_mal_key() {
        use crate::anime_bridge::AnimeBridge;
        use crate::anime_bridge::enrich::enrich_entry;
        use crate::ipc::v1::MediaEntry;

        // Use Cowboy Bebop — same rationale as the search_scoped test:
        // AOT (mal=16498) shares imdb tt2560140 across 6 season records
        // in the bundled Fribb snapshot, so TVDB→imdb→mal enrichment
        // can resolve to a non-canonical mal_id. Bebop's single-record
        // mapping (mal=1, imdb=tt0213338) is unambiguous.
        let mut anilist_me = MediaEntry {
            id: "anilist-1".into(),
            title: "Cowboy Bebop".into(),
            year: Some("1998".into()),
            provider: "anilist".into(),
            mal_id: Some("1".into()),
            ..Default::default()
        };
        let mut tvdb_me = MediaEntry {
            id: "tvdb-76885".into(),
            title: "Cowboy Bebop".into(),
            year: Some("1998".into()),
            provider: "tvdb".into(),
            imdb_id: Some("tt0213338".into()),
            ..Default::default()
        };

        let bridge = AnimeBridge::new();
        enrich_entry(&mut anilist_me, &bridge);
        enrich_entry(&mut tvdb_me, &bridge);

        let to_catalog = |e: MediaEntry| CatalogEntry {
            id: e.id, title: e.title, year: e.year,
            genre: None, rating: None, description: None,
            poster_url: None, poster_art: None,
            provider: e.provider, tab: "series".into(), artist: None,
            imdb_id: e.imdb_id, tmdb_id: e.tmdb_id, mal_id: e.mal_id,
            media_type: MediaType::default(),
            ratings: HashMap::new(), rating_votes: HashMap::new(), original_language: None,
        };

        let merged = CatalogAggregator::new().merge(vec![
            to_catalog(anilist_me),
            to_catalog(tvdb_me),
        ]);
        assert_eq!(merged.len(), 1);
        // `merge_group` joins all contributing providers into a comma list,
        // spine first → assert starts_with rather than exact match.
        assert!(
            merged[0].provider.starts_with("anilist"),
            "AniList must lead provider list; got {}",
            merged[0].provider,
        );
    }

    #[test]
    fn test_merge_keeps_tvdb_over_anilist_on_imdb_key() {
        // Western series; bridge enrichment no-ops; key is imdb:; existing
        // α priority keeps TVDB as spine.
        let tvdb = CatalogEntry {
            id: "tvdb-81189".into(),
            title: "Breaking Bad".into(),
            year: Some("2008".into()),
            genre: None, rating: None, description: None,
            poster_url: None, poster_art: None,
            provider: "tvdb".into(), tab: "series".into(), artist: None,
            imdb_id: Some("tt0903747".into()), tmdb_id: None, mal_id: None,
            media_type: MediaType::default(),
            ratings: HashMap::new(), rating_votes: HashMap::new(), original_language: None,
        };
        let anilist = CatalogEntry {
            id: "anilist-X".into(),
            title: "Breaking Bad".into(),
            year: Some("2008".into()),
            genre: None, rating: None, description: None,
            poster_url: None, poster_art: None,
            provider: "anilist".into(), tab: "series".into(), artist: None,
            imdb_id: Some("tt0903747".into()), tmdb_id: None, mal_id: None,
            media_type: MediaType::default(),
            ratings: HashMap::new(), rating_votes: HashMap::new(), original_language: None,
        };

        let merged = CatalogAggregator::new().merge(vec![tvdb, anilist]);
        assert_eq!(merged.len(), 1);
        // `merge_group` joins all contributing providers into a comma list,
        // spine first → assert starts_with rather than exact match.
        assert!(
            merged[0].provider.starts_with("tvdb"),
            "TVDB must lead provider list on imdb-keyed merge; got {}",
            merged[0].provider,
        );
    }

    #[test]
    fn test_apply_weighted_rating_preserves_original_for_unknown_source() {
        // When the ratings map contains only unrecognised keys, the provider's
        // original rating string must be preserved unchanged (no fallback to
        // raw unscaled values from the map).
        let mut entry = make_entry(
            "Movie",
            "tt0000001",
            Some("8.5"),                // Provider's original rating
            &[("unknown_source", 7.5)], // Not in any weight table
            MediaType::Movie,
        );
        apply_weighted_rating(&mut entry);
        assert_eq!(entry.rating, Some("8.5".to_string()));
    }

    #[test]
    fn bayesian_cap_for_returns_per_source_values() {
        assert_eq!(bayesian_cap_for("imdb"), 10_000.0);
        assert_eq!(bayesian_cap_for("tmdb"), 1_000.0);
        assert_eq!(bayesian_cap_for("anilist"), 1_000.0);
        assert_eq!(bayesian_cap_for("tomatometer"), 1_000.0);
        assert_eq!(bayesian_cap_for(""), 1_000.0);
    }

    #[test]
    fn shrink_ratings_high_vote_imdb_barely_moves() {
        let mut ratings = std::collections::HashMap::new();
        ratings.insert("imdb".to_string(), 9.0);
        let mut votes = std::collections::HashMap::new();
        votes.insert("imdb".to_string(), 200_000u32);
        let shrunk = shrink_ratings(&ratings, &votes, WEIGHTS_MOVIE);
        let v = *shrunk.get("imdb").unwrap();
        // 200k / (200k + 10k) * 9.0 + 10k / 210k * 6.5 ≈ 8.881
        assert!(v > 8.85 && v < 8.95, "expected ~8.88, got {v}");
    }

    #[test]
    fn shrink_ratings_low_vote_imdb_pulls_toward_prior() {
        let mut ratings = std::collections::HashMap::new();
        ratings.insert("imdb".to_string(), 9.0);
        let mut votes = std::collections::HashMap::new();
        votes.insert("imdb".to_string(), 100u32);
        let shrunk = shrink_ratings(&ratings, &votes, WEIGHTS_MOVIE);
        let v = *shrunk.get("imdb").unwrap();
        // 100 / 10_100 * 9.0 + 10_000 / 10_100 * 6.5 ≈ 6.525
        assert!(v < 6.6, "expected <6.6 (heavy pull), got {v}");
    }

    #[test]
    fn shrink_ratings_tmdb_cap_unchanged() {
        let mut ratings = std::collections::HashMap::new();
        ratings.insert("tmdb".to_string(), 9.0);
        let mut votes = std::collections::HashMap::new();
        votes.insert("tmdb".to_string(), 5_000u32);
        let shrunk = shrink_ratings(&ratings, &votes, WEIGHTS_MOVIE);
        let v = *shrunk.get("tmdb").unwrap();
        // 5000 / 6000 * 9.0 + 1000 / 6000 * 6.5 ≈ 8.583
        assert!(v > 8.55 && v < 8.62, "expected ~8.58, got {v}");
    }

    #[test]
    fn shrink_ratings_imdb_monotonic_in_votes() {
        // Sweep starts at 100 (not 0) — at 0 votes the shrinkage path is
        // skipped entirely and the raw 9.0 passes through, breaking
        // monotonicity against the n=100 result (~6.52). Pass-through at
        // n=0 is correct behavior, just not part of this monotonicity
        // claim. `bayesian_shrink` itself is also tested directly with
        // votes=0 elsewhere.
        let votes_seq = [100u32, 1_000, 10_000, 100_000, 1_000_000];
        let mut prev: f64 = f64::NEG_INFINITY;
        for &n in &votes_seq {
            let mut ratings = std::collections::HashMap::new();
            ratings.insert("imdb".to_string(), 9.0);
            let mut votes = std::collections::HashMap::new();
            votes.insert("imdb".to_string(), n);
            let shrunk = shrink_ratings(&ratings, &votes, WEIGHTS_MOVIE);
            let v = *shrunk.get("imdb").unwrap();
            assert!(v >= prev, "non-monotonic at votes={n}: {v} < {prev}");
            prev = v;
        }
        assert!(prev > 8.95 && prev <= 9.0, "1M votes should ≈ 9.0");
    }
}
