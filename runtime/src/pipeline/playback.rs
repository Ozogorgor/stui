//! Playback pipeline — translate IPC player commands to engine calls.
//!
//! Also owns the top-level `run_play` entry point that fires off the player
//! and the skip-detection analyser as independent background tasks.

use std::sync::Arc;

use serde_json::json;
use percent_encoding::percent_decode_str;
use tracing::warn;

fn display_title_from_url(url: &str) -> String {
    if let Some(pos) = url.rfind('/') {
        let segment = &url[pos + 1..];
        if !segment.is_empty() {
            if let Ok(decoded) = percent_decode_str(segment).decode_utf8() {
                let name = decoded.trim();
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    url.to_string()
}

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
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
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
    let via_mpd = player.is_mpd_active();
    match cmd {
        // ── Shared commands — routed to MPD or mpv depending on active player ──

        PlayerCmd::Pause => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.pause().await { warn!("mpd pause failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("set_property", &[json!("pause"), json!(true)]).await;
            }
        }
        PlayerCmd::Resume => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.resume().await { warn!("mpd resume failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("set_property", &[json!("pause"), json!(false)]).await;
            }
        }
        PlayerCmd::TogglePause => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.toggle_pause().await { warn!("mpd toggle_pause failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("cycle", &[json!("pause")]).await;
            }
        }
        PlayerCmd::Stop => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.stop().await { warn!("mpd stop failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("quit", &[]).await;
            }
        }
        PlayerCmd::Seek { seconds } => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.seek_relative(seconds).await { warn!("mpd seek_relative failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("seek", &[json!(seconds), json!("relative")]).await;
            }
        }
        PlayerCmd::SeekAbsolute { seconds } => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.seek(seconds).await { warn!("mpd seek failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("seek", &[json!(seconds), json!("absolute")]).await;
            }
        }
        PlayerCmd::SetVolume { level } => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.set_volume(level.clamp(0.0, 100.0) as u32).await { warn!("mpd set_volume failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("set_property", &[json!("volume"), json!(level)]).await;
            }
        }
        PlayerCmd::AdjustVolume { delta } => {
            if via_mpd {
                if let Some(m) = mpd {
                    if let Err(e) = m.adjust_volume(delta.clamp(-100.0, 100.0) as i32).await { warn!("mpd adjust_volume failed: {e}"); }
                } else {
                    warn!("mpd flagged active but bridge unavailable");
                }
            } else {
                player.send_command("add", &[json!("volume"), json!(delta)]).await;
            }
        }
        PlayerCmd::SwitchStream { url } => {
            if via_mpd {
                let title = display_title_from_url(&url);
                player.switch_stream_mpd(&url, &title).await;
            } else {
                player.send_command("loadfile", &[json!(url), json!("replace")]).await;
            }
        }

        // ── mpv-only parameterised commands ──────────────────────────────
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

        // ── Simple mpv-only commands ──────────────────────────────────────
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

        // ── Playlist commands ────────────────────────────────────────────────
        PlayerCmd::MpdPlaylistSave { name } => {
            if let Some(m) = mpd { if let Err(e) = m.save_playlist(&name).await   { warn!("mpd playlist save failed: {e}"); } }
        }
        PlayerCmd::MpdPlaylistLoad { name } => {
            if let Some(m) = mpd { if let Err(e) = m.load_playlist(&name).await   { warn!("mpd playlist load failed: {e}"); } }
        }
        PlayerCmd::MpdPlaylistAppend { name } => {
            if let Some(m) = mpd { if let Err(e) = m.append_playlist(&name).await { warn!("mpd playlist append failed: {e}"); } }
        }
        PlayerCmd::MpdPlaylistDelete { name } => {
            if let Some(m) = mpd { if let Err(e) = m.delete_playlist(&name).await { warn!("mpd playlist delete failed: {e}"); } }
        }
        PlayerCmd::MpdPlaylistAddTrack { name, uri } => {
            if let Some(m) = mpd { if let Err(e) = m.add_to_playlist(&name, &uri).await { warn!("mpd playlist add-track failed: {e}"); } }
        }
        PlayerCmd::MpdPlaylistCreate { name, uris } => {
            if let Some(m) = mpd { if let Err(e) = m.create_playlist(&name, &uris).await { warn!("mpd playlist create failed: {e}"); } }
        }
        PlayerCmd::MpdPlaylistRemoveTrack { name, pos } => {
            if let Some(m) = mpd { if let Err(e) = m.remove_from_playlist(&name, pos).await { warn!("mpd playlist remove-track failed: {e}"); } }
        }

        // ── Queue manipulation ───────────────────────────────────────────────
        PlayerCmd::MpdAdd { uri } => {
            if let Some(m) = mpd { if let Err(e) = m.add(&uri).await { warn!("mpd add failed: {e}"); } }
        }
        PlayerCmd::MpdRemove { id } => {
            if let Some(m) = mpd { if let Err(e) = m.remove_id(id).await { warn!("mpd remove failed: {e}"); } }
        }
        PlayerCmd::MpdPlayId { id } => {
            if let Some(m) = mpd { if let Err(e) = m.play_id(id).await { warn!("mpd play_id failed: {e}"); } }
        }
        PlayerCmd::MpdSetVolume { volume } => {
            if let Some(m) = mpd { if let Err(e) = m.set_volume(volume).await { warn!("mpd set_volume failed: {e}"); } }
        }
        PlayerCmd::MpdSeek { id, time } => {
            if let Some(m) = mpd { if let Err(e) = m.seek_id(id, time).await { warn!("mpd seek failed: {e}"); } }
        }
        PlayerCmd::MpdTogglePause => {
            if let Some(m) = mpd { if let Err(e) = m.toggle_pause().await { warn!("mpd toggle_pause failed: {e}"); } }
        }
        PlayerCmd::MpdStop => {
            if let Some(m) = mpd { if let Err(e) = m.stop().await { warn!("mpd stop failed: {e}"); } }
        }
        PlayerCmd::MpdUpdate => {
            if let Some(m) = mpd { if let Err(e) = m.update_library(None).await { warn!("mpd update failed: {e}"); } }
        }
        PlayerCmd::MpdToggleRepeat => {
            if let Some(m) = mpd { if let Err(e) = m.toggle_repeat().await { warn!("mpd toggle_repeat failed: {e}"); } }
        }
        PlayerCmd::MpdToggleSingle => {
            if let Some(m) = mpd { if let Err(e) = m.toggle_single().await { warn!("mpd toggle_single failed: {e}"); } }
        }
        PlayerCmd::MpdToggleRandom => {
            if let Some(m) = mpd { if let Err(e) = m.toggle_random().await { warn!("mpd toggle_random failed: {e}"); } }
        }
    }
    Response::Ok
}
