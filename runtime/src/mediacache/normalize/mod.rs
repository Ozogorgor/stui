//! Music tag normalization pipeline.
//!
//! Pure functions only. No I/O. See docs/superpowers/specs/2026-04-15-music-tag-normalization-design.md.

pub mod exceptions;
pub mod lookup;
pub mod rules;
pub mod store;
pub mod unusual_case;
pub mod year;

use exceptions::ExceptionList;
use lookup::{overwrite_if_empty, LookupResult};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RawTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub date: String,
    pub genre: String,
    pub track: String,
    pub disc: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NormalizedTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub year: String,
    pub genre: String,
    pub track: u32,
    pub disc: u32,
}

pub struct NormalizationConfig<'a> {
    pub enabled: bool,
    pub use_lookup: bool,
    pub exceptions: &'a ExceptionList,
}

/// Apply the pipeline.
///
/// When `cfg.enabled == false`: returns a trivial conversion (year extracted,
/// numerics parsed). No casing/whitespace normalization. This preserves
/// existing behavior for the disabled path.
pub fn normalize(
    raw: &RawTags,
    cfg: &NormalizationConfig,
    lookup: Option<&LookupResult>,
) -> NormalizedTags {
    let mut artist = raw.artist.clone();
    let mut album_artist = raw.album_artist.clone();
    let mut album = raw.album.clone();
    let mut title = raw.title.clone();
    let mut date = raw.date.clone();
    let mut genre = raw.genre.clone();

    if cfg.enabled && cfg.use_lookup {
        if let Some(l) = lookup {
            overwrite_if_empty(&mut artist, l.artist.as_deref());
            overwrite_if_empty(&mut album_artist, l.album_artist.as_deref());
            overwrite_if_empty(&mut album, l.album.as_deref());
            overwrite_if_empty(&mut title, l.title.as_deref());
            overwrite_if_empty(&mut date, l.year.as_deref());
            overwrite_if_empty(&mut genre, l.genre.as_deref());
        }
    }

    if cfg.enabled {
        if !cfg.exceptions.is_artist_protected(&raw.artist) {
            artist = rules::smart_title_case(&artist);
        }
        if !cfg.exceptions.is_album_artist_protected(&raw.album_artist) {
            album_artist = rules::smart_title_case(&album_artist);
        }
        if !cfg.exceptions.is_album_protected(&raw.album) {
            album = rules::smart_title_case(&album);
        }
        if !cfg.exceptions.is_title_protected(&raw.title) {
            title = rules::smart_title_case(&title);
        }
        if !cfg.exceptions.is_genre_protected(&raw.genre) {
            genre = rules::smart_title_case(&genre);
        }
    }

    NormalizedTags {
        artist,
        album_artist,
        album,
        title,
        year: year::extract_year(&date),
        genre,
        track: rules::parse_track_or_disc(&raw.track),
        disc: rules::parse_track_or_disc(&raw.disc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_exceptions() -> ExceptionList {
        ExceptionList::default()
    }
    fn cfg_on<'a>(ex: &'a ExceptionList) -> NormalizationConfig<'a> {
        NormalizationConfig {
            enabled: true,
            use_lookup: false,
            exceptions: ex,
        }
    }
    fn cfg_off<'a>(ex: &'a ExceptionList) -> NormalizationConfig<'a> {
        NormalizationConfig {
            enabled: false,
            use_lookup: false,
            exceptions: ex,
        }
    }

    #[test]
    fn disabled_extracts_year_only() {
        let raw = RawTags {
            artist: "pink floyd".into(),
            album: "the wall".into(),
            date: "1979-11-30".into(),
            track: "3/12".into(),
            ..Default::default()
        };
        let ex = empty_exceptions();
        let out = normalize(&raw, &cfg_off(&ex), None);
        assert_eq!(out.artist, "pink floyd");
        assert_eq!(out.album, "the wall");
        assert_eq!(out.year, "1979");
        assert_eq!(out.track, 3);
    }

    #[test]
    fn enabled_title_cases() {
        let raw = RawTags {
            artist: "pink floyd".into(),
            album: "the wall".into(),
            ..Default::default()
        };
        let ex = empty_exceptions();
        let out = normalize(&raw, &cfg_on(&ex), None);
        assert_eq!(out.artist, "Pink Floyd");
        assert_eq!(out.album, "The Wall");
    }

    #[test]
    fn exception_vetoes_artist() {
        let raw = RawTags {
            artist: "deadmau5".into(),
            album: "random album name".into(),
            ..Default::default()
        };
        let mut ex = ExceptionList::default();
        ex.artist.insert("deadmau5".to_string());
        let out = normalize(&raw, &cfg_on(&ex), None);
        assert_eq!(out.artist, "deadmau5");
        assert_eq!(out.album, "Random Album Name");
    }

    #[test]
    fn unusual_case_preserved_without_exception() {
        let raw = RawTags {
            artist: "AC/DC".into(),
            ..Default::default()
        };
        let ex = empty_exceptions();
        let out = normalize(&raw, &cfg_on(&ex), None);
        assert_eq!(out.artist, "AC/DC");
    }

    #[test]
    fn lookup_fills_missing_fields() {
        let raw = RawTags {
            artist: "pink floyd".into(),
            ..Default::default()
        };
        let look = LookupResult {
            album: Some("the wall".into()),
            ..Default::default()
        };
        let ex = empty_exceptions();
        let cfg = NormalizationConfig {
            enabled: true,
            use_lookup: true,
            exceptions: &ex,
        };
        let out = normalize(&raw, &cfg, Some(&look));
        assert_eq!(out.album, "The Wall");
    }

    #[test]
    fn lookup_does_not_overwrite() {
        let raw = RawTags {
            album: "Already Here".into(),
            ..Default::default()
        };
        let look = LookupResult {
            album: Some("Different Value".into()),
            ..Default::default()
        };
        let ex = empty_exceptions();
        let cfg = NormalizationConfig {
            enabled: true,
            use_lookup: true,
            exceptions: &ex,
        };
        let out = normalize(&raw, &cfg, Some(&look));
        assert_eq!(out.album, "Already Here");
    }

    #[test]
    fn lookup_full_date_still_year_extracted() {
        let raw = RawTags::default();
        let look = LookupResult {
            year: Some("2017-05-03".into()),
            ..Default::default()
        };
        let ex = empty_exceptions();
        let cfg = NormalizationConfig {
            enabled: true,
            use_lookup: true,
            exceptions: &ex,
        };
        let out = normalize(&raw, &cfg, Some(&look));
        assert_eq!(out.year, "2017");
    }
}
