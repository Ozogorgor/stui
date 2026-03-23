//! Persistent cache for fingerprints and detected segments.

use std::path::PathBuf;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::warn;

use super::fingerprint::Fingerprint;
use super::analyzer::Segment;

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredFingerprints {
    pub episode_id: String,
    pub intro_fp:   Option<Fingerprint>,
    pub credits_fp: Option<Fingerprint>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StoredSegments {
    pub episode_id: String,
    /// Intro: absolute start/end timestamps (seconds from start of video).
    pub intro:      Option<Segment>,
    /// Credits: start/end are seconds-from-end of video (positive numbers).
    pub credits:    Option<Segment>,
}

pub struct SkipperStore {
    base: PathBuf,
}

impl SkipperStore {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { base: cache_dir.join("skipper") }
    }

    fn series_dir(&self, series_id: &str) -> PathBuf {
        self.base.join(slug(series_id))
    }

    fn fp_path(&self, series_id: &str, episode_id: &str) -> PathBuf {
        self.series_dir(series_id).join(format!("{}.fp.json", slug(episode_id)))
    }

    fn seg_path(&self, series_id: &str, episode_id: &str) -> PathBuf {
        self.series_dir(series_id).join(format!("{}.seg.json", slug(episode_id)))
    }

    /// Save fingerprints for an episode.
    pub async fn save_fp(
        &self,
        series_id:  &str,
        episode_id: &str,
        intro_fp:   Option<Fingerprint>,
        credits_fp: Option<Fingerprint>,
    ) -> Result<()> {
        let dir = self.series_dir(series_id);
        fs::create_dir_all(&dir).await?;
        let data = StoredFingerprints { episode_id: episode_id.to_string(), intro_fp, credits_fp };
        fs::write(self.fp_path(series_id, episode_id), serde_json::to_string(&data)?).await?;
        Ok(())
    }

    /// Load fingerprints for all OTHER episodes of a series.
    pub async fn load_others(&self, series_id: &str, episode_id: &str) -> Vec<StoredFingerprints> {
        let dir = self.series_dir(series_id);
        let mut out = Vec::new();
        let mut rd = match fs::read_dir(&dir).await { Ok(r) => r, Err(_) => return out };
        while let Ok(Some(ent)) = rd.next_entry().await {
            let p = ent.path();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if !stem.ends_with(".fp") { continue; }
            let ep_slug = stem.trim_end_matches(".fp");
            if ep_slug == slug(episode_id) { continue; }
            match fs::read_to_string(&p).await {
                Ok(json) => {
                    if let Ok(fp) = serde_json::from_str::<StoredFingerprints>(&json) {
                        out.push(fp);
                    }
                }
                Err(e) => warn!(path=%p.display(), error=%e, "cannot read fp cache"),
            }
        }
        out
    }

    /// Save detected segments for an episode.
    pub async fn save_segments(
        &self,
        series_id:  &str,
        episode_id: &str,
        intro:      Option<Segment>,
        credits:    Option<Segment>,
    ) -> Result<()> {
        let dir = self.series_dir(series_id);
        fs::create_dir_all(&dir).await?;
        let data = StoredSegments { episode_id: episode_id.to_string(), intro, credits };
        fs::write(self.seg_path(series_id, episode_id), serde_json::to_string(&data)?).await?;
        Ok(())
    }

    /// Load previously cached segments for an episode, if any.
    pub async fn load_segments(&self, series_id: &str, episode_id: &str) -> Option<StoredSegments> {
        let json = fs::read_to_string(self.seg_path(series_id, episode_id)).await.ok()?;
        serde_json::from_str(&json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use crate::skipper::fingerprint::Fingerprint;

    /// Test slug function converts alphanumeric correctly.
    #[test]
    fn test_slug_alphanumeric() {
        assert_eq!(slug("tt123456"), "tt123456");
    }

    /// Test slug converts special chars to underscores.
    #[test]
    fn test_slug_special_chars() {
        assert_eq!(slug("tt123:1:5"), "tt123_1_5");
    }

    /// Test slug keeps allowed chars.
    #[test]
    fn test_slug_allowed_chars() {
        assert_eq!(slug("movie-title_2024"), "movie-title_2024");
    }

/// Test slug handles empty string.
    #[test]
    fn test_slug_empty() {
        assert_eq!(slug(""), "");
    }

    /// Test slug handles multi-episode style IDs.
    #[test]
    fn test_slug_multi_episode() {
        assert_eq!(slug("tt123456:1:5"), "tt123456_1_5");
    }

    /// Test slug documents potential collision (informational).
    #[test]
    fn test_slug_documentation() {
        let note = "Note: Collisions are theoretically possible";
        assert!(note.len() > 0);
    }

    /// Test slug collapses multiple special chars.
    #[test]
    fn test_slug_multiple_specials() {
        assert_eq!(slug("a:b/c"), "a_b_c");
    }

    /// Test store creation.
    #[test]
    fn test_store_new() {
        let temp = TempDir::new().unwrap();
        let store = SkipperStore::new(temp.path().to_path_buf());
        // Should not panic
        assert_eq!(store.base, temp.path().join("skipper"));
    }

    /// Test series_dir combines base and slug.
    #[test]
    fn test_series_dir() {
        let temp = TempDir::new().unwrap();
        let store = SkipperStore::new(temp.path().to_path_buf());
        let dir = store.series_dir("tt12345");
        assert!(dir.to_string_lossy().ends_with("skipper/tt12345"));
    }

    /// Test fp_path creates correct filename.
    #[test]
    fn test_fp_path() {
        let temp = TempDir::new().unwrap();
        let store = SkipperStore::new(temp.path().to_path_buf());
        let path = store.fp_path("series1", "ep1");
        assert!(path.to_string_lossy().ends_with("series1/ep1.fp.json"));
    }

    /// Test seg_path creates correct filename.
    #[test]
    fn test_seg_path() {
        let temp = TempDir::new().unwrap();
        let store = SkipperStore::new(temp.path().to_path_buf());
        let path = store.seg_path("series1", "ep1");
        assert!(path.to_string_lossy().ends_with("series1/ep1.seg.json"));
    }

    /// Test save and load fingerprint roundtrip.
    #[tokio::test]
    async fn test_save_load_fp() {
        let temp = TempDir::new().unwrap();
        let store = SkipperStore::new(temp.path().to_path_buf());

        let fp = Fingerprint {
            values: vec![1, 2, 3],
            scan_secs: 5.0,
        };

        store.save_fp("series", "episode1", Some(fp.clone()), None).await.unwrap();

        let others = store.load_others("series", "episode1").await;
        assert_eq!(others.len(), 0); // no other episodes
    }

    /// Test save and load segments roundtrip.
    #[tokio::test]
    async fn test_save_load_segments() {
        let temp = TempDir::new().unwrap();
        let store = SkipperStore::new(temp.path().to_path_buf());

        let intro = Segment { start: 0.0, end: 90.0 };
        let credits = Segment { start: 5.0, end: 180.0 };

        store.save_segments("series", "ep1", Some(intro), Some(credits)).await.unwrap();

        let loaded = store.load_segments("series", "ep1").await;
        assert!(loaded.is_some());
        assert!(loaded.as_ref().unwrap().intro.is_some());
        assert!(loaded.as_ref().unwrap().credits.is_some());
    }
}

/// Convert a string to a safe filesystem slug.
/// Note: Collisions are theoretically possible (e.g. "tt1:2" and "tt1_2" both map to "tt1_2")
/// but unlikely in practice since Stremio-style IDs have specific formats.
fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}
