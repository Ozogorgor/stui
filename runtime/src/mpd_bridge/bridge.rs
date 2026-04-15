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

use crate::config::types::{MpdConfig, MusicNormalizeConfig};
use crate::ipc::{
    MpdAlbumWire, MpdArtistWire, MpdDirEntryWire, MpdQueueTrackWire,
    MpdSavedPlaylistWire, MpdSongWire,
};
use crate::mediacache::normalize::year::extract_year;
use crate::mediacache::normalize::{self, store as norm_store, NormalizationConfig, RawTags};
use super::client::MpdConnection;

/// Escape a string for use inside an MPD quoted argument (`"..."`).
fn quote_mpd(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        if ch == '\\' || ch == '"' { out.push('\\'); }
        out.push(ch);
    }
    out.push('"');
    out
}

fn parse_u32(v: Option<&String>) -> u32 {
    v.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0)
}
fn parse_f64(v: Option<&String>) -> f64 {
    v.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
}
fn str_or(v: Option<&String>) -> String {
    v.cloned().unwrap_or_default()
}

fn default_entry() -> MpdDirEntryWire {
    MpdDirEntryWire {
        name: String::new(),
        is_dir: false,
        file: String::new(),
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration: 0.0,
        raw_artist: String::new(),
        raw_album: String::new(),
        raw_title: String::new(),
    }
}

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
    normalize_cfg: MusicNormalizeConfig,
}

impl MpdBridge {
    /// Create a bridge. Does NOT connect immediately — connection is lazy on
    /// first command and retried automatically on failure.
    pub fn new(
        config: MpdConfig,
        ipc_tx: tokio::sync::mpsc::Sender<String>,
        normalize_cfg: MusicNormalizeConfig,
    ) -> Self {
        let bridge = MpdBridge {
            config,
            conn: Arc::new(Mutex::new(None)),
            ipc_tx,
            normalize_cfg,
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

    /// Get the duration of the current song in seconds, or 0.0 if unavailable.
    pub async fn current_song_duration(&self) -> f64 {
        match self.cmd_with_kv("currentsong").await {
            Ok(kv) => {
                kv.get("duration")
                    .or_else(|| kv.get("Time"))
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            }
            Err(_) => 0.0,
        }
    }

    /// Get duration with retry loop to handle MPD async metadata loading.
    /// Polls for up to `max_retries` times with `delay`ms between attempts.
    pub async fn current_song_duration_with_retry(&self, max_retries: u32, delay_ms: u64) -> f64 {
        for _ in 0..max_retries {
            let duration = self.current_song_duration().await;
            if duration > 0.0 {
                return duration;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }
        0.0
    }

    /// Execute a command and return key-value response.
    pub async fn cmd_with_kv(&self, cmd: &str) -> Result<std::collections::HashMap<String, String>> {
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        match conn.command_kv(cmd).await {
            Ok(kv) => Ok(kv),
            Err(e) => {
                warn!("mpd command {cmd} failed: {e} — dropping connection");
                *guard = None;
                Err(anyhow::anyhow!("{}", e))
            }
        }
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

    /// Adjust volume by a relative delta (MPD `volume {n}`, clamped to −100..100).
    pub async fn adjust_volume(&self, delta: i32) -> Result<()> {
        let clamped = delta.clamp(-100, 100);
        self.cmd(&format!("volume {clamped}")).await
    }

    /// Seek relative to the current position using `seekcur {+n}` / `seekcur {-n}`.
    pub async fn seek_relative(&self, secs: f64) -> Result<()> {
        if secs >= 0.0 {
            self.cmd(&format!("seekcur +{:.3}", secs.abs())).await
        } else {
            self.cmd(&format!("seekcur {secs:.3}")).await
        }
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

    /// Find the stui DSP FIFO output (`"stui-dsp"`) and enable it if currently disabled.
    ///
    /// Returns `Ok(true)` if the output exists (and was enabled), `Ok(false)` if MPD
    /// has no output with that name — meaning the user needs to add the FIFO stanza to
    /// `mpd.conf` (see [`crate::dsp::mpd_config::ensure_mpd_conf`]).
    pub async fn ensure_dsp_output_enabled(&self) -> Result<bool> {
        let outputs = self.outputs().await?;
        let Some(out) = outputs.iter().find(|o| o.name == crate::dsp::mpd_config::FIFO_OUTPUT_NAME) else {
            return Ok(false);
        };
        if !out.enabled {
            self.cmd(&format!("enableoutput {}", out.id)).await?;
            info!(id = out.id, "enabled stui-dsp MPD FIFO output");
        }
        Ok(true)
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

    // ── Library / browse queries ──────────────────────────────────────────

    /// Fetch the full playback queue via `playlistinfo`.
    pub async fn get_queue(&self) -> Result<Vec<MpdQueueTrackWire>> {
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let records = match conn.command_records("playlistinfo", "file").await {
            Ok(r) => r,
            Err(e) => { *guard = None; return Err(e); }
        };
        Ok(records.into_iter().map(|r| MpdQueueTrackWire {
            id:       parse_u32(r.get("Id")),
            pos:      parse_u32(r.get("Pos")),
            title:    str_or(r.get("Title")),
            artist:   str_or(r.get("Artist")),
            album:    str_or(r.get("Album")),
            duration: parse_f64(r.get("duration").or_else(|| r.get("Time"))),
            file:     str_or(r.get("file")),
        }).collect())
    }

    /// `list artist` — every distinct artist in the MPD database.
    pub async fn list_artists(&self) -> Result<Vec<MpdArtistWire>> {
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let records = match conn.command_records("list artist", "Artist").await {
            Ok(r) => r,
            Err(e) => { *guard = None; return Err(e); }
        };
        Ok(records.into_iter().filter_map(|r| {
            let name = r.get("Artist")?.clone();
            if name.is_empty() { None } else { Some(MpdArtistWire { name }) }
        }).collect())
    }

    /// `list album artist "X" group date` — albums by `artist` with release year.
    ///
    /// Pass empty `artist` to list all albums across every artist.  The `group`
    /// keyword must appear AFTER filter pairs (MPD 0.21+ protocol syntax);
    /// on older MPD it's silently ignored and Year is returned empty.
    ///
    /// De-duplicates entries that share the same (title, artist) — this happens
    /// when an album's tracks have inconsistent Date tags (some tagged with a
    /// full date like `2017-05-03`, others with just `2017`, others missing).
    /// The best-populated year (preferring 4-digit year extracted from any
    /// variant) is kept.
    pub async fn list_albums(&self, artist: &str) -> Result<Vec<MpdAlbumWire>> {
        let cmd = if artist.is_empty() {
            "list album group artist group date".to_string()
        } else {
            format!("list album artist {} group date", quote_mpd(artist))
        };
        // Phase 1: pull album rows. Held in its own scope so the
        // connection lock is released before phase 2 runs follow-up
        // queries — otherwise fetch_originaldate_year would deadlock
        // trying to re-acquire the same lock.
        let records = {
            let mut guard = self.conn.lock().await;
            let conn = Self::get_or_connect(&mut guard, &self.config).await?;
            match conn.command_records(&cmd, "Album").await {
                Ok(r) => r,
                Err(e) => { *guard = None; return Err(e); }
            }
        };

        // Phase 2: dedup + back-fill missing years from OriginalDate.
        //
        // Two releases of the same album by the same artist (1996 original
        // vs 2007 remaster) carry identical Album/Artist tags but different
        // Date values; MPD reports them as separate records via `group
        // date`. We key dedup on (title, artist, raw date) so both rows
        // survive into the TUI, and list_songs disambiguates them with an
        // exact `date` filter.
        //
        // Some releases (typically older ones tagged by tools that only
        // populate the MusicBrainz "OriginalDate") leave Date empty. For
        // those we run a single follow-up `list originaldate` query to
        // recover a year for display, while still keeping raw_date="" as
        // the dedup key (so the empty-Date release stays distinct from
        // any release whose Date is populated).
        let mut out: Vec<MpdAlbumWire> = Vec::new();
        let mut seen: std::collections::HashSet<(String, String, String)> = std::collections::HashSet::new();
        for r in records {
            let Some(title) = r.get("Album").cloned() else { continue };
            if title.is_empty() { continue; }
            let entry_artist = r.get("Artist").cloned().unwrap_or_else(|| artist.to_string());
            let raw_date = str_or(r.get("Date"));
            let mut year = extract_year(&raw_date);
            if year.is_empty() {
                year = self.fetch_originaldate_year(&title, &entry_artist).await
                    .unwrap_or_default();
            }
            info!(
                album = %title,
                artist = %entry_artist,
                raw_date = %raw_date,
                extracted_year = %year,
                "mpd: list_albums record"
            );
            let key = (title.clone(), entry_artist.clone(), raw_date.clone());
            if seen.insert(key) {
                out.push(MpdAlbumWire {
                    title,
                    artist: entry_artist,
                    year,
                    date: raw_date,
                    raw_artist: String::new(),
                    raw_title: String::new(),
                });
            }
        }
        // Apply normalization pipeline if enabled.
        if self.normalize_cfg.enabled {
            let exceptions = norm_store::global().map(|s| s.get()).unwrap_or_default();
            for album in out.iter_mut() {
                let raw = RawTags {
                    artist: album.artist.clone(),
                    album: album.title.clone(),
                    date: album.date.clone(),
                    ..Default::default()
                };
                let cfg = NormalizationConfig {
                    enabled: true,
                    use_lookup: self.normalize_cfg.use_lookup,
                    exceptions: &exceptions,
                };
                let n = normalize::normalize(&raw, &cfg, None);
                if n.artist != album.artist {
                    album.raw_artist = album.artist.clone();
                    album.artist = n.artist;
                }
                if n.album != album.title {
                    album.raw_title = album.title.clone();
                    album.title = n.album;
                }
                // year may be re-extracted but should be identical; trust pipeline.
                album.year = n.year;
            }
        }
        Ok(out)
    }

    /// Look up an album's OriginalDate when MPD's `Date` group came back
    /// empty. Returns the first parseable 4-digit year, or empty if the
    /// album has no OriginalDate either. Failures are swallowed (returned
    /// as empty) since this is a best-effort display enhancement.
    async fn fetch_originaldate_year(&self, title: &str, artist: &str) -> Result<String> {
        let cmd = format!(
            "list originaldate album {} artist {}",
            quote_mpd(title),
            quote_mpd(artist),
        );
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let pairs = match conn.command_kv_ordered(&cmd).await {
            Ok(p) => p,
            Err(e) => { *guard = None; return Err(e); }
        };
        for (key, value) in pairs {
            // MPD spells the tag "OriginalDate" in responses; accept any
            // case to be defensive against server-side normalization.
            if key.eq_ignore_ascii_case("OriginalDate") {
                let y = extract_year(&value);
                if !y.is_empty() {
                    return Ok(y);
                }
            }
        }
        Ok(String::new())
    }

    /// `find album "Y" artist "X" [date "Z"]` — tracks on a specific release.
    /// When `date` is non-empty it's passed as an exact MPD `date` filter so
    /// remasters / reissues that share Album+Artist tags don't collide. The
    /// value must be the raw MPD `Date:` string (e.g. "1996-11-01"); the TUI
    /// gets it from `MpdAlbumWire.date` on the matching list_albums row.
    pub async fn list_songs(&self, artist: &str, album: &str, date: &str) -> Result<Vec<MpdSongWire>> {
        let mut cmd = if artist.is_empty() {
            format!("find album {}", quote_mpd(album))
        } else {
            format!("find album {} artist {}", quote_mpd(album), quote_mpd(artist))
        };
        if !date.is_empty() {
            cmd.push_str(&format!(" date {}", quote_mpd(date)));
        }
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let records = match conn.command_records(&cmd, "file").await {
            Ok(r) => r,
            Err(e) => { *guard = None; return Err(e); }
        };
        Ok(records.into_iter().filter_map(|r| {
            let file = r.get("file")?.clone();
            let mut song = MpdSongWire {
                title:    str_or(r.get("Title")),
                artist:   str_or(r.get("Artist")),
                album:    str_or(r.get("Album")),
                duration: parse_f64(r.get("duration").or_else(|| r.get("Time"))),
                file,
                raw_artist: String::new(),
                raw_album: String::new(),
                raw_title: String::new(),
            };
            apply_song_normalize(
                &self.normalize_cfg,
                &mut song.artist, &mut song.raw_artist,
                &mut song.album,  &mut song.raw_album,
                &mut song.title,  &mut song.raw_title,
            );
            Some(song)
        }).collect())
    }

    /// `lsinfo PATH` — browse the MPD music directory tree.
    ///
    /// Entries in the response may be directories, playlists, or files (songs)
    /// in arbitrary order; we preserve MPD's emission order.
    pub async fn browse(&self, path: &str) -> Result<Vec<MpdDirEntryWire>> {
        let cmd = if path.is_empty() {
            "lsinfo".to_string()
        } else {
            format!("lsinfo {}", quote_mpd(path))
        };
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let pairs = match conn.command_kv_ordered(&cmd).await {
            Ok(p) => p,
            Err(e) => { *guard = None; return Err(e); }
        };

        // Walk the ordered kv stream.  A record starts on any of: `directory`,
        // `file`, `playlist`.  Every subsequent key belongs to that record
        // until the next record starter.
        let mut entries: Vec<MpdDirEntryWire> = Vec::new();
        let mut current: Option<MpdDirEntryWire> = None;

        let normalize_cfg = self.normalize_cfg.clone();
        let flush = |cur: &mut Option<MpdDirEntryWire>, out: &mut Vec<MpdDirEntryWire>| {
            if let Some(mut entry) = cur.take() {
                apply_song_normalize(
                    &normalize_cfg,
                    &mut entry.artist, &mut entry.raw_artist,
                    &mut entry.album,  &mut entry.raw_album,
                    &mut entry.title,  &mut entry.raw_title,
                );
                out.push(entry);
            }
        };

        for (k, v) in pairs {
            match k.as_str() {
                "directory" => {
                    flush(&mut current, &mut entries);
                    let name = v.rsplit('/').next().unwrap_or(&v).to_string();
                    current = Some(MpdDirEntryWire {
                        name,
                        is_dir: true,
                        file: v,
                        ..default_entry()
                    });
                }
                "playlist" => {
                    flush(&mut current, &mut entries);
                    current = Some(MpdDirEntryWire {
                        name: v.clone(),
                        is_dir: false,
                        file: v,
                        ..default_entry()
                    });
                }
                "file" => {
                    flush(&mut current, &mut entries);
                    let name = v.rsplit('/').next().unwrap_or(&v).to_string();
                    current = Some(MpdDirEntryWire {
                        name,
                        is_dir: false,
                        file: v,
                        ..default_entry()
                    });
                }
                _ => {
                    if let Some(ref mut entry) = current {
                        match k.as_str() {
                            "Title"    => entry.title  = v,
                            "Artist"   => entry.artist = v,
                            "Album"    => entry.album  = v,
                            "Time"     => entry.duration = v.parse().unwrap_or(0.0),
                            "duration" => entry.duration = v.parse().unwrap_or(0.0),
                            _ => {}
                        }
                    }
                }
            }
        }
        flush(&mut current, &mut entries);
        Ok(entries)
    }

    /// `listplaylists` — saved MPD playlists.
    pub async fn get_playlists(&self) -> Result<Vec<MpdSavedPlaylistWire>> {
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let records = match conn.command_records("listplaylists", "playlist").await {
            Ok(r) => r,
            Err(e) => { *guard = None; return Err(e); }
        };
        Ok(records.into_iter().filter_map(|r| {
            let name = r.get("playlist")?.clone();
            Some(MpdSavedPlaylistWire {
                name,
                modified: str_or(r.get("Last-Modified")),
            })
        }).collect())
    }

    /// `listplaylistinfo NAME` — tracks inside a saved playlist.
    pub async fn get_playlist_tracks(&self, name: &str) -> Result<Vec<MpdSongWire>> {
        let cmd = format!("listplaylistinfo {}", quote_mpd(name));
        let mut guard = self.conn.lock().await;
        let conn = Self::get_or_connect(&mut guard, &self.config).await?;
        let records = match conn.command_records(&cmd, "file").await {
            Ok(r) => r,
            Err(e) => { *guard = None; return Err(e); }
        };
        Ok(records.into_iter().filter_map(|r| {
            let file = r.get("file")?.clone();
            let mut song = MpdSongWire {
                title:    str_or(r.get("Title")),
                artist:   str_or(r.get("Artist")),
                album:    str_or(r.get("Album")),
                duration: parse_f64(r.get("duration").or_else(|| r.get("Time"))),
                file,
                raw_artist: String::new(),
                raw_album: String::new(),
                raw_title: String::new(),
            };
            apply_song_normalize(
                &self.normalize_cfg,
                &mut song.artist, &mut song.raw_artist,
                &mut song.album,  &mut song.raw_album,
                &mut song.title,  &mut song.raw_title,
            );
            Some(song)
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
        Ok(slot.as_mut().expect("mpd_bridge: connection slot populated above"))
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

    let mut last_state: Option<String> = None;

    loop {
        conn.idle(&["player", "mixer", "options", "playlist"]).await?;

        // Something changed — fetch current state and push to TUI.
        let status  = status_conn.command_kv("status").await?;
        let current = status_conn.command_kv("currentsong").await?;

        let state    = status.get("state").map(|s| s.to_string()).unwrap_or_else(|| "stop".to_string());
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

        // Detect play -> stop transition to emit player_ended.
        // Note: This is a best-effort heuristic; MPD doesn't distinguish EOF from
        // user-initiated stop, so we use "stopped" as a neutral reason.
        if let Some(ref prev) = last_state {
            if (prev == "play" || prev == "pause") && state == "stop" {
                let ended = serde_json::to_string(&serde_json::json!({
                    "type": "player_ended",
                    "reason": "stopped",
                    "error": ""
                })).unwrap_or_default();
                let _ = ipc_tx.send(ended).await;
            }
        }
        last_state = Some(state.clone());

        let wire = MpdStatusWire {
            r#type: "mpd_status",
            state: &state,
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

/// Normalize a song-like record in place. Stashes raw values when the pipeline
/// changes a field. No-op when `cfg.enabled == false`.
fn apply_song_normalize(
    cfg: &MusicNormalizeConfig,
    artist: &mut String, raw_artist: &mut String,
    album: &mut String, raw_album: &mut String,
    title: &mut String, raw_title: &mut String,
) {
    if !cfg.enabled { return; }
    let exceptions = norm_store::global().map(|s| s.get()).unwrap_or_default();
    let raw = RawTags {
        artist: artist.clone(),
        album: album.clone(),
        title: title.clone(),
        ..Default::default()
    };
    let nc = NormalizationConfig {
        enabled: true,
        use_lookup: cfg.use_lookup,
        exceptions: &exceptions,
    };
    let n = normalize::normalize(&raw, &nc, None);
    if n.artist != *artist { *raw_artist = artist.clone(); *artist = n.artist; }
    if n.album != *album   { *raw_album  = album.clone();  *album  = n.album; }
    if n.title != *title   { *raw_title  = title.clone();  *title  = n.title; }
}
