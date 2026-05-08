//! Plugin registry — index fetching and installation.
//!
//! A plugin *registry* is an HTTP endpoint that serves a `plugins.json` index
//! file.  The built-in registry lives at `https://plugins.stui.dev`.
//! Users can add community registries in the plugin-repos settings screen.
//!
//! # Index format
//!
//! `{repo_url}/plugins.json` must return a JSON array of `RegistryEntry`
//! objects:
//!
//! ```json
//! [
//!   {
//!     "name":        "torrentio-rpc",
//!     "version":     "1.2.0",
//!     "type":        "rpc",
//!     "description": "Torrentio stream provider via JSON-RPC",
//!     "author":      "stui-team",
//!     "homepage":    "https://github.com/stui-org/torrentio-rpc",
//!     "binary_url":  "https://plugins.stui.dev/releases/torrentio-rpc-1.2.0.tar.gz",
//!     "checksum":    "sha256:abc123..."
//!   }
//! ]
//! ```
//!
//! # Install flow
//!
//! 1. Download `binary_url` bytes.
//! 2. Verify SHA-256 checksum (`sha256:<hex>`).
//! 3. Extract tar.gz into `~/.stui/plugins/<name>/`.
//! 4. The filesystem watcher in `discovery.rs` hot-reloads the new plugin.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

// ── Types ─────────────────────────────────────────────────────────────────────

/// One entry in a registry index (`plugins.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Unique plugin identifier, also used as the install directory name.
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Plugin execution type: `"rpc"` | `"wasm"` | `"stremio"`.
    #[serde(rename = "type")]
    pub plugin_type: String,
    /// One-line human-readable description.
    pub description: String,
    /// Plugin author (person or organisation).
    pub author: String,
    /// Optional homepage / source URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Canonical download URL for the plugin bundle (`.tar.gz`).
    pub binary_url: String,
    /// SHA-256 checksum in the form `"sha256:<lowercase-hex>"`.
    pub checksum: String,
}

// ── Index fetching ────────────────────────────────────────────────────────────

/// Fetch the plugin index from a single registry URL.
///
/// Appends `/plugins.json` to `repo_url` (trailing slashes are stripped).
/// Returns an empty Vec on non-200 or parse errors, with a tracing warning.
pub async fn fetch_index(repo_url: &str) -> Result<Vec<RegistryEntry>> {
    let url = format!("{}/plugins.json", repo_url.trim_end_matches('/'));
    info!(%url, "fetching plugin registry index");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        bail!("registry {} returned {}", url, resp.status());
    }

    let entries: Vec<RegistryEntry> = resp
        .json()
        .await
        .with_context(|| format!("parse plugins.json from {url}"))?;

    info!(%url, count = entries.len(), "registry index fetched");
    Ok(entries)
}

/// Fetch and merge the plugin index from every repository in `repos`.
///
/// Entries from repos listed first take precedence when plugin names collide
/// (the first occurrence wins).  The returned Vec is stable-sorted by name.
#[allow(dead_code)] // pub API: used by plugin registry loader
pub async fn fetch_merged_index(repos: &[String]) -> Vec<RegistryEntry> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut all: Vec<RegistryEntry> = Vec::new();

    for repo in repos {
        match fetch_index(repo).await {
            Ok(entries) => {
                for e in entries {
                    if seen.insert(e.name.clone()) {
                        all.push(e);
                    }
                }
            }
            Err(err) => {
                warn!(%repo, error = %err, "failed to fetch registry index — skipping");
            }
        }
    }

    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

// ── Installation ──────────────────────────────────────────────────────────────

/// Download, verify, and install a plugin from its registry entry.
///
/// The plugin is extracted to `{plugin_dir}/{entry.name}/`.
/// Returns the installed directory path on success.
///
/// The discovery hot-reload watcher picks up the new directory automatically;
/// no explicit rescan is needed.
pub async fn download_and_install(entry: &RegistryEntry, plugin_dir: &Path) -> Result<PathBuf> {
    info!(name = %entry.name, version = %entry.version, "installing plugin");

    // ── Download ──────────────────────────────────────────────────────────
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let bytes = client
        .get(&entry.binary_url)
        .send()
        .await
        .with_context(|| format!("download {}", entry.binary_url))?
        .error_for_status()
        .with_context(|| format!("download {} — HTTP error", entry.binary_url))?
        .bytes()
        .await
        .with_context(|| format!("read body from {}", entry.binary_url))?;

    info!(name = %entry.name, bytes = bytes.len(), "download complete");

    // ── Checksum verification ─────────────────────────────────────────────
    if let Some(expected_hex) = entry.checksum.strip_prefix("sha256:") {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual_hex = hex::encode(hasher.finalize());
        if actual_hex != expected_hex {
            bail!(
                "checksum mismatch for {}: expected sha256:{}, got sha256:{}",
                entry.name,
                expected_hex,
                actual_hex
            );
        }
        info!(name = %entry.name, "checksum verified");
    } else {
        warn!(
            name = %entry.name,
            checksum = %entry.checksum,
            "unrecognised checksum format — skipping verification"
        );
    }

    // ── Extraction ────────────────────────────────────────────────────────
    let dest = plugin_dir.join(&entry.name);
    tokio::fs::create_dir_all(&dest)
        .await
        .with_context(|| format!("create plugin dir {}", dest.display()))?;

    // Extraction is CPU-bound; run in a blocking thread pool.
    let bytes_clone = bytes.clone();
    let dest_clone = dest.clone();
    let name = entry.name.clone();

    tokio::task::spawn_blocking(move || extract_tgz(&bytes_clone, &dest_clone, &name))
        .await
        .with_context(|| "spawn_blocking for extraction panicked")?
        .with_context(|| format!("extract {}", entry.name))?;

    info!(name = %entry.name, dir = %dest.display(), "plugin installed");
    Ok(dest)
}

/// Extract a `.tar.gz` archive from `bytes` into `dest_dir`.
///
/// All entries are extracted relative to `dest_dir`.  Entries with
/// path components that would escape `dest_dir` are skipped (path traversal
/// guard).
fn extract_tgz(bytes: &[u8], dest_dir: &Path, plugin_name: &str) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::path::Component;
    use tar::Archive;

    let gz = GzDecoder::new(std::io::Cursor::new(bytes));
    let mut ar = Archive::new(gz);

    // Canonicalise dest_dir so we can detect path traversal attempts.
    // If dest_dir doesn't exist yet use its absolute path instead.
    let canonical_dest = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| dest_dir.to_path_buf());

    for entry in ar.entries().context("iterate tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let raw_path = entry.path().context("entry path")?.into_owned();

        // Strip a leading component that is a normal dir segment or a CurDir —
        // common in tarballs produced by `tar czf name-ver.tar.gz name-ver/`.
        let rel: PathBuf = {
            let mut components = raw_path.components();
            match components.next() {
                Some(Component::Normal(_)) | Some(Component::CurDir) => {
                    let rest: PathBuf = components.collect();
                    if rest.as_os_str().is_empty() {
                        raw_path.clone()
                    } else {
                        rest
                    }
                }
                _ => raw_path.clone(),
            }
        };

        // Path-traversal guard: resolve the target, ensure it stays under dest_dir.
        let target = dest_dir.join(&rel);

        // Check the parent directory (which may or may not exist yet).
        let parent_ok = match target.parent() {
            None => false,
            Some(parent) => {
                // Try to canonicalise; fall back to a prefix check on the raw path.
                if let Ok(canon_parent) = parent.canonicalize() {
                    canon_parent.starts_with(&canonical_dest)
                } else {
                    // Parent doesn't exist yet — check by raw path normalization.
                    target.starts_with(dest_dir)
                }
            }
        };

        if !parent_ok {
            warn!(
                plugin = %plugin_name,
                path   = %rel.display(),
                "skipping path-traversal entry"
            );
            continue;
        }

        // Create parent directories as needed, then unpack.
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dirs for {}", parent.display()))?;
        }
        entry
            .unpack(&target)
            .with_context(|| format!("unpack {} to {}", rel.display(), target.display()))?;
    }

    Ok(())
}
