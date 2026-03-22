//! Cross-episode fingerprint comparison to find recurring segments.

use super::fingerprint::Fingerprint;

/// A detected temporal segment within an episode.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
}

impl Segment {
    #[allow(dead_code)]
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
    threshold: f64, // 0..1, higher = stricter (e.g. 0.85)
) -> Option<(Segment, Segment)> {
    // Per-frame "match" criterion: fraction of differing bits must be ≤ (1 - threshold)
    let max_dist = ((1.0 - threshold) * 32.0) as u32;

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
