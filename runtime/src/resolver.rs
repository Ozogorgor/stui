//! Stream URL resolver — dispatches resolution to loaded plugins.
//!
//! # Future merge note
//!
//! `resolver.rs`, `streamer.rs`, and `scraper.rs` handle adjacent
//! responsibilities that may eventually converge into a single
//! `sources/` module:
//!
//! ```text
//! scraper   -> find raw sources (URLs, torrent pages, API results)
//! resolver  -> convert sources into typed StreamCandidates
//! streamer  -> prepare / buffer the chosen StreamCandidate for playback
//! ```
//!
//! When that refactor happens, the three files should become:
//! ```text
//! sources/
//!   mod.rs       - shared Source/StreamCandidate types
//!   scraper.rs   - source discovery
//!   resolver.rs  - candidate construction
//!   streamer.rs  - adaptive buffering
//! ```
//!
//! Until then they are kept separate to avoid a large churn commit.
//! See: https://github.com/stui/stui/issues/TODO (stream pipeline merge)

/// Resolver — turns a media entry ID into a playable stream URL.
///
/// Each resolver plugin exposes a `resolve(entry_id) -> StreamResult`
/// function. This module dispatches to the correct execution mode.
use anyhow::Result;

use crate::ipc::SubtitleTrack;
use crate::plugin::{ExecutionMode, LoadedPlugin};
use crate::sandbox::{call_wasm, SandboxCtx};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct StreamResult {
    pub stream_url: String,
    pub quality: Option<String>,
    pub subtitles: Vec<SubtitleTrack>,
}

// ── Public interface ──────────────────────────────────────────────────────────

pub async fn resolve(
    ctx: &SandboxCtx,
    plugin: &LoadedPlugin,
    entry_id: &str,
) -> Result<StreamResult> {
    match &plugin.mode {
        ExecutionMode::Wasm => resolve_wasm(ctx, plugin, entry_id).await,
        ExecutionMode::NativeLib => resolve_native(ctx, entry_id).await,
        ExecutionMode::Grpc(addr) => resolve_grpc(ctx, addr, entry_id).await,
    }
}

// ── WASM ──────────────────────────────────────────────────────────────────────

async fn resolve_wasm(
    ctx: &SandboxCtx,
    plugin: &LoadedPlugin,
    entry_id: &str,
) -> Result<StreamResult> {
    let input = serde_json::json!({ "entry_id": entry_id });
    let result = call_wasm(ctx, &plugin.entrypoint, "resolve", &input).await?;

    let stream_url = result["stream_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("resolver returned no stream_url"))?
        .to_string();

    let quality = result["quality"].as_str().map(|s| s.to_string());

    let subtitles: Vec<SubtitleTrack> = result
        .get("subtitles")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    Ok(StreamResult {
        stream_url,
        quality,
        subtitles,
    })
}

// ── Native / gRPC stubs ───────────────────────────────────────────────────────

async fn resolve_native(ctx: &SandboxCtx, _entry_id: &str) -> Result<StreamResult> {
    anyhow::bail!(
        "Native resolver not yet supported (plugin: '{}')",
        ctx.plugin_name
    )
}

async fn resolve_grpc(ctx: &SandboxCtx, addr: &str, _entry_id: &str) -> Result<StreamResult> {
    anyhow::bail!(
        "gRPC resolver not yet implemented for '{}' at {}",
        ctx.plugin_name,
        addr
    )
}
