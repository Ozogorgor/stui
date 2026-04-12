//! PlayerManager — high-level playback queue and session manager.
//!
//! Sits above `PlayerBridge` and handles:
//!
//! - **Playlist / queue**: ordered list of items to play next
//! - **Auto next-episode**: when an episode ends, automatically queue S01E02
//! - **Resume position**: track where the user left off (via mpv watch-later)
//! - **Playback history**: record what was watched and when
//! - **Active stream info**: expose current item + position to the rest of the app
//!
//! # Lifecycle
//!
//! ```text
//! manager.play_item(item)        -> starts playback, sets current
//! manager.enqueue(item)          -> adds to tail of queue
//! manager.skip()                 -> stops current, plays next in queue
//! manager.on_playback_ended(why) -> called by bridge; auto-advances if queue non-empty
//! ```

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::media::MediaItem;
use crate::media::stream::StreamCandidate;
use crate::events::{EventBus, RuntimeEvent};
use super::bridge::PlayerBridge;
use super::commands::PlayerCommand;
use super::state::PlaybackState;

// ── Queue entry ───────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub item:     MediaItem,
    pub provider: String,
    /// Subtitle file path if already resolved.
    pub sub_path: Option<String>,
    /// Resume position in seconds (0 = start from beginning).
    pub resume_at: f64,
}

// ── Playback record ───────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PlaybackRecord {
    pub item:       MediaItem,
    pub started_at: Instant,
    pub stopped_at: Option<Instant>,
    /// Last known position when playback ended.
    pub position:   f64,
    pub completed:  bool,
}

// ── PlayerManager ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct PlayerManager {
    bridge:  PlayerBridge,
    inner:   Arc<Mutex<ManagerState>>,
    ipc_tx:  mpsc::Sender<String>,
    bus:     Arc<EventBus>,
}

#[allow(dead_code)] // planned: PlayerManager pub API, fields read when manager is wired in
struct ManagerState {
    queue:      VecDeque<QueueEntry>,
    current:    Option<QueueEntry>,
    history:    Vec<PlaybackRecord>,
    /// Ranked stream candidates for the currently playing item.
    candidates: Vec<StreamCandidate>,
    /// Index into `candidates` of the active stream.
    active_idx: usize,
    /// Current playback state snapshot (updated from MpvEvent::Progress).
    state:      PlaybackState,
}

#[allow(dead_code)] // planned: PlayerManager pub API, wired in by TUI/IPC layer
impl PlayerManager {
    #[allow(dead_code)]
    pub fn new(bridge: PlayerBridge, ipc_tx: mpsc::Sender<String>, bus: Arc<EventBus>) -> Self {
        PlayerManager {
            bridge,
            ipc_tx,
            bus,
            inner: Arc::new(Mutex::new(ManagerState {
                queue:      VecDeque::new(),
                current:    None,
                history:    Vec::new(),
                candidates: vec![],
                active_idx: 0,
                state:      PlaybackState::default(),
            })),
        }
    }

    /// Set the ranked stream candidates for the current item.
    /// Called by the pipeline after resolving streams.
    #[allow(dead_code)]
    pub async fn set_candidates(&self, candidates: Vec<StreamCandidate>) {
        let mut s = self.inner.lock().await;
        s.active_idx = 0;
        s.candidates = candidates;
        let count = s.candidates.len();
        s.state.candidate_count = count;
        s.state.active_candidate = 0;
    }

    /// Get a snapshot of the current playback state.
    #[allow(dead_code)]
    pub async fn playback_state(&self) -> PlaybackState {
        self.inner.lock().await.state.clone()
    }

    // ── Command dispatch ──────────────────────────────────────────────────

    /// Execute a typed `PlayerCommand`.  All TUI/IPC player commands flow here.
    pub async fn handle_command(&self, cmd: PlayerCommand) {
        use PlayerCommand::*;
        let cmd_name = cmd.name().to_string();
        tracing::debug!("player command: {}", cmd_name);

        let result: Result<(), String> = match cmd {
            Pause              => self.bridge.mpv().set_pause(true).await,
            Resume             => self.bridge.mpv().set_pause(false).await,
            TogglePause        => self.bridge.mpv().toggle_pause().await,
            Stop               => { self.stop().await; Ok(()) }
            Seek { seconds }   => self.bridge.mpv().seek_relative(seconds).await,
            SeekAbsolute { seconds } => self.bridge.mpv().seek_absolute(seconds).await,

            SetVolume { level }    => self.bridge.mpv().set_volume(level).await,
            AdjustVolume { delta } => self.bridge.mpv().adjust_volume(delta).await,
            ToggleMute             => self.bridge.mpv().toggle_mute().await,

            SetSubtitleTrack { id }       => self.bridge.mpv().set_subtitle_track(id).await,
            DisableSubtitles              => self.bridge.mpv().disable_subtitles().await,
            CycleSubtitles                => self.bridge.mpv().cycle_subtitles().await,
            AdjustSubtitleDelay { delta } => {
                self.inner.lock().await.state.subtitle_delay += delta;
                self.bridge.mpv().adjust_sub_delay(delta).await
            }
            ResetSubtitleDelay => {
                self.inner.lock().await.state.subtitle_delay = 0.0;
                self.bridge.mpv().reset_sub_delay().await
            }
            LoadSubtitle { path } => self.bridge.mpv().load_subtitle(&path).await,

            SetAudioTrack { id }        => self.bridge.mpv().set_audio_track(id).await,
            CycleAudioTracks            => self.bridge.mpv().cycle_audio_tracks().await,
            AdjustAudioDelay { delta }  => {
                self.inner.lock().await.state.audio_delay += delta;
                self.bridge.mpv().adjust_audio_delay(delta).await
            }
            ResetAudioDelay => {
                self.inner.lock().await.state.audio_delay = 0.0;
                self.bridge.mpv().reset_audio_delay().await
            }

            SwitchStream { url } => {
                info!("manager: switching stream to {}", &url[..url.len().min(80)]);
                self.bus.emit(RuntimeEvent::StreamSwitchRequested { entry_id: url.clone() });
                self.bridge.mpv().loadfile_replace(&url).await
            }
            NextStreamCandidate => {
                self.try_next_candidate().await;
                Ok(())
            }

            ToggleFullscreen => self.bridge.mpv().toggle_fullscreen().await,
            Screenshot       => self.bridge.mpv().screenshot().await,
        };

        if let Err(e) = result {
            warn!("player command {} failed: {e}", cmd_name);
        }
    }

    /// Switch to the next ranked stream candidate (automatic fallback or user request).
    pub async fn try_next_candidate(&self) {
        let (url, new_idx, total) = {
            let mut s = self.inner.lock().await;
            let next = s.active_idx + 1;
            if next >= s.candidates.len() {
                warn!("manager: no more stream candidates — giving up");
                return;
            }
            s.active_idx = next;
            s.state.active_candidate = next;
            let url = s.candidates[next].url.clone();
            (url, next, s.candidates.len())
        };
        info!("manager: switching to candidate {}/{}: {}", new_idx + 1, total, &url[..url.len().min(80)]);
        let _ = self.bridge.mpv().loadfile_replace(&url).await;
    }

    // ── Playback control ──────────────────────────────────────────────────

    /// Start playing `entry` immediately, replacing whatever is currently playing.
    pub async fn play_item(&self, entry: QueueEntry) {
        info!("manager: play {:?}", entry.item.title);
        {
            let mut s = self.inner.lock().await;
            s.current = Some(entry.clone());
        }
        self.bridge.play(
            &entry.item.id.to_string_id(),
            &entry.provider,
            entry.item.imdb_id.as_deref().unwrap_or(""),
            None,
            Some(entry.item.media_type.clone()),
            entry.item.year,
        ).await;
    }

    /// Add an item to the end of the playback queue.
    pub async fn enqueue(&self, entry: QueueEntry) {
        info!("manager: enqueue {:?}", entry.item.title);
        self.inner.lock().await.queue.push_back(entry);
        self.push_queue_event().await;
    }

    /// Add an item to play immediately after the current one.
    pub async fn play_next(&self, entry: QueueEntry) {
        self.inner.lock().await.queue.push_front(entry);
        self.push_queue_event().await;
    }

    /// Stop current playback and clear the queue.
    pub async fn stop(&self) {
        self.bridge.stop().await;
        let mut s = self.inner.lock().await;
        s.queue.clear();
        s.current = None;
    }

    /// Skip current item and play the next in queue (if any).
    pub async fn skip(&self) {
        self.bridge.stop().await;
        self.advance_queue().await;
    }

    /// Called by the IPC loop when `player_ended` is received.
    /// Automatically advances the queue.
    pub async fn on_playback_ended(&self, reason: &str, position: f64) {
        info!("manager: playback ended reason={reason} pos={position}");

        let completed = reason == "eof";

        {
            let mut s = self.inner.lock().await;
            if let Some(entry) = s.current.take() {
                s.history.push(PlaybackRecord {
                    item:       entry.item,
                    started_at: Instant::now(), // approximate
                    stopped_at: Some(Instant::now()),
                    position,
                    completed,
                });
            }
        }

        if completed {
            self.advance_queue().await;
        }
    }

    // ── Queue introspection ───────────────────────────────────────────────

    pub async fn queue_len(&self) -> usize {
        self.inner.lock().await.queue.len()
    }

    pub async fn current_item(&self) -> Option<MediaItem> {
        self.inner.lock().await.current.as_ref().map(|e| e.item.clone())
    }

    pub async fn history(&self) -> Vec<PlaybackRecord> {
        self.inner.lock().await.history.clone()
    }

    // ── Internal ──────────────────────────────────────────────────────────

    async fn advance_queue(&self) {
        let next = self.inner.lock().await.queue.pop_front();
        match next {
            Some(entry) => {
                info!("manager: auto-advancing to {:?}", entry.item.title);
                self.play_item(entry).await;
            }
            None => {
                info!("manager: queue empty — stopping");
            }
        }
    }

    async fn push_queue_event(&self) {
        let len = self.queue_len().await;
        let msg = serde_json::to_string(&serde_json::json!({
            "type":      "queue_update",
            "queue_len": len,
        })).unwrap_or_default();
        let _ = self.ipc_tx.send(msg).await;
    }
}
