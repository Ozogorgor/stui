//! Quality score computation for a single stream.

use crate::providers::{HdrFormat, Stream, StreamQuality};
use super::policy::RankingPolicy;

/// Composite quality score.  All sub-scores are in 0–N points; `total()` sums them.
#[derive(Debug, Clone, Default)]
pub struct QualityScore {
    pub resolution: u32, // 0–400
    pub codec:      u32, // 0–150
    pub seeders:    u32, // 0–150
    pub bitrate:    u32, // 0–150
    pub source:     u32, // 0–100
    pub hdr_bonus:  u32, // 0–50
}

impl QualityScore {
    pub fn total(&self) -> u32 {
        self.resolution + self.codec + self.seeders + self.bitrate + self.source + self.hdr_bonus
    }

    /// Compute a score for `stream` according to `policy`.
    ///
    /// Explicit metadata fields on `Stream` are used when present; name-string
    /// heuristics serve as fallbacks for streams that don't populate them.
    pub fn from_stream(stream: &Stream, policy: &RankingPolicy) -> Self {
        let name_up = stream.name.to_uppercase();

        // ── Resolution ────────────────────────────────────────────────────
        let resolution = match stream.quality {
            StreamQuality::Uhd4k  => policy.resolution_weights[3],
            StreamQuality::Hd1080 => policy.resolution_weights[2],
            StreamQuality::Hd720  => policy.resolution_weights[1],
            StreamQuality::Sd     => policy.resolution_weights[0],
            StreamQuality::Unknown => {
                // Fall back to name-string inference
                if name_up.contains("2160") || name_up.contains("4K") {
                    policy.resolution_weights[3]
                } else if name_up.contains("1080") {
                    policy.resolution_weights[2]
                } else if name_up.contains("720") {
                    policy.resolution_weights[1]
                } else {
                    50
                }
            }
        };

        // ── Codec — explicit field wins, name-parse as fallback ───────────
        let codec = if let Some(ref c) = stream.codec {
            match c.to_uppercase().as_str() {
                "AV1"                    => 150,
                "HEVC" | "H265" | "X265" => 120,
                "H264" | "X264" | "AVC"  => 90,
                _                        => 50,
            }
        } else if name_up.contains("AV1") {
            150
        } else if name_up.contains("HEVC") || name_up.contains("H265") || name_up.contains("X265") {
            120
        } else if name_up.contains("H264") || name_up.contains("X264") || name_up.contains("AVC") {
            90
        } else {
            50
        };

        // ── Seeders — explicit field wins, name-parse as fallback ─────────
        let seeders = stream.seeders
            .or_else(|| extract_seeders(&stream.name))
            .map(|s| {
                // Log-scale: 150 pts at 100+ seeds, 75 pts at 10 seeds
                let capped = s.min(100) as f64;
                (capped.ln_1p() / 100_f64.ln_1p() * 150.0) as u32
            })
            .unwrap_or(0);

        // ── Bitrate — explicit field wins, name-parse as fallback ─────────
        let bitrate = stream.bitrate_kbps
            .or_else(|| extract_bitrate_kbps(&stream.name))
            .map(|kbps| {
                let capped = kbps.min(40_000) as f64;
                (capped / 40_000.0 * 150.0) as u32
            })
            .unwrap_or(0);

        // ── Source ────────────────────────────────────────────────────────
        let source = if name_up.contains("BLURAY") || name_up.contains("BLU-RAY") || name_up.contains("BDREMUX") {
            100
        } else if name_up.contains("WEBDL") || name_up.contains("WEB-DL") {
            80
        } else if name_up.contains("WEBRIP") {
            70
        } else if name_up.contains("HDTV") {
            60
        } else if name_up.contains("DVDRIP") {
            40
        } else if name_up.contains("CAM") || name_up.contains("HDCAM") {
            5
        } else {
            50
        };

        // ── HDR bonus — explicit enum wins, name-parse as fallback ────────
        let hdr_bonus = if stream.hdr != HdrFormat::None {
            stream.hdr.score()
        } else {
            HdrFormat::from_name(&stream.name).score()
        };

        QualityScore { resolution, codec, seeders, bitrate, source, hdr_bonus }
    }
}

fn extract_seeders(name: &str) -> Option<u32> {
    // Look for patterns like "432 seeds", "432 seeders", "Seeds: 432"
    let lower = name.to_lowercase();
    for pattern in &["seeds", "seeders", "peers"] {
        if let Some(pos) = lower.find(pattern) {
            // scan backwards for digits
            let prefix = &lower[..pos].trim_end();
            let digits: String = prefix.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                let reversed: String = digits.chars().rev().collect();
                return reversed.parse().ok();
            }
        }
    }
    None
}

fn extract_bitrate_kbps(name: &str) -> Option<u32> {
    // Look for "12000kbps", "12 Mbps", "12mbps"
    let lower = name.to_lowercase();
    if let Some(pos) = lower.find("mbps") {
        let prefix = lower[..pos].trim_end();
        let digits: String = prefix.chars().rev()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !digits.is_empty() {
            let s: String = digits.chars().rev().collect();
            return s.parse::<f64>().ok().map(|v| (v * 1000.0) as u32);
        }
    }
    if let Some(pos) = lower.find("kbps") {
        let prefix = lower[..pos].trim_end();
        let digits: String = prefix.chars().rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            let s: String = digits.chars().rev().collect();
            return s.parse().ok();
        }
    }
    None
}
