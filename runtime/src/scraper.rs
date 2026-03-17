//! Plugin search dispatch helper — fans out queries across loaded plugins.
//!
//! # Future merge note
//!
//! See `resolver.rs` for the rationale on why scraper, resolver, and
//! streamer may eventually merge into a `sources/` module.

/// Scraper — dispatches search requests to provider plugins.
///
/// For now this implements a thin dispatch layer. Each provider plugin
/// will eventually expose a WASM function `search(query, tab) -> [MediaEntry]`.
/// Until WASM integration lands (Phase 2), this module provides:
///   - The dispatch interface that the engine calls
///   - A built-in "demo" provider for testing without real plugins
use anyhow::Result;

use crate::ipc::{MediaEntry, MediaTab};
use crate::plugin::{ExecutionMode, LoadedPlugin};
use crate::sandbox::{call_wasm, SandboxCtx};

// ── Public interface ──────────────────────────────────────────────────────────

/// Search a single provider plugin and return matching entries.
pub async fn search(
    ctx: &SandboxCtx,
    plugin: &LoadedPlugin,
    query: &str,
    tab: &MediaTab,
) -> Result<Vec<MediaEntry>> {
    match &plugin.mode {
        ExecutionMode::Wasm => search_wasm(ctx, plugin, query, tab).await,
        ExecutionMode::NativeLib => search_native(ctx, plugin, query, tab).await,
        ExecutionMode::Grpc(addr) => search_grpc(ctx, addr, query, tab).await,
    }
}

// ── WASM dispatch ─────────────────────────────────────────────────────────────

async fn search_wasm(
    ctx: &SandboxCtx,
    plugin: &LoadedPlugin,
    query: &str,
    tab: &MediaTab,
) -> Result<Vec<MediaEntry>> {
    let input = serde_json::json!({
        "query": query,
        "tab":   tab,
    });

    let result = call_wasm(ctx, &plugin.entrypoint, "search", &input).await?;
    let entries: Vec<MediaEntry> = serde_json::from_value(result)?;
    Ok(entries)
}

// ── Native lib dispatch (stub) ────────────────────────────────────────────────

async fn search_native(
    ctx: &SandboxCtx,
    _plugin: &LoadedPlugin,
    _query: &str,
    _tab: &MediaTab,
) -> Result<Vec<MediaEntry>> {
    anyhow::bail!(
        "Native library plugins not yet supported (plugin: '{}')",
        ctx.plugin_name
    )
}

// ── gRPC dispatch (stub) ──────────────────────────────────────────────────────

async fn search_grpc(
    ctx: &SandboxCtx,
    addr: &str,
    _query: &str,
    _tab: &MediaTab,
) -> Result<Vec<MediaEntry>> {
    anyhow::bail!(
        "gRPC plugin dispatch not yet implemented for '{}' at {}",
        ctx.plugin_name,
        addr
    )
}
