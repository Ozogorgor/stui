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

/// Convert a string to a safe filesystem slug.
fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}
