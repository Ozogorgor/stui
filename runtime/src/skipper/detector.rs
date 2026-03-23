//! Background skip-segment detector.

use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::config::types::SkipperConfig;
use super::fingerprint;
use super::analyzer::{self, Segment};
use super::store::SkipperStore;

#[derive(serde::Serialize)]
struct SkipSegmentWire {
    r#type:         &'static str,  // always "skip_segment"
    segment_type:   &'static str,  // "intro" | "credits"
    start:          f64,
    end:            f64,
    /// For credits: start/end are seconds-from-end. For intro: false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    from_end:       bool,
}

/// Background intro/credits detector.
pub struct Skipper {
    config: Arc<RwLock<SkipperConfig>>,
    store:  SkipperStore,
    ipc_tx: mpsc::Sender<String>,
}

impl Skipper {
    pub fn new(
        config: SkipperConfig,
        store:  SkipperStore,
        ipc_tx: mpsc::Sender<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config: Arc::new(RwLock::new(config)),
            store,
            ipc_tx,
        })
    }

    #[allow(dead_code)]
    /// Hot-update config (called by SetConfig IPC handler).
    pub async fn update_config(&self, cfg: SkipperConfig) {
        *self.config.write().await = cfg;
    }

    /// Main entry point — spawned in background when a play request fires.
    pub async fn analyze(&self, url: &str, entry_id: &str, imdb_id: &str) {
        let cfg = self.config.read().await.clone();
        if !cfg.enabled { return; }

        // Only HTTP/HTTPS — skip magnets/local paths
        if !url.starts_with("http://") && !url.starts_with("https://") {
            debug!(url, "skipper: skipping non-HTTP URL");
            return;
        }

        let series_id  = derive_series_id(entry_id, imdb_id);
        let episode_id = entry_id.to_string();

        info!(series_id, episode_id, "skipper: starting analysis");

        // Return immediately if we have cached segments for this episode
        if let Some(cached) = self.store.load_segments(&series_id, &episode_id).await {
            if let Some(ref intro) = cached.intro {
                self.push("intro", intro, false).await;
            }
            if let Some(ref credits) = cached.credits {
                self.push("credits", credits, true).await;
            }
            if cached.intro.is_some() || cached.credits.is_some() {
                info!(episode_id, "skipper: served segments from cache");
            }
            return;
        }

        // Extract intro and credits fingerprints in parallel
        let (intro_fp, credits_fp) = tokio::join!(
            fingerprint::extract_intro(url, cfg.intro_scan_secs as f64),
            fingerprint::extract_credits(url, cfg.credits_scan_secs as f64),
        );

        // Persist fingerprints regardless of comparison outcome
        if intro_fp.is_some() || credits_fp.is_some() {
            if let Err(e) = self.store.save_fp(
                &series_id, &episode_id, intro_fp.clone(), credits_fp.clone(),
            ).await {
                warn!(error=%e, "skipper: failed to save fingerprints");
            }
        }

        // Load all OTHER episodes for this series
        let others = self.store.load_others(&series_id, &episode_id).await;

        if others.len() + 1 < cfg.min_episodes {
            info!(series_id, have=others.len()+1, need=cfg.min_episodes,
                  "skipper: not enough episodes yet — waiting for more watches");
            return;
        }

        let mut det_intro:   Option<Segment> = None;
        let mut det_credits: Option<Segment> = None;

        // Intro: all fingerprints anchored at offset 0.0
        if let Some(ref ifp) = intro_fp {
            let other_intros: Vec<_> = others.iter()
                .filter_map(|o| o.intro_fp.as_ref().map(|fp| (fp.clone(), 0.0f64)))
                .collect();

            det_intro = analyzer::detect_segment(
                ifp, 0.0, &other_intros,
                cfg.min_intro_secs, cfg.max_intro_secs, cfg.similarity_threshold,
            );
        }

        // Credits: fingerprints are anchored at offset 0.0 within the credits window;
        // published timestamps are "seconds from end" (handled by TUI with from_end flag).
        if let Some(ref cfp) = credits_fp {
            let other_credits: Vec<_> = others.iter()
                .filter_map(|o| o.credits_fp.as_ref().map(|fp| (fp.clone(), 0.0f64)))
                .collect();

            // Detected segment is relative to start of the credits window.
            // Convert to "from_end": start_from_end = scan_secs - seg.end
            //                        end_from_end   = scan_secs - seg.start
            det_credits = analyzer::detect_segment(
                cfp, 0.0, &other_credits,
                cfg.min_credits_secs, cfg.max_credits_secs, cfg.similarity_threshold,
            ).map(|seg| {
                let scan = cfg.credits_scan_secs as f64;
                Segment {
                    start: (scan - seg.end).max(0.0),
                    end:   (scan - seg.start).max(0.0),
                }
            });
        }

        // Cache and push
        if let Err(e) = self.store.save_segments(
            &series_id, &episode_id, det_intro.clone(), det_credits.clone(),
        ).await {
            warn!(error=%e, "skipper: failed to cache segments");
        }

        if let Some(ref seg) = det_intro {
            info!(start=seg.start, end=seg.end, "skipper: intro detected");
            self.push("intro", seg, false).await;
        }
        if let Some(ref seg) = det_credits {
            info!(start=seg.start, end=seg.end, "skipper: credits detected (from_end)");
            self.push("credits", seg, true).await;
        }
        if det_intro.is_none() && det_credits.is_none() {
            info!(series_id, episode_id, "skipper: no common segments found");
        }
    }

    async fn push(&self, seg_type: &'static str, seg: &Segment, from_end: bool) {
        let wire = SkipSegmentWire {
            r#type:       "skip_segment",
            segment_type: seg_type,
            start:        seg.start,
            end:          seg.end,
            from_end,
        };
        if let Ok(json) = serde_json::to_string(&wire) {
            if let Err(e) = self.ipc_tx.send(json).await {
                warn!(segment_type=seg_type, error=%e, "failed to send skip segment to TUI");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test derive_series_id with plain imdb_id.
    #[test]
    fn test_derive_series_id_imdb() {
        assert_eq!(derive_series_id("movie123", "tt12345"), "tt12345");
    }

    /// Test derive_series_id prefers non-episode imdb_id.
    #[test]
    fn test_derive_series_id_imdb_no_colon() {
        assert_eq!(derive_series_id("foo:bar", "tt12345"), "tt12345");
    }

    /// Test derive_series_id falls back to entry_id with colon.
    #[test]
    fn test_derive_series_id_stremio_style() {
        assert_eq!(derive_series_id("tt123456:1:5", ""), "tt123456");
    }

    /// Test derive_series_id with slash.
    #[test]
    fn test_derive_series_id_slash() {
        assert_eq!(derive_series_id("tt123/season1/episode5", ""), "tt123");
    }

    /// Test derive_series_id with empty imdb and entry_id with colon.
    #[test]
    fn test_derive_series_id_fallback() {
        assert_eq!(derive_series_id("tt123:1:1", ""), "tt123");
    }

    /// Test derive_series_id uses entry_id when no separators.
    #[test]
    fn test_derive_series_id_plain() {
        assert_eq!(derive_series_id("abc123", ""), "abc123");
    }
}

/// Derive a series-level ID to group episodes of the same show.
///
/// Stremio-style IDs: "tt1234:1:1" → "tt1234"
/// Plain IDs: "tt5678" → "tt5678" (used as-is — could be a movie)
fn derive_series_id(entry_id: &str, imdb_id: &str) -> String {
    // Prefer imdb_id if it doesn't look like an episode-level ID
    if !imdb_id.is_empty() && !imdb_id.contains(':') {
        return imdb_id.to_string();
    }
    // Strip episode suffix at first colon or slash
    if let Some(pos) = entry_id.find(':').or_else(|| entry_id.find('/')) {
        return entry_id[..pos].to_string();
    }
    entry_id.to_string()
}
