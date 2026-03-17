//! Integration tests for providers, catalog filters, and aggregation.

use stui_runtime::catalog::CatalogEntry;
use stui_runtime::catalog_engine::{
    aggregator::CatalogAggregator,
    filters::{Filter, FilterSet},
    ranking::SortOrder,
};
use stui_runtime::ipc::MediaType;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn entry(title: &str, year: &str, genre: &str, rating: &str, mt: MediaType) -> CatalogEntry {
    CatalogEntry {
        id:          title.to_lowercase().replace(' ', "-"),
        title:       title.to_string(),
        year:        Some(year.to_string()),
        genre:       Some(genre.to_string()),
        rating:      Some(rating.to_string()),
        description: None,
        poster_url:  None,
        poster_art:  None,
        provider:    "test".to_string(),
        tab:         "movies".to_string(),
        imdb_id:     None,
        tmdb_id:     None,
        media_type:  mt,
    }
}

fn movie(title: &str, year: &str, genre: &str, rating: &str) -> CatalogEntry {
    entry(title, year, genre, rating, MediaType::Movie)
}

fn series(title: &str, year: &str, genre: &str, rating: &str) -> CatalogEntry {
    entry(title, year, genre, rating, MediaType::Series)
}

// ── Filter tests ──────────────────────────────────────────────────────────────

#[test]
fn test_filter_genre() {
    let entries = vec![
        movie("Dune", "2021", "Sci-Fi", "8.0"),
        movie("The Godfather", "1972", "Crime", "9.2"),
    ];
    let mut fs = FilterSet::new();
    fs.add(Filter::genre("Sci-Fi"));
    let result = fs.apply(entries);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "Dune");
}

#[test]
fn test_filter_year_range() {
    let entries = vec![
        movie("Old Film", "1965", "Drama", "7.0"),
        movie("New Film", "2022", "Action", "7.5"),
        movie("Mid Film", "1990", "Comedy", "6.8"),
    ];
    let mut fs = FilterSet::new();
    fs.add(Filter::year_range(1980, 2000));
    let result = fs.apply(entries);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "Mid Film");
}

#[test]
fn test_filter_media_type() {
    let entries = vec![
        movie("Inception", "2010", "Sci-Fi", "8.8"),
        series("Breaking Bad", "2008", "Drama", "9.5"),
    ];
    let mut fs = FilterSet::new();
    fs.add(Filter::media_type(MediaType::Series));
    let result = fs.apply(entries);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "Breaking Bad");
}

#[test]
fn test_filter_min_rating() {
    let entries = vec![
        movie("Great Film", "2020", "Drama", "9.0"),
        movie("Okay Film", "2020", "Drama", "6.5"),
        movie("Bad Film", "2020", "Drama", "4.0"),
    ];
    let mut fs = FilterSet::new();
    fs.add(Filter::min_rating(7.0));
    let result = fs.apply(entries);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "Great Film");
}

#[test]
fn test_filter_title_contains() {
    let entries = vec![
        movie("The Dark Knight", "2008", "Action", "9.0"),
        movie("Batman Begins", "2005", "Action", "8.2"),
        movie("Oppenheimer", "2023", "Drama", "8.5"),
    ];
    let mut fs = FilterSet::new();
    fs.add(Filter::title_contains("bat"));
    let result = fs.apply(entries);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_multiple_filters_are_anded() {
    let entries = vec![
        movie("Sci-Fi Classic", "1968", "Sci-Fi", "8.5"),
        movie("Sci-Fi New", "2023", "Sci-Fi", "7.0"),
        movie("Drama New", "2023", "Drama", "9.0"),
    ];
    let mut fs = FilterSet::new();
    fs.add(Filter::genre("Sci-Fi"));
    fs.add(Filter::year_range(2000, 2030));
    let result = fs.apply(entries);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "Sci-Fi New");
}

#[test]
fn test_empty_filterset_passes_all() {
    let entries = vec![
        movie("A", "2020", "Action", "8.0"),
        movie("B", "2021", "Drama", "7.0"),
    ];
    let result = FilterSet::new().apply(entries);
    assert_eq!(result.len(), 2);
}

// ── Ranking / sort tests ──────────────────────────────────────────────────────

#[test]
fn test_sort_by_rating() {
    let entries = vec![
        movie("Medium", "2020", "Drama", "7.0"),
        movie("Best", "2020", "Drama", "9.5"),
        movie("Worst", "2020", "Drama", "4.0"),
    ];
    let sorted = SortOrder::Rating.apply(entries);
    assert_eq!(sorted[0].title, "Best");
    assert_eq!(sorted[2].title, "Worst");
}

#[test]
fn test_sort_newest_first() {
    let entries = vec![
        movie("Old", "1990", "Drama", "8.0"),
        movie("New", "2023", "Drama", "7.0"),
        movie("Mid", "2005", "Drama", "8.5"),
    ];
    let sorted = SortOrder::Newest.apply(entries);
    assert_eq!(sorted[0].title, "New");
    assert_eq!(sorted[2].title, "Old");
}

#[test]
fn test_sort_alphabetical() {
    let entries = vec![
        movie("Zodiac", "2007", "Crime", "7.7"),
        movie("Alien", "1979", "Horror", "8.4"),
        movie("Memento", "2000", "Thriller", "8.4"),
    ];
    let sorted = SortOrder::Alphabetical.apply(entries);
    assert_eq!(sorted[0].title, "Alien");
    assert_eq!(sorted[2].title, "Zodiac");
}

// ── Aggregator / dedup tests ──────────────────────────────────────────────────

#[test]
fn test_aggregator_deduplicates_by_title_year() {
    let mut e1 = movie("Dune", "2021", "Sci-Fi", "8.0");
    e1.provider = "tmdb".to_string();

    let mut e2 = movie("Dune", "2021", "Sci-Fi", "");
    e2.provider = "imdb".to_string();
    e2.rating   = None;

    let result = CatalogAggregator::new().apply(vec![e1, e2]);
    assert_eq!(result.len(), 1, "duplicate entries should be merged");
    assert!(result[0].provider.contains("tmdb"), "provider list should include both");
}

#[test]
fn test_aggregator_fills_missing_fields() {
    let mut base = movie("Interstellar", "2014", "Sci-Fi", "8.6");
    base.description = None;
    base.provider = "imdb".to_string();

    let mut enriched = movie("Interstellar", "2014", "Sci-Fi", "8.6");
    enriched.description = Some("A journey beyond the stars".to_string());
    enriched.provider = "tmdb".to_string();

    let result = CatalogAggregator::new().apply(vec![base, enriched]);
    assert_eq!(result.len(), 1);
    assert!(result[0].description.is_some(), "description should be filled from secondary");
}

#[test]
fn test_aggregator_preserves_unique_entries() {
    let entries = vec![
        movie("Film A", "2020", "Action", "8.0"),
        movie("Film B", "2021", "Drama", "7.5"),
        movie("Film C", "2022", "Horror", "6.8"),
    ];
    let result = CatalogAggregator::new().apply(entries);
    assert_eq!(result.len(), 3, "unique entries should all be kept");
}
