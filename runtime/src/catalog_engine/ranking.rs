//! Sort orders for catalog results.

#![allow(dead_code)]

use crate::catalog::CatalogEntry;

#[derive(Debug, Clone, Default)]
pub enum SortOrder {
    /// Sort by audience rating descending (highest first). Default.
    #[default]
    Rating,
    /// Sort by release year descending (newest first).
    Newest,
    /// Sort by release year ascending (oldest first).
    Oldest,
    /// Sort alphabetically by title ascending.
    Alphabetical,
    /// Sort by provider-native relevance score (preserve original order).
    Relevance,
    /// Sort by popularity proxy: rating × log(vote_count) — requires tmdb data.
    Popularity,
}

impl SortOrder {
    #[allow(dead_code)]
    pub fn apply(&self, mut entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        match self {
            SortOrder::Rating => {
                entries.sort_by(|a, b| {
                    let ra: f64 = a
                        .rating
                        .as_deref()
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0.0);
                    let rb: f64 = b
                        .rating
                        .as_deref()
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0.0);
                    rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            SortOrder::Newest => {
                entries.sort_by(|a, b| {
                    let ya: u32 = a.year.as_deref().and_then(|y| y.parse().ok()).unwrap_or(0);
                    let yb: u32 = b.year.as_deref().and_then(|y| y.parse().ok()).unwrap_or(0);
                    yb.cmp(&ya)
                });
            }
            SortOrder::Oldest => {
                entries.sort_by(|a, b| {
                    let ya: u32 = a
                        .year
                        .as_deref()
                        .and_then(|y| y.parse().ok())
                        .unwrap_or(9999);
                    let yb: u32 = b
                        .year
                        .as_deref()
                        .and_then(|y| y.parse().ok())
                        .unwrap_or(9999);
                    ya.cmp(&yb)
                });
            }
            SortOrder::Alphabetical => {
                entries.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
            }
            SortOrder::Relevance => {
                // Preserve original order — already sorted by relevance from provider
            }
            SortOrder::Popularity => {
                // Without vote_count data we fall back to rating
                entries.sort_by(|a, b| {
                    let ra: f64 = a
                        .rating
                        .as_deref()
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0.0);
                    let rb: f64 = b
                        .rating
                        .as_deref()
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0.0);
                    rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }
        entries
    }
}
