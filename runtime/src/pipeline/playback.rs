//! Playback pipeline — translate IPC player commands to engine calls.
//!
//! Also owns the top-level `run_play` entry point that fires off the player
//! and the skip-detection analyser as independent background tasks.

use std::sync::Arc;

use serde_json::json;
use tracing::warn;

use crate::engine::Engine;
use crate::ipc::{MediaTab, MediaType, MpdOutputInfo, MpdOutputsResponse, PlayerCmd, PlayerCommandRequest, Response, ErrorCode};
use crate::mpd_bridge::MpdBridge;
use crate::player::PlayerBridge;
use crate::skipper::Skipper;

// ── Play ──────────────────────────────────────────────────────────────────────

/// Spawn player + skip-detection tasks for a `play` IPC request.
///
/// Both tasks run in the background — this returns immediately.
/// `tab` is `Some(Music|Radio|Podcasts)` for audio (→ MPD),
/// `None` for video (→ mpv).
pub fn run_play(
    player:     PlayerBridge,
    skipper:    Arc<Skipper>,
    engine:     Arc<Engine>,
    entry_id:   String,
    provider:   String,
    imdb_id:    String,
    tab:        Option<MediaTab>,
    media_type: Option<MediaType>,
    year:       Option<u32>,
) {
    // Fire-and-forget: launch playback
    let p = player.clone();
    let eid = entry_id.clone();
    let prov = provider.clone();
    let iid = imdb_id.clone();
    tokio::spawn(async move {
        p.play(&eid, &prov, &iid, tab, media_type, year).await;
    });

    // Fire-and-forget: fingerprint + skip detection (video only — needs HTTP URL)
    let sk_entry  = entry_id;
    let sk_imdb   = imdb_id;
    let sk_prov   = provider;
    tokio::spawn(async move {
        match engine.resolve_raw(&sk_entry, &sk_prov).await {
            Ok(r)  => skipper.analyze(&r.stream_url, &sk_entry, &sk_imdb).await,
            Err(e) => warn!(error=%e, "skipper: could not resolve URL for fingerprinting"),
        }
    });
}

// ── MPD outputs ───────────────────────────────────────────────────────────────

/// Fetch MPD output list and return an IPC `Response`.
pub async fn run_get_mpd_outputs(mpd: Option<&MpdBridge>) -> Response {
    let Some(m) = mpd else {
        return Response::error(None, ErrorCode::InvalidRequest, "MPD not configured".to_string());
    };
    match m.outputs().await {
        Ok(outputs) => Response::MpdOutputs(MpdOutputsResponse {
            outputs: outputs.into_iter().map(|o| MpdOutputInfo {
                id: o.id, name: o.name, plugin: o.plugin, enabled: o.enabled,
            }).collect(),
        }),
        Err(e) => Response::error(None, ErrorCode::Internal, e.to_string()),
    }
}

/// Handle a raw `player_command` IPC request.
///
/// Forwards the command directly to the player bridge (mpv IPC socket).
pub async fn run_player_command(player: &PlayerBridge, r: PlayerCommandRequest) -> Response {
    player.send_command(&r.cmd, &r.args).await;
    Response::Ok
}

/// Handle a typed `cmd` IPC request.
///
/// Translates each `PlayerCmd` variant into the appropriate mpv or MPD call.
/// All commands go through `PlayerBridge::send_command` → mpv IPC socket.
pub async fn run_player_cmd(player: &PlayerBridge, mpd: Option<&MpdBridge>, cmd: PlayerCmd) -> Response {
    match cmd {
        // ── Parameterised mpv commands ────────────────────────────────────
        PlayerCmd::Seek { seconds } => {
            player.send_command("seek", &[json!(seconds), json!("relative")]).await;
        }
        PlayerCmd::SeekAbsolute { seconds } => {
            player.send_command("seek", &[json!(seconds), json!("absolute")]).await;
        }
        PlayerCmd::SetVolume { level } => {
            player.send_command("set_property", &[json!("volume"), json!(level)]).await;
        }
        PlayerCmd::AdjustVolume { delta } => {
            player.send_command("add", &[json!("volume"), json!(delta)]).await;
        }
        PlayerCmd::SetSubtitleTrack { id } => {
            player.send_command("set_property", &[json!("sid"), json!(id)]).await;
        }
        PlayerCmd::AdjustSubtitleDelay { delta } => {
            player.send_command("add", &[json!("sub-delay"), json!(delta)]).await;
        }
        PlayerCmd::LoadSubtitle { path } => {
            player.send_command("sub-add", &[json!(path), json!("select")]).await;
        }
        PlayerCmd::SetAudioTrack { id } => {
            player.send_command("set_property", &[json!("aid"), json!(id)]).await;
        }
        PlayerCmd::AdjustAudioDelay { delta } => {
            player.send_command("add", &[json!("audio-delay"), json!(delta)]).await;
        }
        PlayerCmd::SwitchStream { url } => {
            player.send_command("loadfile", &[json!(url), json!("replace")]).await;
        }

        // ── Simple mpv commands ───────────────────────────────────────────
        PlayerCmd::Pause              => player.send_command("cycle",        &[json!("pause")]).await,
        PlayerCmd::Resume             => player.send_command("set_property", &[json!("pause"), json!(false)]).await,
        PlayerCmd::TogglePause        => player.send_command("cycle",        &[json!("pause")]).await,
        PlayerCmd::Stop               => player.send_command("quit",         &[]).await,
        PlayerCmd::ToggleMute         => player.send_command("cycle",        &[json!("mute")]).await,
        PlayerCmd::DisableSubtitles   => player.send_command("set_property", &[json!("sid"), json!("no")]).await,
        PlayerCmd::CycleSubtitles     => player.send_command("cycle",        &[json!("sub")]).await,
        PlayerCmd::ResetSubtitleDelay => player.send_command("set_property", &[json!("sub-delay"), json!(0)]).await,
        PlayerCmd::CycleAudioTracks   => player.send_command("cycle",        &[json!("audio")]).await,
        PlayerCmd::ResetAudioDelay    => player.send_command("set_property", &[json!("audio-delay"), json!(0)]).await,
        PlayerCmd::NextStreamCandidate => player.send_command("next_candidate", &[]).await,
        PlayerCmd::ToggleFullscreen   => player.send_command("cycle",        &[json!("fullscreen")]).await,
        PlayerCmd::Screenshot         => player.send_command("screenshot",   &[]).await,

        // ── MPD commands ──────────────────────────────────────────────────
        PlayerCmd::MpdNext            => {
            if let Some(m) = mpd { if let Err(e) = m.next().await    { warn!("mpd next failed: {e}");    } }
        }
        PlayerCmd::MpdPrev            => {
            if let Some(m) = mpd { if let Err(e) = m.previous().await { warn!("mpd prev failed: {e}");   } }
        }
        PlayerCmd::MpdShuffle         => {
            if let Some(m) = mpd { if let Err(e) = m.shuffle().await  { warn!("mpd shuffle failed: {e}"); } }
        }
        PlayerCmd::MpdClear           => {
            if let Some(m) = mpd { if let Err(e) = m.clear().await    { warn!("mpd clear failed: {e}");  } }
        }
        PlayerCmd::MpdConsume { enabled }      => {
            if let Some(m) = mpd { if let Err(e) = m.set_consume(enabled).await       { warn!("mpd consume failed: {e}"); } }
        }
        PlayerCmd::ReplayGainMode { mode }     => {
            if let Some(m) = mpd { if let Err(e) = m.set_replay_gain(&mode).await     { warn!("mpd replay-gain failed: {e}"); } }
        }
        PlayerCmd::ToggleMpdOutput { id }      => {
            if let Some(m) = mpd { if let Err(e) = m.toggle_output(id).await          { warn!("mpd toggle-output failed: {e}"); } }
        }
        PlayerCmd::MpdSeekAbsolute { seconds } => {
            if let Some(m) = mpd { if let Err(e) = m.seek(seconds).await              { warn!("mpd seek failed: {e}"); } }
        }
        PlayerCmd::MpdCrossfade { secs }       => {
            if let Some(m) = mpd { if let Err(e) = m.set_crossfade(secs).await        { warn!("mpd crossfade failed: {e}"); } }
        }
    }
    Response::Ok
}
