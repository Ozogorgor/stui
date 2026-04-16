//! Lookup result type.
//!
//! v1: type is defined and wired into the pipeline, but callers always pass
//! `None` — per-recording lookup isn't exposed by any plugin yet. v2 will
//! add `fetch_batch()` to populate these from ListenBrainz/MusicBrainz.

#[derive(Debug, Clone, Default)]
pub struct LookupResult {
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
}

/// Copy `src` into `dst` only when `dst` is empty (or whitespace-only).
pub fn overwrite_if_empty(dst: &mut String, src: Option<&str>) {
    if dst.trim().is_empty() {
        if let Some(s) = src { *dst = s.to_string(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn fills_empty() {
        let mut s = String::new();
        overwrite_if_empty(&mut s, Some("Pink Floyd"));
        assert_eq!(s, "Pink Floyd");
    }
    #[test] fn preserves_existing() {
        let mut s = String::from("Pink Floyd");
        overwrite_if_empty(&mut s, Some("pink floyd"));
        assert_eq!(s, "Pink Floyd");
    }
    #[test] fn whitespace_treated_as_empty() {
        let mut s = String::from("   ");
        overwrite_if_empty(&mut s, Some("Pink Floyd"));
        assert_eq!(s, "Pink Floyd");
    }
}
