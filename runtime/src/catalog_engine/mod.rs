//! Catalog engine — aggregates, filters, and ranks content from all sources.
//!
//! The raw `catalog.rs` module handles caching and provider fan-out.
//! This module sits on top and adds:
//!
//! - **Aggregator**: merge results from multiple providers, dedup by IMDB id
//! - **Filters**: genre, year range, media type, minimum rating
//! - **Ranking**: sort by rating, recency, alphabetical, or relevance score
//!
//! # Usage
//!
//! ```rust
//! let agg = CatalogAggregator::new();
//! let results = agg
//!     .with_filter(Filter::genre("Sci-Fi"))
//!     .with_filter(Filter::year_range(2010, 2024))
//!     .with_sort(SortOrder::Rating)
//!     .apply(raw_entries);
//! ```

pub mod aggregator;
pub mod filters;
pub mod ranking;

pub use aggregator::CatalogAggregator;
pub use filters::{Filter, FilterSet};
pub use ranking::SortOrder;
