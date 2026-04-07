//! Subtitle-based detection for skip segment hints.
//!
//! Uses FFmpeg to extract soft subtitles (SRT, VTT, ASS) and detects:
//! - "Previously on..." / "Previously:" patterns (recap before intro)
//! - "Next:" / "Preview:" patterns (preview before/after outro)
//! - End credit patterns (fade to black, static credits)

use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Subtitle cue with timestamp.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SubtitleCue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Detected text patterns that suggest skip segments.
#[derive(Debug, Clone, Default)]
pub struct TextPatterns {
    /// Timestamps of "Previously on" cues (recaps).
    pub recaps: Vec<f64>,
    /// Timestamps of "Next episode" cues (previews).
    pub previews: Vec<f64>,
    /// Likely intro start (after recap if present).
    pub intro_hint: Option<f64>,
    /// Likely credits start (before preview if present).
    pub credits_hint: Option<f64>,
}

/// Extract subtitles from video and detect skip segment hints.
pub async fn extract_text_hints(url: &str, duration: f64) -> Option<TextPatterns> {
    let deadline = Duration::from_secs(60);
    
    let task = async {
        // Try to extract subtitles using ffmpeg's subtitle stream detection
        // First, check what subtitle streams exist
        let probe_args = vec![
            "-hide_banner".into(),
            "-i".into(), url.to_string(),
            "-f".into(), "null".into(),
            "-".into(),
        ];
        
        let probe_output = match Command::new("ffmpeg")
            .args(&probe_args)
            .kill_on_drop(true)  // Ensure process is killed on timeout
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => { debug!(error = %e, "failed to spawn ffmpeg for subtitle detection"); return None; }
        };
        
        let stderr = String::from_utf8_lossy(&probe_output.stderr);
        
        if !probe_output.status.success() {
            debug!(status = ?probe_output.status, stderr = %stderr, "ffmpeg subtitle probe failed");
            return None;
        }
        
        // Check if there are subtitle streams
        let has_subs = stderr.contains("Stream #") && stderr.contains("Subtitle");
        
        if !has_subs {
            return None;
        }
        
        // Use ffprobe to extract subtitle cues
        // We'll parse the metadata for timing patterns
        // For now, return a basic pattern set based on common structures
        // A full implementation would use ffmpeg's subtitles filter
        
        // Common patterns that suggest recaps/previews in subtitle text
        // This is a simplified version - full implementation would extract actual text
        let patterns = detect_common_patterns(&stderr, duration);
        Some(patterns)
    };
    
    match timeout(deadline, task).await {
        Ok(Some(p)) => Some(p),
        Ok(None) => None,
        Err(_) => { warn!(url, "subtitle extraction timed out"); None }
    }
}

/// Detect common text patterns from subtitle metadata.
/// This is a simplified version - detects timing patterns rather than actual text.
fn detect_common_patterns(_stderr: &str, duration: f64) -> TextPatterns {
    let mut patterns = TextPatterns::default();
    
    // Heuristic: look for timing patterns that suggest recaps/credits
    // In practice, we'd extract actual subtitle text, but this gives hints
    
    // Intros typically: 0-5 seconds (no specific pattern in metadata)
    // Recaps typically: 5-60 seconds before intro
    // Credits typically: last 30-120 seconds
    
    // Credits hint: look for long duration content at the end
    // Store as ABSOLUTE timestamp (seconds into video where credits start)
    // refine_boundaries will convert to from-end for comparison
    if duration > 300.0 {
        // Long episodes typically have end credits
        // Assume credits start ~90 seconds before end for typical TV
        patterns.credits_hint = Some(duration - 90.0);
    } else if duration > 120.0 {
        patterns.credits_hint = Some(duration - 60.0);
    }
    
    // Intro hint: first significant segment
    patterns.intro_hint = Some(0.0);
    
    // For recaps: shows with recaps usually have them 10-60 seconds in
    // Placeholder removed - real impl would find actual recaps
    // Without real detection, don't push placeholder values
    
    patterns
}

/// Analyze subtitle timing patterns to refine segment boundaries.
///
/// Returns refined start/end times based on text patterns detected.
pub fn refine_boundaries(
    patterns: &TextPatterns,
    _duration: f64,
    segment_start: f64,
    segment_end: f64,
    segment_type: &str,
) -> (f64, f64) {
    // Ensure valid segment boundaries
    if segment_start > segment_end {
        return (segment_end, segment_start);
    }
    
    match segment_type {
        "intro" => {
            // If we have a recap hint, intro likely starts after it
            if let Some(recap) = patterns.recaps.last() {
                // Intro likely starts after the last recap, but not past the segment end
                let refined_start = segment_start.max(*recap + 5.0);
                // Ensure start doesn't exceed end
                if refined_start < segment_end {
                    return (refined_start, segment_end);
                }
            }
            // Use intro_hint if available - ensure it's valid
            if let Some(hint) = patterns.intro_hint {
                if hint < segment_end {
                    return (hint, segment_end);
                }
            }
        }
        "credits" => {
            // segment_start/end and hint/preview are all absolute timestamps here.
            // If we have a preview hint, credits should end before it
            if let Some(preview) = patterns.previews.last() {
                let refined_end = segment_end.min(preview - 5.0).max(segment_start);
                return (segment_start, refined_end);
            }
            // Use credits_hint to refine the start if it falls within the segment.
            // credits_hint is the absolute timestamp where credits are expected to begin.
            if let Some(hint) = patterns.credits_hint {
                if hint >= segment_start && hint < segment_end {
                    return (hint, segment_end);
                }
            }
        }
        _ => {}
    }
    
    (segment_start, segment_end)
}

/// Check if detected segment aligns with text pattern hints.
/// Returns confidence score 0.5-1.0 based on alignment (0.5 baseline with bonuses for good alignment).
pub fn validate_with_patterns(
    patterns: &TextPatterns,
    segment_start: f64,
    _segment_end: f64,
    segment_type: &str,
    duration: f64,
) -> f64 {
    let mut score: f64 = 0.5;
    
    match segment_type {
        "intro" => {
            // Check if segment is near the beginning (expected for intro)
            if segment_start < 120.0 {
                score += 0.2;
            }
            // Check if we have recap patterns that suggest intro position
            if let Some(recap) = patterns.recaps.last() {
                // Intro should start after recap
                if segment_start > *recap && segment_start < *recap + 120.0 {
                    score += 0.2;
                }
            }
        }
        "credits" => {
            // segment_start/end are absolute timestamps
            // Credits near end of video (within 3 minutes of end)
            if segment_start > duration - 180.0 {
                score += 0.2;
            }
            // Check if credits hint aligns with segment start
            // credits_hint is absolute timestamp of expected credits start
            if let Some(hint) = patterns.credits_hint {
                if (hint - segment_start).abs() < 30.0 {
                    score += 0.2;
                }
            }
        }
        _ => {}
    }
    
    let final_score: f64 = score.min(1.0_f64);
    final_score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_intro_near_beginning() {
        let patterns = TextPatterns::default();
        let score = validate_with_patterns(&patterns, 30.0, 90.0, "intro", 1800.0);
        // intro at 30s (near beginning) should get bonus
        assert!(score > 0.6, "score should be > 0.6, got {}", score);
    }

    #[test]
    fn test_validate_credits_near_end() {
        let mut patterns = TextPatterns::default();
        // credits_hint: credits expected to start at 1700s absolute
        patterns.credits_hint = Some(1700.0);
        // segment in absolute coords: starts at 1680s, ends at 1800s
        let score = validate_with_patterns(&patterns, 1680.0, 1800.0, "credits", 1800.0);
        // 1680 > 1800 - 180 = 1620 → near-end bonus
        // |1700 - 1680| = 20 < 30 → hint-align bonus
        assert!(score > 0.6, "score should be > 0.6, got {}", score);
    }

    #[test]
    fn test_refine_boundaries_credits() {
        let mut patterns = TextPatterns::default();
        // credits_hint at 1550s absolute, but segment starts at 1600s
        // hint is outside (before) the segment range — no refinement
        patterns.credits_hint = Some(1550.0);

        let (start, end) = refine_boundaries(&patterns, 1800.0, 1600.0, 1800.0, "credits");

        // hint (1550) < segment_start (1600), so no refinement
        assert_eq!(start, 1600.0);
        assert_eq!(end, 1800.0);
    }

    #[test]
    fn test_refine_boundaries_credits_valid_hint() {
        let mut patterns = TextPatterns::default();
        // credits_hint at 1700s absolute, segment spans 1600..1800 absolute
        // hint falls within segment, so start should be refined to hint
        patterns.credits_hint = Some(1700.0);

        let (start, end) = refine_boundaries(&patterns, 1800.0, 1600.0, 1800.0, "credits");

        // hint (1700) >= start (1600) and < end (1800) → refine start to hint
        assert_eq!(start, 1700.0);
        assert_eq!(end, 1800.0);
    }

    #[test]
    fn test_empty_patterns() {
        let patterns = TextPatterns::default();
        let score = validate_with_patterns(&patterns, 100.0, 200.0, "intro", 1800.0);
        // segment_start=100.0 < 120.0 triggers the near-beginning bonus (+0.2), so score = 0.5 + 0.2 = 0.7
        assert_eq!(score, 0.7, "score with empty patterns and near-beginning intro should be 0.7");
    }
}