//! Composable filters for catalog results.

#![allow(dead_code)]

use crate::catalog::CatalogEntry;
use crate::ipc::MediaType;

/// A single filter predicate.
#[derive(Debug, Clone)]
pub enum Filter {
    /// Keep only entries whose genre contains this string (case-insensitive).
    Genre(String),
    /// Keep only entries within this year range (inclusive).
    YearRange(u32, u32),
    /// Keep only entries of this media type.
    MediaType(MediaType),
    /// Keep only entries with a rating at or above this threshold.
    /// Rating strings are parsed as f64; unparseable entries are kept.
    MinRating(f64),
    /// Keep only entries from this provider (exact match or comma-list contains).
    Provider(String),
    /// Keep only entries whose title contains this string (case-insensitive).
    TitleContains(String),
}

impl Filter {
    pub fn genre(s: impl Into<String>) -> Self {
        Filter::Genre(s.into())
    }
    pub fn year_range(from: u32, to: u32) -> Self {
        Filter::YearRange(from, to)
    }
    pub fn media_type(t: MediaType) -> Self {
        Filter::MediaType(t)
    }
    pub fn min_rating(r: f64) -> Self {
        Filter::MinRating(r)
    }
    pub fn provider(p: impl Into<String>) -> Self {
        Filter::Provider(p.into())
    }
    pub fn title_contains(s: impl Into<String>) -> Self {
        Filter::TitleContains(s.into())
    }

    pub fn matches(&self, entry: &CatalogEntry) -> bool {
        match self {
            Filter::Genre(g) => entry
                .genre
                .as_deref()
                .map(|genre| genre.to_lowercase().contains(&g.to_lowercase()))
                .unwrap_or(true), // keep if unknown

            Filter::YearRange(from, to) => entry
                .year
                .as_deref()
                .and_then(|y| y.parse::<u32>().ok())
                .map(|year| year >= *from && year <= *to)
                .unwrap_or(true),

            Filter::MediaType(t) => &entry.media_type == t,

            Filter::MinRating(min) => entry
                .rating
                .as_deref()
                .and_then(|r| r.parse::<f64>().ok())
                .map(|r| r >= *min)
                .unwrap_or(false),

            Filter::Provider(p) => entry
                .provider
                .split(',')
                .any(|prov| prov.trim() == p.as_str()),

            Filter::TitleContains(s) => entry.title.to_lowercase().contains(&s.to_lowercase()),
        }
    }
}

/// An ordered set of filters — all must pass (AND logic).
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    filters: Vec<Filter>,
}

impl FilterSet {
    pub fn new() -> Self {
        FilterSet::default()
    }

    pub fn add(&mut self, f: Filter) {
        self.filters.push(f);
    }

    #[allow(dead_code)]
    pub fn apply(&self, entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        if self.filters.is_empty() {
            return entries;
        }
        entries
            .into_iter()
            .filter(|e| self.filters.iter().all(|f| f.matches(e)))
            .collect()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}
