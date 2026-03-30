//! FFmpeg/Chromaprint audio fingerprint extraction.

use std::time::Duration;
use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Raw Chromaprint fingerprint for one segment of audio.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    pub values:       Vec<u32>,
    pub scan_secs:    f64,  // how many seconds of audio were requested
}

impl Fingerprint {
    /// Approximate frames-per-second for this fingerprint.
    pub fn fps(&self) -> f64 {
        if self.scan_secs > 0.0 && !self.values.is_empty() {
            self.values.len() as f64 / self.scan_secs
        } else {
            3.0
        }
    }
}

/// Extract a fingerprint from the first `scan_secs` of `url`.
pub async fn extract_intro(url: &str, scan_secs: f64) -> Option<Fingerprint> {
    run_with_timeout(url, None, scan_secs).await
}

/// Extract a fingerprint from the last `scan_secs` of `url` (using ffmpeg -sseof).
pub async fn extract_credits(url: &str, scan_secs: f64) -> Option<Fingerprint> {
    run_with_timeout(url, Some(scan_secs), scan_secs).await
}

async fn run_with_timeout(url: &str, from_end: Option<f64>, scan_secs: f64) -> Option<Fingerprint> {
    const MAX_SCAN_SECS: f64 = 3600.0;
    let deadline = Duration::from_secs(
        (scan_secs.max(0.0).min(MAX_SCAN_SECS).ceil() as u64).saturating_add(90),
    );
    match timeout(deadline, run_ffmpeg(url, from_end, scan_secs)).await {
        Ok(Ok(fp))  => Some(fp),
        Ok(Err(e))  => { debug!(url, error=%e, "fingerprint extraction failed"); None }
        Err(_)      => { warn!(url, "fingerprint extraction timed out"); None }
    }
}

async fn run_ffmpeg(url: &str, from_end: Option<f64>, scan_secs: f64) -> Result<Fingerprint> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(), "-loglevel".into(), "error".into(),
    ];

    if let Some(secs) = from_end {
        args.push("-sseof".into());
        args.push(format!("-{secs}"));
    }

    args.extend(["-i".into(), url.to_string()]);
    args.extend(["-t".into(), format!("{scan_secs}")]);
    args.extend([
        "-vn".into(),
        "-acodec".into(), "pcm_s16le".into(),
        "-ar".into(),     "16000".into(),
        "-ac".into(),     "1".into(),
        "-f".into(),      "chromaprint".into(),
        "-fp_format".into(), "raw".into(),
        "pipe:1".into(),
    ]);

    let output = Command::new("ffmpeg")
        .args(&args)
        .output()
        .await
        .context("failed to spawn ffmpeg")?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffmpeg error: {}", err.lines().last().unwrap_or("(no output)"));
    }

    let raw = &output.stdout;
    if raw.len() < 4 {
        anyhow::bail!("ffmpeg produced no fingerprint output");
    }

    let values: Vec<u32> = raw
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();

    Ok(Fingerprint { values, scan_secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test Fingerprint fps calculation with normal values.
    #[test]
    fn test_fps_normal() {
        let fp = Fingerprint {
            values: vec![0u32; 30],
            scan_secs: 10.0,
        };
        assert!((fp.fps() - 3.0).abs() < 0.001);
    }

    /// Test Fingerprint fps with zero scan_secs defaults to 3.0.
    #[test]
    fn test_fps_zero_scan() {
        let fp = Fingerprint {
            values: vec![0u32; 30],
            scan_secs: 0.0,
        };
        assert!((fp.fps() - 3.0).abs() < 0.001);
    }

    /// Test Fingerprint fps with negative scan (shouldn't happen but defensive).
    #[test]
    fn test_fps_negative_scan() {
        let fp = Fingerprint {
            values: vec![0u32; 30],
            scan_secs: -5.0,
        };
        assert!((fp.fps() - 3.0).abs() < 0.001);
    }

    /// Test Fingerprint fps with empty values.
    #[test]
    fn test_fps_empty_values() {
        let fp = Fingerprint {
            values: vec![],
            scan_secs: 10.0,
        };
        assert!((fp.fps() - 3.0).abs() < 0.001);
    }

    /// Test Fingerprint values round-trip serialization.
    #[test]
    fn test_serialization() {
        let fp = Fingerprint {
            values: vec![1, 2, 3, 4],
            scan_secs: 5.0,
        };
        let json = serde_json::to_string(&fp).unwrap();
        let fp2: Fingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(fp.values, fp2.values);
        assert!((fp.scan_secs - fp2.scan_secs).abs() < 0.001);
    }
}
