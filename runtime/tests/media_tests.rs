//! Integration tests for the media domain module (MediaId, MediaItem).

use stui_runtime::ipc::MediaType;
use stui_runtime::media::id::MediaId;

// ── MediaId ───────────────────────────────────────────────────────────────────

#[test]
fn test_media_id_parse_namespaced() {
    let id = MediaId::parse("tmdb:tt0816692");
    assert_eq!(id.namespace, "tmdb");
    assert_eq!(id.key, "tt0816692");
}

#[test]
fn test_media_id_parse_multi_colon() {
    // "tmdb:movie:123" — namespace is "tmdb", key is "movie:123"
    let id = MediaId::parse("tmdb:movie:123");
    assert_eq!(id.namespace, "tmdb");
    assert_eq!(id.key, "movie:123");
}

#[test]
fn test_media_id_parse_no_colon() {
    let id = MediaId::parse("tt0816692");
    assert_eq!(id.namespace, "unknown");
    assert_eq!(id.key, "tt0816692");
}

#[test]
fn test_media_id_roundtrip() {
    let id = MediaId::new("imdb", "tt1234567");
    assert_eq!(id.to_string_id(), "imdb:tt1234567");
}

#[test]
fn test_media_id_display() {
    let id = MediaId::new("local", "/home/user/movie.mkv");
    assert_eq!(format!("{}", id), "local:/home/user/movie.mkv");
}

#[test]
fn test_media_id_from_string() {
    let id: MediaId = "prowlarr:abc123def".into();
    assert_eq!(id.namespace, "prowlarr");
    assert_eq!(id.key, "abc123def");
}

#[test]
fn test_media_id_equality() {
    let a = MediaId::new("tmdb", "123");
    let b = MediaId::parse("tmdb:123");
    assert_eq!(a, b);
}

#[test]
fn test_media_id_inequality() {
    let a = MediaId::new("tmdb", "123");
    let b = MediaId::new("imdb", "123");
    assert_ne!(a, b);
}

// ── MediaType ─────────────────────────────────────────────────────────────────

#[test]
fn test_media_type_from_tab_movies() {
    use stui_runtime::ipc::MediaTab;
    let t = MediaType::from_tab(&MediaTab::Movies);
    assert_eq!(t, MediaType::Movie);
}

#[test]
fn test_media_type_from_tab_series() {
    use stui_runtime::ipc::MediaTab;
    let t = MediaType::from_tab(&MediaTab::Series);
    assert_eq!(t, MediaType::Series);
}
