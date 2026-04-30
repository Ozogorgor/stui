//! Fetch + parse `album.getInfo` from last.fm and project the
//! `tracks.track[]` block into the runtime's `LastfmAlbumTrackWire`
//! shape for IPC.
//!
//! Endpoint:
//!   https://ws.audioscrobbler.com/2.0/?method=album.getInfo
//!     &artist=<urlencoded>&album=<urlencoded>
//!     &api_key=<key>&format=json&autocorrect=1
//!
//! API key resolution:
//!   1. `Secrets::lastfm_api_key()` — pulls from
//!      `~/.config/stui/secrets.env` where the user actually keeps
//!      their keys (same source the lastfm plugin reads via the
//!      runtime's `cache_get("__env:LASTFM_API_KEY")` shim).
//!   2. `LASTFM_API_KEY` process env var as a fallback for users
//!      who export it directly.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::config::secrets::Secrets;
use crate::ipc::LastfmAlbumTrackWire;

const API_BASE: &str = "https://ws.audioscrobbler.com/2.0";
const USER_AGENT: &str = "stui-runtime/0.1.0 ( https://github.com/stui/stui )";

/// Fetch the tracklist for `(artist, album)`. Returns an empty Vec
/// when last.fm has no `tracks.track[]` block (API returned an
/// album shell without listings — common for obscure releases).
/// Errors propagate transport failures, missing API key, and
/// hard-error responses.
pub async fn fetch(artist: &str, album: &str) -> Result<Vec<LastfmAlbumTrackWire>> {
    // Try secrets.env first (canonical home for stui API keys),
    // fall back to a real process env var for users who prefer
    // exporting it directly. Plugin uses the same precedence via
    // its cache_get("__env:…") shim.
    let key = Secrets::load()
        .lastfm_api_key()
        .or_else(|| std::env::var("LASTFM_API_KEY").ok().filter(|s| !s.is_empty()))
        .ok_or_else(|| anyhow!("LASTFM_API_KEY not set in secrets.env or environment"))?;

    let url = format!(
        "{API_BASE}?method=album.getInfo&artist={a}&album={al}&api_key={k}&format=json&autocorrect=1",
        a = urlencoding::encode(artist),
        al = urlencoding::encode(album),
        k = key,
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
        .context("build reqwest client")?;
    let resp = client
        .get(&url)
        .send()
        .await
        .context("album.getInfo request")?;
    let status = resp.status();
    let body = resp.text().await.context("read album.getInfo body")?;
    if !status.is_success() {
        return Err(anyhow!("album.getInfo HTTP {status}: {body}"));
    }
    let env: Envelope = serde_json::from_str(&body)
        .with_context(|| format!("parse album.getInfo JSON ({} bytes)", body.len()))?;

    if let Some(msg) = env.message {
        // last.fm signals "album not found" via {error: 6, message: "..."}.
        return Err(anyhow!("lastfm: {msg}"));
    }
    let info = match env.album {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };
    let raw_tracks = match info.tracks.and_then(|t| t.track) {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };

    let tracks = raw_tracks
        .into_iter()
        .enumerate()
        .map(|(i, t)| LastfmAlbumTrackWire {
            // `@attr.rank` is the canonical ordering when present;
            // fall back to the array index + 1 so we never emit 0.
            number: t.attr.and_then(|a| a.rank).unwrap_or((i as u32) + 1),
            title: t.name.unwrap_or_default(),
            duration_secs: t.duration.and_then(|d| d.into_secs()),
            mbid: t.mbid.filter(|s| !s.is_empty()),
        })
        .collect();
    Ok(tracks)
}

// ── JSON shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)] album: Option<AlbumInfo>,
    /// On error, last.fm returns `{ error: <int>, message: "<text>" }`
    /// at the top level instead of `album`.
    #[serde(default)] message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlbumInfo {
    #[serde(default)] tracks: Option<TracksWrap>,
}

/// last.fm wraps the array in a `track` field (singular), so we
/// double-step: outer `tracks` object → `track` array.
#[derive(Debug, Deserialize)]
struct TracksWrap {
    #[serde(default, deserialize_with = "deserialize_tracks_field")]
    track: Option<Vec<RawTrack>>,
}

/// last.fm sometimes serialises a single-track album as an object
/// rather than a one-element array. Accept both.
fn deserialize_tracks_field<'de, D>(deserializer: D) -> Result<Option<Vec<RawTrack>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    match v {
        serde_json::Value::Array(_) => {
            serde_json::from_value(v).map(Some).map_err(serde::de::Error::custom)
        }
        serde_json::Value::Object(_) => {
            let one: RawTrack =
                serde_json::from_value(v).map_err(serde::de::Error::custom)?;
            Ok(Some(vec![one]))
        }
        serde_json::Value::Null => Ok(None),
        _ => Ok(None),
    }
}

#[derive(Debug, Deserialize)]
struct RawTrack {
    #[serde(default)] name: Option<String>,
    /// last.fm returns duration as a JSON *number* (seconds) for
    /// album.getInfo, but a *string* in some other endpoints. Accept
    /// either to avoid serde rejecting the whole envelope.
    #[serde(default)] duration: Option<DurationField>,
    #[serde(default)] mbid: Option<String>,
    #[serde(default, rename = "@attr")] attr: Option<TrackAttr>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DurationField {
    Int(i64),
    Str(String),
}

impl DurationField {
    fn into_secs(self) -> Option<u32> {
        match self {
            DurationField::Int(n) if n > 0 => Some(n as u32),
            DurationField::Int(_) => None,
            DurationField::Str(s) => s.parse::<u32>().ok().filter(|n| *n > 0),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TrackAttr {
    #[serde(default)] rank: Option<u32>,
}
