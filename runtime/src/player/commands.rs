//! Typed player command API.
//!
//! `PlayerCommand` is the public surface for controlling playback.  The IPC
//! handler deserialises incoming requests into `PlayerCommand` variants and
//! passes them to `PlayerManager::handle_command()`, which dispatches to the
//! right `MpvPlayer` method.
//!
//! # Why a separate enum?
//!
//! Having a typed enum rather than raw mpv command strings:
//! - Gives the IPC layer compile-time safety
//! - Lets `PlayerManager` intercept commands (e.g. record subtitle delay)
//! - Makes the public API self-documenting
//!
//! # Adding a new command
//!
//! 1. Add a variant here.
//! 2. Handle it in `PlayerManager::handle_command`.
//! 3. Add the matching IPC request type in `ipc/v1/mod.rs`.
//! 4. Handle it in `tui/internal/ipc/ipc.go`.

use serde::{Deserialize, Serialize};

/// All commands the TUI (or any caller) can send to the player.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum PlayerCommand {
    // ── Transport ──────────────────────────────────────────────────────────

    /// Pause playback. No-op if already paused.
    Pause,

    /// Resume playback. No-op if already playing.
    Resume,

    /// Toggle between paused and playing.
    TogglePause,

    /// Seek relative to the current position (positive = forward, negative = back).
    Seek { seconds: f64 },

    /// Seek to an absolute position in seconds.
    SeekAbsolute { seconds: f64 },

    /// Stop playback and reset state. Does not advance the queue.
    Stop,

    // ── Volume ────────────────────────────────────────────────────────────

    /// Set volume (0–130). Values above 100 apply software amplification.
    SetVolume { level: f64 },

    /// Adjust volume by a delta (e.g. +5 or -5).
    AdjustVolume { delta: f64 },

    /// Toggle mute.
    ToggleMute,

    // ── Subtitle track ────────────────────────────────────────────────────

    /// Switch to a specific subtitle track by mpv track ID.
    SetSubtitleTrack { id: i64 },

    /// Disable subtitles.
    DisableSubtitles,

    /// Cycle to the next subtitle track (wraps around; disables after last).
    CycleSubtitles,

    /// Adjust subtitle display timing.
    /// Positive values delay subtitles; negative values advance them.
    AdjustSubtitleDelay { delta: f64 },

    /// Reset subtitle delay to 0.
    ResetSubtitleDelay,

    /// Load an external subtitle file.
    LoadSubtitle { path: String },

    // ── Audio track ───────────────────────────────────────────────────────

    /// Switch to a specific audio track by mpv track ID.
    SetAudioTrack { id: i64 },

    /// Cycle to the next audio track.
    CycleAudioTracks,

    /// Adjust audio synchronisation timing.
    /// Positive values delay audio; negative values advance it.
    AdjustAudioDelay { delta: f64 },

    /// Reset audio delay to 0.
    ResetAudioDelay,

    // ── Stream switching ──────────────────────────────────────────────────

    /// Replace the current stream with another URL (mpv `loadfile … replace`).
    /// Used for quality switching or manual fallback.
    SwitchStream { url: String },

    /// Switch to the next stream candidate in the ranked list.
    NextStreamCandidate,

    // ── Display ───────────────────────────────────────────────────────────

    /// Toggle fullscreen (only works if mpv has a window).
    ToggleFullscreen,

    /// Take a screenshot (saved to mpv's screenshot directory).
    Screenshot,
}

impl PlayerCommand {
    /// Short human-readable name for logging.
    pub fn name(&self) -> &'static str {
        match self {
            PlayerCommand::Pause              => "pause",
            PlayerCommand::Resume             => "resume",
            PlayerCommand::TogglePause        => "toggle_pause",
            PlayerCommand::Seek { .. }        => "seek",
            PlayerCommand::SeekAbsolute { .. }=> "seek_absolute",
            PlayerCommand::Stop               => "stop",
            PlayerCommand::SetVolume { .. }   => "set_volume",
            PlayerCommand::AdjustVolume { .. }=> "adjust_volume",
            PlayerCommand::ToggleMute         => "toggle_mute",
            PlayerCommand::SetSubtitleTrack { .. }   => "set_sub_track",
            PlayerCommand::DisableSubtitles          => "disable_subs",
            PlayerCommand::CycleSubtitles            => "cycle_subs",
            PlayerCommand::AdjustSubtitleDelay { .. }=> "adjust_sub_delay",
            PlayerCommand::ResetSubtitleDelay        => "reset_sub_delay",
            PlayerCommand::LoadSubtitle { .. }       => "load_subtitle",
            PlayerCommand::SetAudioTrack { .. }      => "set_audio_track",
            PlayerCommand::CycleAudioTracks          => "cycle_audio",
            PlayerCommand::AdjustAudioDelay { .. }   => "adjust_audio_delay",
            PlayerCommand::ResetAudioDelay           => "reset_audio_delay",
            PlayerCommand::SwitchStream { .. }       => "switch_stream",
            PlayerCommand::NextStreamCandidate       => "next_candidate",
            PlayerCommand::ToggleFullscreen          => "toggle_fullscreen",
            PlayerCommand::Screenshot                => "screenshot",
        }
    }
}
