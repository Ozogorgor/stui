//! Adaptive stream buffering — pre-roll, stall detection, speed estimation.
//!
//! # Future merge note
//!
//! See `resolver.rs` for the rationale on why resolver, streamer, and
//! scraper may eventually merge into a `sources/` module.

// streamer.rs — Smart adaptive buffer algorithm for torrent-backed streaming.
//
// ─────────────────────────────────────────────────────────────────────────────
// DESIGN
// ─────────────────────────────────────────────────────────────────────────────
//
// The fundamental problem: a torrent download progresses at `download_speed`
// bytes/s while mpv consumes the file at `video_bitrate` bytes/s.  If the
// download is slower than playback the player will stall.  We need to hold
// playback until enough of a cushion exists that stalls are unlikely even if
// the download speed temporarily drops.
//
// Three phases:
//
//   Phase 1 — Speed baseline (first ~10 s of download)
//     Sample aria2's `download_speed` every 500 ms.  Compute an
//     exponentially-weighted moving average (EWMA, α = 0.3) to smooth
//     momentary fluctuations while reacting to genuine speed changes.
//     We also record a conservative floor = min(all samples in the window),
//     because it is the worst-case speed that determines stall risk.
//
//   Phase 2 — Pre-roll calculation
//     Once we know file size + duration (or can estimate bitrate):
//
//       video_bitrate_bps  = total_bytes / duration_secs
//       slack              = ewma_speed / video_bitrate
//
//       slack ≥ 2.0  →  start at  3 s pre-roll  (trivially fast connection)
//       slack ≥ 1.2  →  start at 30 s pre-roll  (comfortable headroom)
//       slack ≥ 0.8  →  start at 90 s pre-roll  (marginal, keep buffer wide)
//       slack <  0.8  →  start at min(25% of file, 300 s of video)
//
//     "X seconds of pre-roll" means we wait until `completed_bytes ≥
//     X * video_bitrate_bps` before handing the file to mpv.
//
//   Phase 3 — Stall-guard loop (runs concurrently with playback)
//     Every 2 s:
//       remaining_download_secs = remaining_bytes / ewma_speed
//       remaining_playback_secs = duration - mpv_position
//
//       if remaining_download_secs > remaining_playback_secs * STALL_THRESHOLD:
//           pause mpv
//           accumulate RECOVERY_BUFFER_SECS of video (30 s default)
//           resume mpv
//           push player_buffering / player_resuming events to Go
//
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::player::mpv::MpvPlayer;

// ── Tuning constants ──────────────────────────────────────────────────────────

/// EWMA smoothing factor: 0 < α ≤ 1.  Higher = more reactive to changes.
const EWMA_ALPHA: f64 = 0.3;

/// How many speed samples to collect during baseline (sampled every 500 ms).
const BASELINE_SAMPLES: usize = 20; // 10 seconds

/// Safety margin: if download ETA exceeds playback ETA by this factor, stall guard triggers.
const STALL_THRESHOLD: f64 = 1.05;

/// How many seconds of extra buffer to accumulate during a stall-guard re-pause.
const RECOVERY_BUFFER_SECS: f64 = 30.0;

/// Minimum absolute pre-roll before any playback starts (bytes).
const MIN_PREROLL_BYTES: u64 = 4 * 1024 * 1024; // 4 MB — always safe

/// Cap on pre-roll as a fraction of total file size.
const MAX_PREROLL_FRACTION: f64 = 0.25; // never buffer more than 25%

// ── Wire event shapes ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct BufferingWire {
    r#type:         &'static str, // "player_buffering"
    reason:         &'static str, // "initial" | "stall_guard"
    pre_roll_secs:  f64,
    fill_percent:   f64,
    speed_mbps:     f64,
    eta_secs:       f64,
}

#[derive(Serialize)]
struct BufferReadyWire {
    r#type:        &'static str, // "player_buffer_ready"
    pre_roll_secs: f64,
    speed_mbps:    f64,
    slack:         f64,           // download_speed / video_bitrate
}

// ── SpeedEstimator ────────────────────────────────────────────────────────────

/// Maintains an EWMA of observed download speeds plus a sliding-window floor.
#[derive(Default)]
pub struct SpeedEstimator {
    ewma:        f64,
    floor:       f64,
    samples:     Vec<f64>,
    n:           usize,
    window:      usize, // number of samples to keep for floor calculation
}

impl SpeedEstimator {
    pub fn new(window: usize) -> Self {
        SpeedEstimator {
            window,
            floor: f64::MAX,
            ..Default::default()
        }
    }

    /// Feed a new speed sample (bytes/s).
    pub fn observe(&mut self, bps: f64) {
        if self.n == 0 {
            self.ewma = bps;
        } else {
            self.ewma = EWMA_ALPHA * bps + (1.0 - EWMA_ALPHA) * self.ewma;
        }
        self.n += 1;

        // Rolling window for floor
        self.samples.push(bps);
        if self.samples.len() > self.window {
            self.samples.remove(0);
        }
        self.floor = self.samples.iter().cloned().fold(f64::MAX, f64::min);
    }

    /// EWMA speed estimate (bytes/s).
    pub fn ewma(&self) -> f64 { self.ewma }

    /// Conservative floor from the rolling window (bytes/s).
    pub fn floor(&self) -> f64 {
        if self.floor == f64::MAX { self.ewma } else { self.floor }
    }

    /// Number of samples seen so far.
    pub fn count(&self) -> usize { self.n }

    /// Conservatively blend ewma and floor: 70% ewma, 30% floor.
    /// This is the speed we use for buffering decisions — optimistic enough
    /// to not over-buffer on fast connections, pessimistic enough to protect
    /// against speed dips.
    pub fn effective(&self) -> f64 {
        0.7 * self.ewma() + 0.3 * self.floor()
    }
}

// ── Pre-roll calculation ──────────────────────────────────────────────────────

/// Everything the pre-roll formula needs.
#[derive(Debug, Clone)]
pub struct StreamParams {
    /// Total file size in bytes (from aria2).
    pub total_bytes:   u64,
    /// Video duration in seconds (from mpv media-title or aria2 — may be 0 if unknown).
    pub duration_secs: f64,
    /// Conservative effective download speed (bytes/s).
    pub speed_bps:     f64,
}

#[derive(Debug, Clone)]
pub struct PreRollPlan {
    /// Bytes that must be downloaded before playback starts.
    pub preroll_bytes: u64,
    /// Corresponding seconds of video content.
    pub preroll_secs:  f64,
    /// Estimated video bitrate (bytes/s). 0 if unknown.
    pub video_bitrate: f64,
    /// slack = speed / bitrate.  ≥ 1.0 means download is faster than playback.
    pub slack:         f64,
}

impl PreRollPlan {
    pub fn compute(p: &StreamParams) -> Self {
        let bitrate = if p.total_bytes > 0 && p.duration_secs > 0.0 {
            p.total_bytes as f64 / p.duration_secs
        } else {
            // Unknown — assume 720p H.264 average (~2 Mbps = 250 KB/s)
            250_000.0
        };

        let slack = if bitrate > 0.0 { p.speed_bps / bitrate } else { 2.0 };

        // Pre-roll target in seconds of video content
        let target_secs: f64 = if slack >= 2.0 {
            3.0
        } else if slack >= 1.2 {
            30.0
        } else if slack >= 0.8 {
            90.0
        } else {
            // Download slower than playback — compute how much buffer gives
            // at least 300 s of continuous viewing before a potential stall.
            // buffer_bytes = (bitrate - speed) * 300
            let gap = (bitrate - p.speed_bps).max(0.0);
            let raw_secs = if bitrate > 0.0 { gap * 300.0 / bitrate } else { 300.0 };
            raw_secs.min(p.duration_secs * MAX_PREROLL_FRACTION)
        };

        let mut preroll_bytes = (target_secs * bitrate) as u64;

        // Always enforce the hard floor
        preroll_bytes = preroll_bytes.max(MIN_PREROLL_BYTES);

        // Never buffer more than 25% of the file
        if p.total_bytes > 0 {
            let cap = (p.total_bytes as f64 * MAX_PREROLL_FRACTION) as u64;
            preroll_bytes = preroll_bytes.min(cap).max(MIN_PREROLL_BYTES);
        }

        let preroll_secs = if bitrate > 0.0 {
            preroll_bytes as f64 / bitrate
        } else {
            target_secs
        };

        info!(
            "streamer: pre-roll plan — slack={:.2} bitrate={:.0} KB/s speed={:.0} KB/s \
             target={:.0}s preroll={:.1} MB",
            slack,
            bitrate / 1024.0,
            p.speed_bps / 1024.0,
            preroll_secs,
            preroll_bytes as f64 / 1024.0 / 1024.0,
        );

        PreRollPlan { preroll_bytes, preroll_secs, video_bitrate: bitrate, slack }
    }
}

// ── StreamerState ─────────────────────────────────────────────────────────────

/// Tracks live state shared between the pre-roll waiter and the stall-guard loop.
#[allow(dead_code)]
pub struct StreamerState {
    pub plan:             Option<PreRollPlan>,
    pub speed:            SpeedEstimator,
    /// mpv playback position (seconds), updated by the player_bridge.
    pub mpv_position:     f64,
    /// mpv duration (seconds), updated once known.
    pub mpv_duration:     f64,
    /// True once mpv has been launched.
    pub playing:          bool,
    /// True while the stall guard has paused mpv.
    pub guard_paused:     bool,
}

impl StreamerState {
    #[allow(dead_code)]
    pub fn new() -> Self {
        StreamerState {
            plan:         None,
            speed:        SpeedEstimator::new(BASELINE_SAMPLES),
            mpv_position: 0.0,
            mpv_duration: 0.0,
            playing:      false,
            guard_paused: false,
        }
    }
}

impl Default for StreamerState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Streamer ──────────────────────────────────────────────────────────────────

/// Main handle — created by player_bridge, drives the whole adaptive flow.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct Streamer {
    state:   Arc<Mutex<StreamerState>>,
    ipc_tx:  mpsc::Sender<String>,
}

impl Streamer {
    pub fn new(ipc_tx: mpsc::Sender<String>) -> Self {
        Streamer {
            state:  Arc::new(Mutex::new(StreamerState::new())),
            ipc_tx,
        }
    }

    /// Update mpv position + duration (called from the mpv event forwarder).
    pub async fn on_mpv_progress(&self, position: f64, duration: f64) {
        let mut s = self.state.lock().await;
        s.mpv_position = position;
        if duration > 0.0 { s.mpv_duration = duration; }
    }

    // ── Pre-roll phase ────────────────────────────────────────────────────────

    /// Wait until the torrent has buffered enough to start playback safely.
    /// Returns `(file_path, plan)` when ready, or `None` on failure/timeout.
    ///
    /// `duration_hint` is optionally set from a previous mpv session or
    /// a metadata provider; 0 means unknown.
    pub async fn wait_for_preroll(
        &self,
        aria2:         &stui_aria2::Aria2Client,
        gid:           &str,
        duration_hint: f64,
    ) -> Option<(String, PreRollPlan)> {
        use tokio::time::interval;

        let mut tick = interval(Duration::from_millis(500));
        let mut plan: Option<PreRollPlan> = None;
        let start = Instant::now();
        const TIMEOUT: Duration = Duration::from_secs(600); // 10 min max

        loop {
            if start.elapsed() > TIMEOUT {
                warn!("streamer: pre-roll timed out after 10 min");
                return None;
            }

            tick.tick().await;

            let status = match aria2.tell_status(gid).await {
                Ok(s)  => s,
                Err(e) => { warn!("streamer: aria2 poll: {e}"); continue; }
            };

            if status.is_error() {
                warn!("streamer: aria2 reported error for gid={gid}");
                return None;
            }

            // Feed speed sample
            {
                let speed_bps = status.speed_bps() as f64;
                let mut s = self.state.lock().await;
                s.speed.observe(speed_bps);
            }

            // Find the video file
            let video_file = status.files.iter()
                .find(|f| is_video_file(&f.path))
                .cloned();

            let Some(vf) = video_file else {
                // Torrent metadata not resolved yet — keep waiting
                debug!("streamer: no video file yet in gid={gid}");
                self.push_buffering("initial", 0, 0.0, 0.0, 0.0, 0.0).await;
                continue;
            };

            let total_bytes:     u64 = vf.length.parse().unwrap_or(0);
            let completed_bytes: u64 = vf.completed_length.parse().unwrap_or(0);

            // Build or re-evaluate the plan once we have baseline speed
            let (effective_speed, n) = {
                let s = self.state.lock().await;
                (s.speed.effective(), s.speed.count())
            };

            if n >= BASELINE_SAMPLES || (n >= 4 && total_bytes > 0) {
                // We have enough speed samples — compute or refresh plan
                let duration = if duration_hint > 0.0 {
                    duration_hint
                } else {
                    // Estimate from file size assuming 720p H.264 average
                    if total_bytes > 0 { total_bytes as f64 / 250_000.0 } else { 0.0 }
                };

                let p = PreRollPlan::compute(&StreamParams {
                    total_bytes,
                    duration_secs: duration,
                    speed_bps:     effective_speed,
                });
                plan = Some(p);
            }

            let Some(ref p) = plan else {
                // Still building baseline — show progress
                let fill = if total_bytes > 0 {
                    completed_bytes as f64 / total_bytes as f64 * 100.0
                } else { 0.0 };
                self.push_buffering(
                    "initial", completed_bytes, fill,
                    effective_speed / 1024.0 / 1024.0,
                    0.0,
                    (BASELINE_SAMPLES - n) as f64 * 0.5,
                ).await;
                continue;
            };

            // Check completion condition
            let fill_pct = if p.preroll_bytes > 0 {
                (completed_bytes as f64 / p.preroll_bytes as f64 * 100.0).min(100.0)
            } else { 100.0 };

            let remaining_preroll = p.preroll_bytes.saturating_sub(completed_bytes);
            let eta_secs = if effective_speed > 0.0 {
                remaining_preroll as f64 / effective_speed
            } else { 9999.0 };

            self.push_buffering(
                "initial",
                completed_bytes,
                fill_pct,
                effective_speed / 1024.0 / 1024.0,
                p.preroll_secs,
                eta_secs,
            ).await;

            if completed_bytes >= p.preroll_bytes {
                // ✓ Pre-roll satisfied — push buffer_ready and return
                let _ = self.ipc_tx.send(
                    serde_json::to_string(&BufferReadyWire {
                        r#type:        "player_buffer_ready",
                        pre_roll_secs: p.preroll_secs,
                        speed_mbps:    effective_speed / 1024.0 / 1024.0,
                        slack:         p.slack,
                    }).unwrap_or_default()
                ).await;

                info!(
                    "streamer: pre-roll ready — {:.1}MB downloaded, {:.0}s buffered, slack={:.2}",
                    completed_bytes as f64 / 1024.0 / 1024.0,
                    p.preroll_secs,
                    p.slack,
                );
                return Some((vf.path.clone(), p.clone()));
            }
        }
    }

    // ── Stall-guard loop ──────────────────────────────────────────────────────

    /// Runs concurrently with mpv playback.
    /// Pauses mpv if the download is falling behind playback consumption.
    pub async fn run_stall_guard(
        &self,
        aria2:  &stui_aria2::Aria2Client,
        gid:    &str,
        mpv:    &MpvPlayer,
        plan:   PreRollPlan,
    ) {
        use tokio::time::interval;

        let mut tick    = interval(Duration::from_secs(2));
        let mut paused  = false;
        let mut pause_start: Option<Instant> = None;

        info!("streamer: stall-guard running for gid={gid}");

        loop {
            tick.tick().await;

            // ── Bail if mpv has ended ─────────────────────────────────────
            let (mpv_pos, mpv_dur) = {
                let s = self.state.lock().await;
                (s.mpv_position, s.mpv_duration)
            };
            if mpv_dur > 0.0 && mpv_pos >= mpv_dur - 2.0 {
                debug!("streamer: stall-guard: playback finished, exiting");
                break;
            }

            // ── Poll aria2 ────────────────────────────────────────────────
            let status = match aria2.tell_status(gid).await {
                Ok(s)  => s,
                Err(_) => continue,
            };

            // Update speed estimator
            let speed_bps = {
                let mut s = self.state.lock().await;
                s.speed.observe(status.speed_bps() as f64);
                s.speed.effective()
            };

            // Download is complete — stall guard no longer needed
            if status.status == "complete" {
                debug!("streamer: stall-guard: download complete, exiting");
                if paused {
                    let _ = mpv.send_command(&serde_json::json!(["set_property","pause",false])).await;
                }
                break;
            }

            let completed: u64 = status.files.iter()
                .find(|f| is_video_file(&f.path))
                .and_then(|f| f.completed_length.parse().ok())
                .unwrap_or(0);

            let total: u64 = status.files.iter()
                .find(|f| is_video_file(&f.path))
                .and_then(|f| f.length.parse().ok())
                .unwrap_or(0);

            let remaining_bytes = total.saturating_sub(completed);
            let remaining_download_secs = if speed_bps > 0.0 {
                remaining_bytes as f64 / speed_bps
            } else { f64::MAX };

            let remaining_playback_secs = if mpv_dur > 0.0 {
                (mpv_dur - mpv_pos).max(0.0)
            } else { f64::MAX };

            debug!(
                "streamer: guard — dl_eta={:.0}s play_rem={:.0}s speed={:.2}Mbps paused={}",
                remaining_download_secs,
                remaining_playback_secs,
                speed_bps / 1024.0 / 1024.0,
                paused,
            );

            if !paused {
                // ── Stall risk? ───────────────────────────────────────────
                if remaining_download_secs > remaining_playback_secs * STALL_THRESHOLD {
                    warn!(
                        "streamer: stall guard triggered — dl_eta={:.0}s > play_rem={:.0}s",
                        remaining_download_secs,
                        remaining_playback_secs,
                    );
                    // Pause mpv
                    let _ = mpv.send_command(
                        &serde_json::json!(["set_property","pause",true])
                    ).await;

                    // Push buffering event to Go
                    self.push_buffering(
                        "stall_guard",
                        completed,
                        0.0,
                        speed_bps / 1024.0 / 1024.0,
                        RECOVERY_BUFFER_SECS,
                        RECOVERY_BUFFER_SECS * plan.video_bitrate / speed_bps.max(1.0),
                    ).await;

                    paused = true;
                    pause_start = Some(Instant::now());
                }
            } else {
                // ── We're paused — check if we can resume ─────────────────
                let buffer_seconds_ahead = if plan.video_bitrate > 0.0 {
                    // bytes ahead of the playback head
                    let playback_head_bytes = (mpv_pos * plan.video_bitrate) as u64;
                    let ahead = completed.saturating_sub(playback_head_bytes);
                    ahead as f64 / plan.video_bitrate
                } else {
                    0.0
                };

                // Resume when we have RECOVERY_BUFFER_SECS buffered ahead,
                // OR if download speed is now comfortably above bitrate
                let speed_ok = speed_bps >= plan.video_bitrate * 1.2;
                let buffer_ok = buffer_seconds_ahead >= RECOVERY_BUFFER_SECS;

                // Safety valve: don't keep user paused for more than 3 minutes
                let timeout = pause_start
                    .map(|t| t.elapsed() > Duration::from_secs(180))
                    .unwrap_or(false);

                if buffer_ok || speed_ok || timeout {
                    info!(
                        "streamer: resuming — buffer_ahead={:.0}s speed_ok={} timeout={}",
                        buffer_seconds_ahead, speed_ok, timeout,
                    );
                    let _ = mpv.send_command(
                        &serde_json::json!(["set_property","pause",false])
                    ).await;

                    // Push buffer_ready to Go
                    let _ = self.ipc_tx.send(
                        serde_json::to_string(&BufferReadyWire {
                            r#type:        "player_buffer_ready",
                            pre_roll_secs: buffer_seconds_ahead,
                            speed_mbps:    speed_bps / 1024.0 / 1024.0,
                            slack:         speed_bps / plan.video_bitrate.max(1.0),
                        }).unwrap_or_default()
                    ).await;

                    paused = false;
                    pause_start = None;
                    {
                        let mut s = self.state.lock().await;
                        s.guard_paused = false;
                    }
                } else {
                    // Still waiting — push updated progress
                    let fill = (buffer_seconds_ahead / RECOVERY_BUFFER_SECS * 100.0).min(100.0);
                    let eta = if speed_bps > 0.0 {
                        let needed = (RECOVERY_BUFFER_SECS - buffer_seconds_ahead)
                            .max(0.0) * plan.video_bitrate;
                        needed / speed_bps
                    } else { 999.0 };

                    self.push_buffering(
                        "stall_guard",
                        completed,
                        fill,
                        speed_bps / 1024.0 / 1024.0,
                        RECOVERY_BUFFER_SECS - buffer_seconds_ahead,
                        eta,
                    ).await;
                }
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn push_buffering(
        &self,
        reason:        &'static str,
        _completed:    u64,
        fill_percent:  f64,
        speed_mbps:    f64,
        pre_roll_secs: f64,
        eta_secs:      f64,
    ) {
        let msg = serde_json::to_string(&BufferingWire {
            r#type:        "player_buffering",
            reason,
            pre_roll_secs,
            fill_percent,
            speed_mbps,
            eta_secs,
        }).unwrap_or_default();
        let _ = self.ipc_tx.send(msg).await;
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

pub fn is_video_file(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".mkv") || p.ends_with(".mp4") || p.ends_with(".avi") ||
    p.ends_with(".mov") || p.ends_with(".m4v") || p.ends_with(".webm") ||
    p.ends_with(".ts")  || p.ends_with(".m2ts")
}
