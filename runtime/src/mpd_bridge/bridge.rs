//! High-level MPD bridge: connection management, playback API, and idle-loop
//! event forwarding to the Go TUI.
//!
//! # Two connections
//!
//! MPD requires separate connections for commands and for `idle` because `idle`
//! blocks the connection until something changes.
//!
//! - **command connection** — used for all play/pause/seek/etc. calls
//! - **idle connection**    — stays in `idle player mixer options` permanently;
//!                            on wake-up, fetches fresh `status`+`currentsong`
//!                            and pushes an `mpd_status` event to the TUI

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::types::MpdConfig;
use super::client::MpdConnection;

// ── Wire event types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MpdOutput {
    pub id:      u32,
    pub name:    String,
    pub plugin:  String,
    pub enabled: bool,
}

/// Pushed to the TUI on every player/mixer/options change.
#[derive(Serialize)]
struct MpdStatusWire<'a> {
    r#type:        &'static str,
    state:         &'a str,             // "play" | "pause" | "stop"
    song_title:    Option<&'a str>,
    song_artist:   Option<&'a str>,
    song_album:    Option<&'a str>,
    elapsed:       f64,
    duration:      f64,
    volume:        u32,                 // 0–100
    bitrate:       Option<u32>,         // kbps
    audio_format:  Option<&'a str>,     // "192000:24:2"
    replay_gain:   &'a str,
    crossfade:     u32,
    consume:       bool,
    random:        bool,
    queue_length:  u32,
}

// ── MpdBridge ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct MpdBridge {
    config: MpdConfig,
    conn:   Arc<Mutex<Option<MpdConnection>>>,
    ipc_tx: tokio::sync::mpsc::Sender<String>,
}

impl MpdBridge {
    /// Create a bridge. Does NOT connect immediately — connection is lazy on
    /// first command and retried automatically on failure.
    pub fn new(config: MpdConfig, ipc_tx: tokio::sync::mpsc::Sender<String>) -> Self {
        let bridge = MpdBridge {
            config,
            conn: Arc::new(Mutex::new(None)),
            ipc_tx,
        };
        bridge.start_idle_loop();
        bridge
    }

    // ── Public playback API ───────────────────────────────────────────────

    /// Clear the queue, add `url`, and start playing. Idempotent.
    pub async fn queue_and_play(&self, url: &str) -> Result<()> {
        self.cmd("clear").await?;
        self.cmd(&format!("add {url}")).await?;
        self.cmd("play").await?;
        info!(url, "mpd: queued and playing");
        Ok(())
    }

    pub async fn pause(&self)         -> Result<()> { self.cmd("pause 1").await }
    pub async fn resume(&self)        -> Result<()> { self.cmd("pause 0").await }
    pub async fn toggle_pause(&self)  -> Result<()> { self.cmd("pause").await }
    pub async fn stop(&self)          -> Result<()> { self.cmd("stop").await }
    pub async fn next(&self)          -> Result<()> { self.cmd("next").await }
    pub async fn previous(&self)      -> Result<()> { self.cmd("previous").await }
    pub async fn clear(&self)         -> Result<()> { self.cmd("clear").await }
    pub async fn shuffle(&self)       -> Result<()> { self.cmd("shuffle").await }

    pub async fn seek(&self, secs: f64) -> Result<()> {
        self.cmd(&format!("seekcur {secs:.3}")).await
    }

    pub async fn set_volume(&self, vol: u32) -> Result<()> {
        self.cmd(&format!("setvol {}", vol.min(100))).await
    }

    pub async fn set_replay_gain(&self, mode: &str) -> Result<()> {
        self.cmd(&format!("replay_gain_mode {mode}")).await
    }

    pub async fn set_crossfade(&self, secs: u32) -> Result<()> {
        self.cmd(&format!("crossfade {secs}")).await
    }

    pub async fn set_mixramp_db(&self, db: f64) -> Result<()> {
        self.cmd(&format!("mixrampdb {db}")).await
    }

    pub async fn set_consume(&self, enabled: bool) -> Result<()> {
        self.cmd(&format!("consume {}", if enabled { 1 } else { 0 })).await
    }

    pub async fn toggle_output(&self, id: u32) -> Result<()> {
        // Get current state then toggle.
        let outputs = self.outputs().await?;
        if let Some(out) = outputs.iter().find(|o| o.id == id) {
            if out.enabled {
                self.cmd(&format!("disableoutput {id}")).await?;
            } else {
                self.cmd(&format!("enableoutput {id}")).await?;
            }
        }
        Ok(())
    }

    /// List all configured MPD audio outputs.
    pub async fn outputs(&self) -> Result<Vec<MpdOutput>> {
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let records = conn.command_records("outputs", "outputid").await?;
        Ok(records.into_iter().filter_map(|r| {
            Some(MpdOutput {
                id:      r.get("outputid")?.parse().ok()?,
                name:    r.get("outputname").cloned().unwrap_or_default(),
                plugin:  r.get("plugin").cloned().unwrap_or_default(),
                enabled: r.get("outputenabled").map(|v| v == "1").unwrap_or(false),
            })
        }).collect())
    }

    /// Apply initial config to the live MPD daemon (replay gain, crossfade, etc.)
    pub async fn apply_config(&self) {
        let _ = self.set_replay_gain(&self.config.replay_gain.clone()).await;
        let _ = self.set_crossfade(self.config.crossfade_secs).await;
        if let Some(db) = self.config.mixramp_db {
            let _ = self.set_mixramp_db(db).await;
        }
        let _ = self.set_consume(self.config.consume).await;
    }

    // ── Internals ─────────────────────────────────────────────────────────

    async fn cmd(&self, cmd: &str) -> Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        match conn.run_command(cmd).await {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!("mpd command `{cmd}` failed: {e} — dropping connection");
                *guard = None;
                Err(e)
            }
        }
    }

    async fn get_or_connect<'a>(
        slot:   &'a mut Option<MpdConnection>,
        config: &MpdConfig,
    ) -> Result<&'a mut MpdConnection> {
        if slot.is_none() {
            *slot = Some(MpdConnection::connect(
                &config.host,
                config.port,
                config.password.as_deref(),
            ).await?);
        }
        Ok(slot.as_mut().unwrap())
    }

    /// Background task: maintain an idle connection and push `mpd_status`
    /// events whenever player/mixer/options state changes.
    fn start_idle_loop(&self) {
        let config = self.config.clone();
        let ipc_tx = self.ipc_tx.clone();

        tokio::spawn(async move {
            loop {
                match run_idle_loop(&config, &ipc_tx).await {
                    Ok(()) => {}
                    Err(e) => {
                        warn!("mpd idle loop error: {e} — reconnecting in 5s");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }
}

async fn run_idle_loop(
    config: &MpdConfig,
    ipc_tx: &tokio::sync::mpsc::Sender<String>,
) -> Result<()> {
    let mut conn = MpdConnection::connect(
        &config.host,
        config.port,
        config.password.as_deref(),
    ).await?;

    info!(host = %config.host, port = config.port, "mpd idle loop connected");

    // A second connection for fetching status while idle is blocked.
    let mut status_conn = MpdConnection::connect(
        &config.host,
        config.port,
        config.password.as_deref(),
    ).await?;

    loop {
        conn.idle(&["player", "mixer", "options", "playlist"]).await?;

        // Something changed — fetch current state and push to TUI.
        let status  = status_conn.command_kv("status").await?;
        let current = status_conn.command_kv("currentsong").await?;

        let state    = status.get("state").map(String::as_str).unwrap_or("stop");
        let elapsed  = status.get("elapsed").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let duration = status.get("duration").and_then(|v| v.parse::<f64>().ok())
            .or_else(|| status.get("time").and_then(|t| {
                t.split(':').nth(1).and_then(|s| s.parse::<f64>().ok())
            })).unwrap_or(0.0);
        let volume       = status.get("volume").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let bitrate      = status.get("bitrate").and_then(|v| v.parse::<u32>().ok());
        let audio_format = status.get("audio").map(String::as_str);
        let replay_gain  = status.get("replay_gain_mode").map(String::as_str).unwrap_or("off");
        let crossfade    = status.get("xfade").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let consume      = status.get("consume").map(|v| v == "1").unwrap_or(false);
        let random       = status.get("random").map(|v| v == "1").unwrap_or(false);
        let queue_length = status.get("playlistlength").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);

        let song_title  = current.get("Title").map(String::as_str);
        let song_artist = current.get("Artist").map(String::as_str);
        let song_album  = current.get("Album").map(String::as_str);

        let wire = MpdStatusWire {
            r#type: "mpd_status",
            state,
            song_title,
            song_artist,
            song_album,
            elapsed,
            duration,
            volume,
            bitrate,
            audio_format,
            replay_gain,
            crossfade,
            consume,
            random,
            queue_length,
        };

        if let Ok(mut msg) = serde_json::to_string(&wire) {
            msg.push('\n');
            let _ = ipc_tx.send(msg).await;
        }
    }
}
