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
//! See [`aggregators`](crate::catalog_engine::aggregator) module for detailed usage examples.

pub mod aggregator;
pub mod filters;
pub mod ranking;

pub use aggregator::CatalogAggregator;
#[allow(unused_imports)]
pub use aggregator::weighted_median;
#[allow(unused_imports)]
pub use filters::{Filter, FilterSet};
#[allow(unused_imports)]
pub use ranking::SortOrder;
