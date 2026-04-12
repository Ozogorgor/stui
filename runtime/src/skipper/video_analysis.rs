//! Video scene detection and audio analysis enhancements.
//!
//! Provides additional signals beyond Chromaprint fingerprinting:
//! - Scene change detection for boundary refinement
//! - Audio energy/volume profile for consistent segments
//! - Silence detection for credits detection

use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Scene change information from video analysis.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // planned: skip-segment video analysis structs
pub struct SceneInfo {
    /// Timestamps of scene cuts (in seconds).
    pub cuts: Vec<f64>,
    /// Number of frames analyzed.
    pub frame_count: u64,
}

/// Audio energy profile for a segment.
#[derive(Debug, Clone)]
#[allow(dead_code)] // planned: skip-segment video analysis structs
pub struct AudioProfile {
    /// RMS energy per analysis window (0.1s windows).
    pub energy: Vec<f64>,
    /// Average energy level.
    pub avg_energy: f64,
    /// Peak energy level.
    pub peak_energy: f64,
    /// Fraction of silent windows (below threshold).
    pub silence_ratio: f64,
}

impl Default for AudioProfile {
    fn default() -> Self {
        Self {
            energy: Vec::new(),
            avg_energy: 0.0,
            peak_energy: 0.0,
            silence_ratio: 0.0,
        }
    }
}

/// Extract scene change timestamps from video.
/// Uses FFmpeg's scene detection with libavfilter.
pub async fn extract_scene_changes(url: &str, start_secs: f64, duration_secs: f64) -> Option<SceneInfo> {
    const MAX_DURATION_SECS: f64 = 3600.0;
    let deadline = Duration::from_secs(
        (duration_secs.max(0.0).min(MAX_DURATION_SECS).ceil() as u64).saturating_add(30),
    );
    
    let task = async {
        let args: Vec<String> = vec![
            "-hide_banner".into(), "-loglevel".into(), "warning".into(),
            "-ss".into(), format!("{start_secs}"),
            "-t".into(), format!("{duration_secs}"),
            "-i".into(), url.to_string(),
            "-filter_complex".into(), 
            "select='gt(scene,0.3)',metadata=print:csv".into(),
            "-f".into(), "null".into(),
            "-".into(),
        ];
        
        let output = match Command::new("ffmpeg")
            .args(&args)
            .kill_on_drop(true)  // Ensure process is killed on timeout
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => { debug!(error = %e, "failed to spawn ffmpeg for scene detection"); return None; }
        };
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(status = ?output.status, stderr = %stderr, "ffmpeg scene detection failed");
            return None;
        }
        
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut cuts = Vec::new();
        
        // Parse scene detection output lines like:
        // frame:   123 pts:   4101 pts_time:4.101 scene:0.42
        for line in stderr.lines() {
            if line.contains("pts_time") && line.contains("scene:") {
                if let Some(time) = line.split("pts_time:").nth(1) {
                    if let Some(pts) = time.split_whitespace().next() {
                        if let Ok(t) = pts.parse::<f64>() {
                            cuts.push(t + start_secs);
                        }
                    }
                }
            }
        }
        
        Some(SceneInfo {
            cuts,
            frame_count: 0,
        })
    };
    
    match timeout(deadline, task).await {
        Ok(Some(info)) => Some(info),
        Ok(None) => { debug!(url, "scene detection returned no results"); None }
        Err(_) => { warn!(url, "scene detection timed out"); None }
    }
}

/// Extract audio energy profile from a video segment.
/// Used to detect consistent volume patterns in intros/credits.
pub async fn extract_audio_profile(url: &str, start_secs: f64, duration_secs: f64) -> Option<AudioProfile> {
    const MAX_DURATION_SECS: f64 = 3600.0;
    let deadline = Duration::from_secs(
        (duration_secs.max(0.0).min(MAX_DURATION_SECS).ceil() as u64).saturating_add(30),
    );
    
    let task = async {
        let args: Vec<String> = vec![
            "-hide_banner".into(), "-loglevel".into(), "warning".into(),
            "-ss".into(), format!("{start_secs}"),
            "-t".into(), format!("{duration_secs}"),
            "-i".into(), url.to_string(),
            "-af".into(), "astats=metadata=1:reset=1,ametadata=print:csv".into(),
            "-f".into(), "null".into(),
            "-".into(),
        ];
        
        let output = match Command::new("ffmpeg")
            .args(&args)
            .kill_on_drop(true)  // Ensure process is killed on timeout
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => { debug!(error = %e, "failed to spawn ffmpeg for audio analysis"); return None; }
        };
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(status = ?output.status, stderr = %stderr, "ffmpeg audio analysis failed");
            return None;
        }
        
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut energy_values = Vec::new();
        
        // Parse RMS level from astats output
        // FFmpeg astats with metadata=1 outputs keys like:
        //   lavfi.astats.Overall.RMS_level=-18.5
        // or CSV format from ametadata=print:csv:
        //   0.000000,lavfi.astats.Overall.RMS_level,-18.500000
        for line in stderr.lines() {
            // Look for RMS_level key (not "RMS:")
            if let Some(eq_pos) = line.find("RMS_level=") {
                let value_part = &line[eq_pos + "RMS_level=".len()..];
                // Get the value (may have trailing chars)
                let db_val = value_part.split(',').next().unwrap_or(value_part).trim();
                if let Ok(db) = db_val.parse::<f64>() {
                    // Convert dB to linear (0-1 range)
                    let linear = 10.0_f64.powf(db / 20.0);
                    energy_values.push(linear);
                }
            }
        }
        
        if energy_values.is_empty() {
            return None;
        }
        
        let avg_energy = energy_values.iter().sum::<f64>() / energy_values.len() as f64;
        let peak_energy = energy_values.iter().cloned().fold(0.0_f64, f64::max);
        let silence_count = energy_values.iter().filter(|&&e| e < 0.01).count() as f64;
        let silence_ratio = silence_count / energy_values.len() as f64;
        
        Some(AudioProfile {
            energy: energy_values,
            avg_energy,
            peak_energy,
            silence_ratio,
        })
    };
    
    match timeout(deadline, task).await {
        Ok(Some(profile)) => Some(profile),
        Ok(None) => { debug!(url, "audio profile extraction returned no results"); None }
        Err(_) => { warn!(url, "audio profile extraction timed out"); None }
    }
}

/// Check if a timestamp is likely an intro/credits boundary based on scene changes.
/// Intros typically start with a scene cut at the beginning.
/// Credits often have few/no scene changes in the last 30-60 seconds.
#[allow(dead_code)] // Available for future use
pub fn estimate_boundaries(scene_info: &SceneInfo, duration: f64) -> (Option<f64>, Option<f64>) {
    let intro_boundary = scene_info.cuts.first();
    let credits_start = scene_info.cuts
        .iter()
        .rev()
        .find(|&&t| t < duration - 60.0);
    
    (intro_boundary.copied(), credits_start.copied())
}

/// Compare two audio profiles for similarity.
/// Returns 0.0 (identical) to 1.0 (completely different).
pub fn profile_similarity(a: &AudioProfile, b: &AudioProfile) -> f64 {
    if a.energy.is_empty() || b.energy.is_empty() {
        return 1.0;
    }
    
    // Normalize to same length by resampling (simple linear)
    let target_len = a.energy.len().min(b.energy.len()).max(1_usize);
    
    let a_sample: Vec<f64> = if a.energy.len() == target_len {
        a.energy.clone()
    } else {
        let step = a.energy.len() as f64 / target_len as f64;
        (0..target_len).map(|i| {
            let idx = ((i as f64 * step) as usize).min(a.energy.len().saturating_sub(1));
            a.energy[idx]
        }).collect()
    };
    
    let b_sample: Vec<f64> = if b.energy.len() == target_len {
        b.energy.clone()
    } else {
        let step = b.energy.len() as f64 / target_len as f64;
        (0..target_len).map(|i| {
            let idx = ((i as f64 * step) as usize).min(b.energy.len().saturating_sub(1));
            b.energy[idx]
        }).collect()
    };
    
    // RMS difference between profiles
    let diff: f64 = a_sample.iter()
        .zip(b_sample.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>() 
        / target_len as f64;
    
    // Normalize to 0-1 range (clamp at 1.0)
    diff.sqrt().min(1.0_f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_similarity_identical() {
        let a = AudioProfile {
            energy: vec![0.5, 0.5, 0.5, 0.5],
            avg_energy: 0.5,
            peak_energy: 0.5,
            silence_ratio: 0.0,
        };
        let b = a.clone();
        
        let sim = profile_similarity(&a, &b);
        assert!(sim < 0.01, "identical profiles should have ~0 difference");
    }

    #[test]
    fn test_profile_similarity_different() {
        let a = AudioProfile {
            energy: vec![0.1, 0.2, 0.1, 0.2],
            avg_energy: 0.15,
            peak_energy: 0.2,
            silence_ratio: 0.5,
        };
        let b = AudioProfile {
            energy: vec![0.9, 0.8, 0.9, 0.8],
            avg_energy: 0.85,
            peak_energy: 0.9,
            silence_ratio: 0.0,
        };
        
        let sim = profile_similarity(&a, &b);
        assert!(sim > 0.5, "different profiles should have high difference");
    }

    #[test]
    fn test_profile_similarity_empty() {
        let a = AudioProfile::default();
        let b = AudioProfile::default();
        
        let sim = profile_similarity(&a, &b);
        assert_eq!(sim, 1.0, "empty profiles should return max difference");
    }

    #[test]
    fn test_estimate_boundaries() {
        let info = SceneInfo {
            cuts: vec![0.5, 4.2, 120.0, 245.0],
            frame_count: 1000,
        };
        
        let (intro, credits) = estimate_boundaries(&info, 300.0);
        
        assert_eq!(intro, Some(0.5), "intro should start at first cut");
        assert_eq!(credits, Some(120.0), "credits should be after 60s from end");
    }
}