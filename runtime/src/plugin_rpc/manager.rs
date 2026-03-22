//! External plugin manager — discovery, spawn, capability routing, and dispatch.
//!
//! `PluginRpcManager` is the single runtime object that owns all external
//! (out-of-process) plugins.  It handles:
//!
//! 1. **Discovery**: scans a directory for plugin executables (`plugin.json`
//!    manifest or executable + capability handshake).
//! 2. **Spawning**: starts each plugin process and waits for the handshake.
//! 3. **Routing**: dispatches search/stream/subtitle calls to all plugins
//!    that advertise the relevant capability, then merges results.
//! 4. **Lifecycle**: gracefully shuts plugins down on exit.
//!
//! # Plugin directory layout
//!
//! ```text
//! $HOME/.stui/plugins/
//!   torrentio/
//!     plugin           - executable (any language)
//!     plugin.json      - optional static manifest (name, version, capabilities)
//!   opensubtitles/
//!     plugin.py        - Python plugin (needs python3 in PATH)
//!     plugin.json
//! ```
//!
//! If `plugin.json` is absent the runtime still works — capabilities are
//! learned from the handshake response.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::supervisor::{PluginSupervisor, SupervisorConfig};
use super::protocol::{RpcMediaItem, RpcStream};
use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, SubtitleTrack};
use crate::providers::Stream;

/// Manages all external plugin processes for the runtime.
#[allow(dead_code)]
pub struct PluginRpcManager {
    plugins: Arc<RwLock<Vec<Arc<PluginSupervisor>>>>,
    config:  SupervisorConfig,
}

impl PluginRpcManager {
    pub fn new() -> Self {
        PluginRpcManager {
            plugins: Arc::new(RwLock::new(vec![])),
            config:  SupervisorConfig::default(),
        }
    }

    /// Create with custom supervisor config (e.g. different memory limit).
    pub fn with_config(config: SupervisorConfig) -> Self {
        PluginRpcManager {
            plugins: Arc::new(RwLock::new(vec![])),
            config,
        }
    }

    // ── Discovery & loading ───────────────────────────────────────────────

    /// Scan `plugin_dir` for external plugins and spawn each one.
    ///
    /// Each subdirectory is checked for an executable named `plugin`,
    /// `plugin.py`, `plugin.js`, `plugin.rb`, or any file matching the
    /// executable bit.  The first match is spawned.
    pub async fn discover_and_load(&self, plugin_dir: &Path) {
        let Ok(mut entries) = tokio::fs::read_dir(plugin_dir).await else {
            warn!(path = %plugin_dir.display(), "plugin directory not found or not accessible");
            return;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() { continue; }

            match self.load_from_dir(&path).await {
                Ok(sup) => {
                    let info = sup.info.read().await;
                    info!(
                        plugin = %info.name,
                        caps   = ?info.capabilities,
                        "external plugin loaded (supervised)"
                    );
                    drop(info);
                    self.plugins.write().await.push(Arc::new(sup));
                }
                Err(e) => {
                    warn!(path = %path.display(), err = %e, "failed to load RPC plugin");
                }
            }
        }
    }

    /// Load a single plugin from a directory, wrapping it in a supervisor.
    async fn load_from_dir(&self, dir: &Path) -> Result<PluginSupervisor> {
        let bin = Self::find_executable(dir)?;
        PluginSupervisor::spawn(bin, self.config.clone()).await
    }

    /// Find the plugin executable inside a directory.
    ///
    /// Checks in order: `plugin`, `plugin.py`, `plugin.js`, `plugin.ts`,
    /// `plugin.rb`, `plugin.sh`, then any executable file.
    fn find_executable(dir: &Path) -> Result<PathBuf> {
        let candidates = [
            "plugin",
            "plugin.py",
            "plugin.js",
            "plugin.ts",
            "plugin.rb",
            "plugin.sh",
            "plugin.go",
        ];
        for name in &candidates {
            let p = dir.join(name);
            if p.exists() {
                return Ok(p);
            }
        }
        // Fall back: first file with executable bit
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(rd) = std::fs::read_dir(dir) {
                for entry in rd.flatten() {
                    let Ok(meta) = entry.metadata() else { continue; };
                    if meta.permissions().mode() & 0o111 != 0 && meta.is_file() {
                        return Ok(entry.path());
                    }
                }
            }
        }
        anyhow::bail!("no executable found in {}", dir.display())
    }

    // ── Capability-based dispatch ─────────────────────────────────────────

    /// Fan out a search query to all plugins with the `catalog` capability.
    /// Results are merged and returned in arrival order.
    pub async fn search(
        &self,
        tab: &MediaTab,
        query: &str,
        page: u32,
    ) -> Vec<CatalogEntry> {
        let tab_str = format!("{tab:?}").to_lowercase();
        let plugins  = self.plugins_with_cap("catalog").await;
        let mut results = vec![];

        let handles: Vec<_> = plugins.iter().map(|p| {
            let p = Arc::clone(p);
            let q = query.to_string();
            let t = tab_str.clone();
            tokio::spawn(async move { p.catalog_search(&q, &t, page).await })
        }).collect();

        for handle in handles {
            match handle.await {
                Ok(Ok(items)) => {
                    for item in items {
                        results.push(rpc_item_to_catalog(item, tab));
                    }
                }
                Ok(Err(e)) => warn!("rpc search error: {e}"),
                Err(e)     => warn!("rpc search task panicked: {e}"),
            }
        }

        results
    }

    /// Fan out stream resolution to all plugins with the `streams` capability.
    pub async fn resolve_streams(&self, id: &str) -> Vec<Stream> {
        let plugins = self.plugins_with_cap("streams").await;
        let mut results = vec![];

        let handles: Vec<_> = plugins.iter().map(|p| {
            let p  = Arc::clone(p);
            let id = id.to_string();
            tokio::spawn(async move { p.streams_resolve(&id).await })
        }).collect();

        for handle in handles {
            match handle.await {
                Ok(Ok(streams)) => {
                    for s in streams {
                        results.push(rpc_stream_to_stream(s));
                    }
                }
                Ok(Err(e)) => warn!("rpc streams error: {e}"),
                Err(e)     => warn!("rpc streams task panicked: {e}"),
            }
        }

        results
    }

    /// Fan out subtitle fetching to all plugins with the `subtitles` capability.
    pub async fn fetch_subtitles(&self, id: &str) -> Vec<SubtitleTrack> {
        let plugins = self.plugins_with_cap("subtitles").await;
        let mut results = vec![];

        let handles: Vec<_> = plugins.iter().map(|p| {
            let p  = Arc::clone(p);
            let id = id.to_string();
            tokio::spawn(async move { p.subtitles_fetch(&id).await })
        }).collect();

        for handle in handles {
            match handle.await {
                Ok(Ok(tracks)) => {
                    for t in tracks {
                        results.push(SubtitleTrack {
                            language: t.language,
                            url:      t.url,
                            format:   t.format,
                        });
                    }
                }
                Ok(Err(e)) => warn!("rpc subtitles error: {e}"),
                Err(e)     => warn!("rpc subtitles task panicked: {e}"),
            }
        }

        results
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Gracefully shut down all supervised plugin processes.
    pub async fn shutdown_all(&self) {
        let plugins = self.plugins.read().await;
        for p in plugins.iter() {
            p.shutdown().await;
        }
        info!("all external plugins shut down");
    }

    /// Number of currently loaded external plugins.
    pub async fn len(&self) -> usize {
        self.plugins.read().await.len()
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    async fn plugins_with_cap(&self, cap: &str) -> Vec<Arc<PluginSupervisor>> {
        let mut result = vec![];
        for p in self.plugins.read().await.iter() {
            if !p.is_failed() && p.has_capability(cap).await {
                result.push(Arc::clone(p));
            }
        }
        result
    }
}

impl Default for PluginRpcManager {
    fn default() -> Self { Self::new() }
}

// ── Type conversions ──────────────────────────────────────────────────────────

#[allow(dead_code)]
fn rpc_item_to_catalog(item: RpcMediaItem, tab: &MediaTab) -> CatalogEntry {
    use crate::ipc::MediaType;
    CatalogEntry {
        id:          item.id,
        title:       item.title,
        year:        item.year,
        genre:       item.genre,
        rating:      item.rating,
        description: item.description,
        poster_url:  item.poster_url,
        poster_art:  None,
        provider:    "rpc-plugin".to_string(),
        tab:         format!("{tab:?}").to_lowercase(),
        imdb_id:     None,
        tmdb_id:     None,
        media_type:  MediaType::Movie,
        ratings:     std::collections::HashMap::new(),
    }
}

#[allow(dead_code)]
fn rpc_stream_to_stream(s: RpcStream) -> Stream {
    use crate::providers::StreamQuality;

    let quality = s.quality.as_deref()
        .map(StreamQuality::from_label)
        .unwrap_or(StreamQuality::Unknown);

    let codec = s.codec.clone();
    let hdr   = s.hdr.clone().unwrap_or_default();

    let protocol = if s.url.starts_with("magnet:") {
        Some("magnet".to_string())
    } else if s.url.starts_with("https://") {
        Some("https".to_string())
    } else if s.url.starts_with("http://") {
        Some("http".to_string())
    } else {
        None
    };

    Stream {
        id:             s.url.clone(),
        name:           s.name,
        url:            s.url,
        mime:           None,
        quality,
        provider:       "rpc-plugin".to_string(),
        protocol,
        seeders:        s.seeders,
        bitrate_kbps:   s.bitrate_kbps,
        codec,
        resolution:     s.resolution,
        hdr,
        size_bytes:     s.size_bytes,
        latency_ms:     None,
        speed_mbps:     None,
        audio_channels: s.audio_channels,
        language:       s.language,
    }
}
