//! Plugin loader — parses `plugin.toml`, validates, resolves the entrypoint,
//! and (future) instantiates the WASM module. Config resolution and `init()`
//! flow through here once the ABI chunk lands.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use thiserror::Error;

use super::manifest::{self, ManifestValidationError, PluginManifest, PluginMetaExt};

// ── LoadedPlugin ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)] // pub API: used by engine and registry
pub struct LoadedPlugin {
    pub id: String, // uuid assigned at load time
    pub manifest: PluginManifest,
    pub dir: PathBuf,        // directory containing plugin.toml
    pub entrypoint: PathBuf, // resolved absolute path to .wasm / .so / rpc binary
    pub mode: ExecutionMode,
    /// When `false`, capability dispatch (`find_by_capability`,
    /// `find_providers_for_tab`, `find_stream_providers`,
    /// `find_subtitle_providers`) skips this plugin. Supervisor +
    /// sandbox stay alive so toggling back to `true` is O(1) — no
    /// reload, no re-download. Defaults to `true` on load.
    pub enabled: bool,
}

impl LoadedPlugin {
    /// Check whether this plugin advertises a specific capability.
    ///
    /// Two information sources, in precedence order:
    ///   1. The structured `[capabilities]` table in the manifest — the
    ///      authoritative declaration in modern plugins (e.g.
    ///      `streams = true`, `catalog.search = true`).
    ///   2. The legacy `[plugin] type = "..."` field, for older manifests
    ///      that predate the structured table.
    ///
    /// New plugins (jackett, prowlarr, …) omit `[plugin] type` entirely
    /// and rely solely on `[capabilities]`. Without (1) those plugins look
    /// like default `MetadataProvider`s and never appear in
    /// `find_stream_providers`, breaking the streams column.
    pub fn has_capability(&self, cap: super::manifest::PluginCapability) -> bool {
        use super::manifest::PluginCapability;
        use stui_plugin_sdk::CatalogCapability;
        let caps = &self.manifest.capabilities;
        let catalog_declared = match &caps.catalog {
            CatalogCapability::Enabled(b) => *b,
            CatalogCapability::Typed { search, kinds, .. } => {
                search.unwrap_or(false) || !kinds.is_empty()
            }
        };
        match cap {
            // Streams: governed entirely by the [capabilities] table.
            // (Resolver-typed plugins without `streams = true` still
            // need Streams routing — handled below by the type fallback.)
            PluginCapability::Streams if caps.streams => return true,

            // Catalog: explicit declaration wins.
            PluginCapability::Catalog if catalog_declared => return true,

            // Stream-providers (`streams = true`) must NOT auto-inherit
            // Catalog from the plugin-type default. New manifests omit
            // `[plugin] type`, defaulting it to MetadataProvider, whose
            // capability set includes Catalog. That silently re-granted
            // Catalog to every stream-only provider regardless of
            // `[capabilities.catalog]`, so Jackett/Prowlarr dumped raw
            // torrent file names into the trending fan-out even with
            // `kinds = []` `search = false`.
            PluginCapability::Catalog if caps.streams => return false,

            _ => {}
        }
        // Legacy fallback: pure-metadata plugins that don't declare
        // `[capabilities.catalog]` rely on `[plugin] type = "metadata-
        // provider"` (or the MetadataProvider default) to expose
        // Catalog. Subtitles / Auth / Index / Resolver-style Streams
        // also derive from the plugin type since the [capabilities]
        // table doesn't model them.
        self.manifest
            .plugin
            .plugin_type_or_default()
            .capabilities()
            .contains(&cap)
    }

    #[allow(dead_code)] // pub API: used by engine and registry
    /// All capabilities this plugin advertises. Union of the structured
    /// `[capabilities]` table and the legacy `[plugin] type` mapping so
    /// list_plugins exposes a complete view regardless of manifest style.
    pub fn capabilities(&self) -> Vec<super::manifest::PluginCapability> {
        use super::manifest::PluginCapability;
        use stui_plugin_sdk::CatalogCapability;
        let mut out = self.manifest.plugin.plugin_type_or_default().capabilities();
        let caps = &self.manifest.capabilities;
        if caps.streams && !out.contains(&PluginCapability::Streams) {
            out.push(PluginCapability::Streams);
        }
        let catalog_declared = match &caps.catalog {
            CatalogCapability::Enabled(b) => *b,
            CatalogCapability::Typed { search, kinds, .. } => {
                search.unwrap_or(false) || !kinds.is_empty()
            }
        };
        if catalog_declared && !out.contains(&PluginCapability::Catalog) {
            out.push(PluginCapability::Catalog);
        }
        out
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
        enabled: true,
    })
}
