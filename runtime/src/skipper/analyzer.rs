//! Cross-episode fingerprint comparison to find recurring segments.
//! Also integrates video scene detection and audio profile analysis.

use super::fingerprint::Fingerprint;
use super::video_analysis::AudioProfile;

/// A detected temporal segment within an episode.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
}

impl Segment {
    #[allow(dead_code)] // pub API: used by skip-segment analyzer
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

/// Hamming distance between two 32-bit fingerprint values (0 = identical, 32 = all bits differ).
#[inline]
fn hamming(a: u32, b: u32) -> u32 {
    (a ^ b).count_ones()
}

/// Find the longest common sub-sequence between fingerprint arrays `a` and `b` using
/// a DP longest-common-substring approach.
///
/// `a_offset` / `b_offset` are the seconds into the episode where each fingerprint window begins
/// (0.0 for intro, `duration - scan_secs` for credits — caller must provide correct offset).
///
/// Returns the segment in episode A and episode B, or `None` if no match meeting the
/// duration requirements is found.
pub fn find_common(
    a: &Fingerprint,
    b: &Fingerprint,
    a_offset: f64,
    b_offset: f64,
    min_secs: f64,
    max_secs: f64,
    threshold: f64,
) -> Option<(Segment, Segment)> {
    // Per-frame "match" criterion: fraction of differing bits must be ≤ (1 - threshold)
    let max_dist = ((1.0_f64 - threshold) * 32.0) as u32;

    let av = &a.values;
    let bv = &b.values;
    let na = av.len();
    let nb = bv.len();
    if na < 4 || nb < 4 {
        return None;
    }

    // dp[i][j] = run length ending at a[i], b[j]
    // Flattened row-major.
    let mut dp = vec![0u16; na * nb];
    let mut best: u16 = 0;
    let mut bi = 0usize;
    let mut bj = 0usize;

    for i in 0..na {
        for j in 0..nb {
            if hamming(av[i], bv[j]) <= max_dist {
                let prev = if i > 0 && j > 0 {
                    dp[(i - 1) * nb + (j - 1)]
                } else {
                    0
                };
                let run = prev.saturating_add(1);
                dp[i * nb + j] = run;
                if run > best {
                    best = run;
                    bi = i;
                    bj = j;
                }
            }
        }
    }

    let fps_a = a.fps();
    let fps_b = b.fps();
    let run_a = best as f64 / fps_a;
    let run_b = best as f64 / fps_b;

    // Use the shorter run's duration to be conservative
    let run_secs = run_a.min(run_b);
    if run_secs < min_secs || run_secs > max_secs {
        return None;
    }

    let a_start = (bi + 1).saturating_sub(best as usize);
    let b_start = (bj + 1).saturating_sub(best as usize);

    Some((
        Segment {
            start: a_offset + a_start as f64 / fps_a,
            end: a_offset + (bi + 1) as f64 / fps_a,
        },
        Segment {
            start: b_offset + b_start as f64 / fps_b,
            end: b_offset + (bj + 1) as f64 / fps_b,
        },
    ))
}

/// Given fingerprints for multiple episodes, find the segment in the current episode
/// using a majority-vote across comparisons with other episodes.
pub fn detect_segment(
    current: &Fingerprint,
    current_offset: f64,
    others: &[(Fingerprint, f64)], // (fp, offset_secs)
    min_secs: f64,
    max_secs: f64,
    threshold: f64,
) -> Option<Segment> {
    let mut candidates: Vec<Segment> = Vec::new();

    for (other_fp, other_offset) in others {
        if let Some((seg, _)) = find_common(
            current,
            other_fp,
            current_offset,
            *other_offset,
            min_secs,
            max_secs,
            threshold,
        ) {
            candidates.push(seg);
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // Robust estimate: median start and end
    let mut starts: Vec<f64> = candidates.iter().map(|s| s.start).collect();
    let mut ends: Vec<f64> = candidates.iter().map(|s| s.end).collect();
    starts.sort_by(f64::total_cmp);
    ends.sort_by(f64::total_cmp);

    let mid = starts.len() / 2;
    Some(Segment {
        start: starts[mid],
        end: ends[mid],
    })
}

/// Enhanced segment detection that combines audio fingerprint matching with
/// audio profile similarity.
///
/// The algorithm weights audio fingerprint matches higher, but considers:
/// - Audio profile similarity (consistent volume patterns)
///
/// Note: Scene change positions are not currently used in this enhanced detector.
pub fn detect_segment_enhanced(
    current: &Fingerprint,
    current_profile: Option<&AudioProfile>,
    current_offset: f64,
    others: &[(Fingerprint, Option<&AudioProfile>, f64)],
    min_secs: f64,
    max_secs: f64,
    fingerprint_threshold: f64,
    profile_threshold: f64,
) -> Option<Segment> {
    use super::video_analysis::profile_similarity;

    let mut candidates: Vec<(Segment, f64)> = Vec::new();

    for (other_fp, other_profile, other_offset) in others {
        // Get fingerprint match
        if let Some((seg, _)) = find_common(
            current,
            other_fp,
            current_offset,
            *other_offset,
            min_secs,
            max_secs,
            fingerprint_threshold,
        ) {
            // Calculate confidence based on:
            // 1. Segment duration (longer = more confident)
            // 2. Audio profile similarity (if available)
            let duration: f64 = seg.end - seg.start;
            let max_secs_f64: f64 = max_secs;
            let duration_conf = if max_secs_f64 > 0.0 {
                duration.min(max_secs_f64) / max_secs_f64
            } else {
                0.5 // neutral if max_secs is zero
            };

            let profile_conf =
                if let (Some(cur_p), Some(other_p)) = (current_profile, other_profile) {
                    1.0 - profile_similarity(cur_p, other_p)
                } else {
                    0.5 // neutral if no profile
                };

            // Weighted confidence: 70% fingerprint, 30% profile
            let confidence = 0.7 * duration_conf + 0.3 * profile_conf;

            // Only include candidate if profile confidence meets threshold OR profile comparison unavailable
            if profile_conf >= profile_threshold
                || current_profile.is_none()
                || other_profile.is_none()
            {
                candidates.push((seg, confidence));
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // Sort by confidence and take best
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    Some(candidates[0].0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skipper::fingerprint::Fingerprint;

    /// Test that Segment duration is computed correctly.
    #[test]
    fn test_segment_duration() {
        let seg = Segment {
            start: 10.0,
            end: 30.0,
        };
        assert!((seg.duration() - 20.0).abs() < 0.001);
    }

    /// Test Segment with zero duration.
    #[test]
    fn test_segment_zero_duration() {
        let seg = Segment {
            start: 5.0,
            end: 5.0,
        };
        assert!((seg.duration() - 0.0).abs() < 0.001);
    }

    /// Test a perfect match between two identical fingerprints.
    #[test]
    fn test_find_common_identical() {
        let a = Fingerprint {
            values: vec![0, 0, 0, 0, 0, 0, 0, 0],
            scan_secs: 8.0,
        };
        let b = a.clone();

        let result = find_common(&a, &b, 0.0, 0.0, 1.0, 10.0, 0.85);
        assert!(result.is_some());

        let (seg_a, seg_b) = result.unwrap();
        assert!(seg_a.start < seg_a.end);
        assert!(seg_b.start < seg_b.end);
    }

    /// Test no match for completely different fingerprints.
    #[test]
    fn test_find_common_no_match() {
        let a = Fingerprint {
            values: vec![0u32; 10],
            scan_secs: 10.0,
        };
        let b = Fingerprint {
            values: vec![u32::MAX; 10],
            scan_secs: 10.0,
        };

        let result = find_common(&a, &b, 0.0, 0.0, 1.0, 10.0, 0.85);
        assert!(result.is_none());
    }

    /// Test that extremely short fingerprints are rejected.
    #[test]
    fn test_find_common_too_short() {
        let a = Fingerprint {
            values: vec![0, 1, 2],
            scan_secs: 3.0,
        };
        let b = a.clone();

        let result = find_common(&a, &b, 0.0, 0.0, 1.0, 10.0, 0.85);
        assert!(result.is_none());
    }

    /// Test detect_segment with single other episode.
    #[test]
    fn test_detect_segment_single_other() {
        let current = Fingerprint {
            values: vec![0u32; 10],
            scan_secs: 10.0,
        };
        let other = current.clone();

        let result = detect_segment(&current, 0.0, &[(other, 0.0)], 1.0, 10.0, 0.85);
        assert!(result.is_some());
    }

    /// Test detect_segment returns None when no matches.
    #[test]
    fn test_detect_segment_no_matches() {
        let current = Fingerprint {
            values: vec![0u32; 10],
            scan_secs: 10.0,
        };
        let other = Fingerprint {
            values: vec![u32::MAX; 10],
            scan_secs: 10.0,
        };

        let result = detect_segment(&current, 0.0, &[(other, 0.0)], 1.0, 10.0, 0.85);
        assert!(result.is_none());
    }
}
