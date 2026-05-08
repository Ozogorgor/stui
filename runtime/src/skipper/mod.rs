//! Intro / credits skip detection.
//!
//! Uses FFmpeg's Chromaprint demuxer to fingerprint audio from stream URLs,
//! then cross-compares fingerprints across episodes of the same series to find
//! recurring segments (intros, ending credits, recaps).
//!
//! # How it works
//!
//! 1. When a play request fires, a background task is spawned via `Skipper::analyze()`.
//! 2. The first `intro_scan_secs` of audio are fingerprinted for intro detection.
//!    The last `credits_scan_secs` are fingerprinted for credits detection.
//! 3. Fingerprints are cached on disk under `~/.stui/cache/skipper/`.
//! 4. Once ≥ `min_episodes` fingerprints exist for a series, a DP longest-common-
//!    substring comparison finds the recurring segment.
//! 5. Detected segments are pushed as `{"type":"skip_segment", ...}` NDJSON to the TUI.
//! 6. The TUI shows a "Skip Intro / Skip Credits" overlay and optionally auto-skips.
//!
//! # FFmpeg requirement
//!
//! FFmpeg must be compiled with `--enable-chromaprint`. If it isn't, fingerprint
//! extraction silently returns `None` and no skip buttons appear.

pub mod analyzer;
pub mod detector;
pub mod fingerprint;
pub mod store;
pub mod text_analysis;
pub mod video_analysis;

#[allow(unused_imports)]
pub use analyzer::Segment;
pub use detector::Skipper;
#[allow(dead_code, unused_imports)]
pub use fingerprint::Fingerprint;
#[allow(dead_code, unused_imports)]
pub use store::SkipperStore;

// Functions available for enhanced detection (wired but not default)
#[allow(dead_code, unused_imports)]
pub use analyzer::detect_segment_enhanced;
