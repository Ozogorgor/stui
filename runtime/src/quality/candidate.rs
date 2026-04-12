//! A stream candidate = stream + computed quality score.

use super::QualityScore;
use crate::providers::{HdrFormat, Stream};

#[allow(dead_code)] // pub API: used by stream ranking and TUI
#[derive(Debug, Clone)]
pub struct StreamCandidate {
    pub stream: Stream,
    pub score: QualityScore,
}

impl StreamCandidate {
    #[allow(dead_code)] // pub API: used by stream ranking and TUI
    /// Human-readable badge string for the UI, e.g. "1080p HEVC WEB-DL HDR10 ★ 847"
    ///
    /// Uses explicit metadata fields when available, falls back to name parsing.
    pub fn badge(&self) -> String {
        let s = &self.stream;
        let mut parts = vec![];

        // Resolution label
        parts.push(s.quality.label().to_string());

        // Codec — prefer explicit field
        if let Some(ref c) = s.codec {
            parts.push(c.clone());
        } else if let Some(c) = extract_codec(&s.name) {
            parts.push(c);
        }

        // Source (WEB-DL, BluRay, etc.) — always from name
        if let Some(src) = extract_source(&s.name) {
            parts.push(src);
        }

        // HDR — prefer explicit field
        let hdr_label = match s.hdr {
            HdrFormat::DolbyVision => Some("DV"),
            HdrFormat::Hdr10Plus => Some("HDR10+"),
            HdrFormat::Hdr10 => Some("HDR"),
            HdrFormat::None => {
                let n = s.name.to_uppercase();
                if n.contains("DOLBY VISION") || n.contains(" DV ") {
                    Some("DV")
                } else if n.contains("HDR10+") {
                    Some("HDR10+")
                } else if n.contains("HDR") {
                    Some("HDR")
                } else {
                    None
                }
            }
        };
        if let Some(h) = hdr_label {
            parts.push(h.to_string());
        }

        // Seeders — explicit field
        if let Some(seeds) = s.seeders {
            parts.push(format!("↑{seeds}"));
        }

        format!("{} ★ {}", parts.join(" "), self.score.total())
    }
}

#[allow(dead_code)] // pub API: used by stream ranking and TUI
fn extract_codec(name: &str) -> Option<String> {
    let n = name.to_uppercase();
    if n.contains("AV1") {
        return Some("AV1".into());
    }
    if n.contains("HEVC") || n.contains("H265") || n.contains("X265") {
        return Some("HEVC".into());
    }
    if n.contains("H264") || n.contains("X264") || n.contains("AVC") {
        return Some("H264".into());
    }
    None
}

#[allow(dead_code)] // pub API: used by stream ranking and TUI
fn extract_source(name: &str) -> Option<String> {
    let n = name.to_uppercase();
    if n.contains("BLURAY") || n.contains("BLU-RAY") || n.contains("BDREMUX") {
        return Some("BluRay".into());
    }
    if n.contains("WEBDL") || n.contains("WEB-DL") {
        return Some("WEB-DL".into());
    }
    if n.contains("WEBRIP") || n.contains("WEB-RIP") {
        return Some("WEBRip".into());
    }
    if n.contains("HDTV") {
        return Some("HDTV".into());
    }
    if n.contains("DVDRIP") || n.contains("DVD-RIP") {
        return Some("DVDRip".into());
    }
    if n.contains("CAM") || n.contains("HDCAM") {
        return Some("CAM".into());
    }
    None
}
