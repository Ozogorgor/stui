//! Plugin loader — parses `plugin.toml`, validates, resolves the entrypoint,
//! and (future) instantiates the WASM module. Config resolution and `init()`
//! flow through here once the ABI chunk lands.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use thiserror::Error;

use super::manifest::{self, ManifestValidationError, PluginManifest};

// ── LoadedPlugin ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)] // pub API: used by engine and registry
pub struct LoadedPlugin {
    pub id: String, // uuid assigned at load time
    pub manifest: PluginManifest,
    pub dir: PathBuf,        // directory containing plugin.toml
    pub entrypoint: PathBuf, // resolved absolute path to .wasm / .so / rpc binary
    pub mode: ExecutionMode,
}

impl LoadedPlugin {
    /// Check whether this plugin advertises a specific capability.
    ///
    /// Use this for all capability-based dispatch instead of matching
    /// on `PluginType` directly — it handles legacy type aliases correctly.
    pub fn has_capability(&self, cap: super::manifest::PluginCapability) -> bool {
        self.manifest
            .plugin
            .plugin_type_or_default()
            .capabilities()
            .contains(&cap)
    }

    #[allow(dead_code)] // pub API: used by engine and registry
    /// All capabilities this plugin advertises.
    pub fn capabilities(&self) -> Vec<super::manifest::PluginCapability> {
        self.manifest.plugin.plugin_type_or_default().capabilities()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionMode {
    Wasm,
    NativeLib,
    Grpc(String), // address
}

// ── LoaderError ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("manifest parse error: {0}")]
    Parse(#[from] anyhow::Error),

    #[error("manifest validation: {0}")]
    Validation(#[from] ManifestValidationError),

    #[error("unknown entrypoint format: {0}")]
    UnknownEntrypoint(String),
}

// ── load_manifest (legacy, kept for callers that don't need validation) ───────

/// Load a plugin manifest without running strict validation.
///
/// This is the legacy entry point called by `engine::load_plugin` and the
/// filesystem watcher in `discovery`. Strict validation will be added in
/// Task 1.8+ once all real plugin.toml files pass the canonical schema.
pub fn load_manifest(plugin_dir: &Path) -> Result<PluginManifest> {
    let manifest_path = plugin_dir.join("plugin.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: PluginManifest = toml::from_str(&raw).with_context(|| "parsing plugin.toml")?;
    Ok(manifest)
}

/// Parse + strictly validate a manifest. Used by `load_from_dir` and by any
/// caller that wants the new canonical-schema enforcement.
pub fn parse_manifest(plugin_dir: &Path) -> Result<PluginManifest, LoaderError> {
    let manifest = load_manifest(plugin_dir)?;
    manifest::validate(&manifest)?;
    Ok(manifest)
}

// ── Entrypoint resolution ─────────────────────────────────────────────────────

/// Resolve the execution mode and entrypoint path for a loaded manifest.
pub fn resolve_entrypoint(
    plugin_dir: &Path,
    manifest: &PluginManifest,
) -> Result<(ExecutionMode, PathBuf)> {
    let entry = &manifest.plugin.entrypoint;

    // gRPC: entrypoint looks like "grpc://host:port"
    if entry.starts_with("grpc://") {
        return Ok((ExecutionMode::Grpc(entry.clone()), PathBuf::from(entry)));
    }

    let abs = plugin_dir.join(entry);

    let mode = if entry.ends_with(".wasm") {
        ExecutionMode::Wasm
    } else if entry.ends_with(".so") || entry.ends_with(".dylib") || entry.ends_with(".dll") {
        ExecutionMode::NativeLib
    } else {
        anyhow::bail!("Unknown entrypoint format: {}", entry);
    };

    Ok((mode, abs))
}

// ── load_from_dir ─────────────────────────────────────────────────────────────

/// Full plugin load from a directory: parse → validate → resolve entrypoint.
///
/// WASM instantiation / `init()` call wiring is handled by the engine today;
/// this function owns the parsing + validation step so the engine can be
/// retrofitted to call it once the validator is turned on for all plugins.
pub fn load_from_dir(dir: &Path) -> Result<LoadedPlugin, LoaderError> {
    let manifest = parse_manifest(dir)?;
    let (mode, entrypoint) = resolve_entrypoint(dir, &manifest).map_err(LoaderError::Parse)?;
    Ok(LoadedPlugin {
        id: uuid::Uuid::new_v4().to_string(),
        manifest,
        dir: dir.to_path_buf(),
        entrypoint,
        mode,
    })
}
