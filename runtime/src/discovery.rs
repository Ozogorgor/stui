//! Plugin discovery — finds and hot-reloads plugins from `~/.stui/plugins/`.
//!
//! ## Startup scan
//! On start, `Discovery::scan_and_load()` walks the plugin directory,
//! validates each subdirectory's `plugin.toml`, and loads valid plugins
//! into the engine.
//!
//! ## Hot reload (filesystem watcher)
//! `Discovery::watch()` spawns a background task using the `notify` crate.
//! When a new directory appears under the plugin root (i.e. the user drops
//! in a new plugin), the watcher:
//!   1. Waits for a quiescence period (500ms) to let file writes settle
//!   2. Validates the new plugin manifest
//!   3. Loads it into the engine
//!   4. Sends a `PluginToastMsg` to the Go TUI via the broadcast channel
//!
//! ## Directory convention
//! Each plugin lives in its own subdirectory:
//! ```text
//! $HOME/.stui/plugins/
//!   my-provider/
//!     plugin.toml
//!     plugin.wasm
//!   another-plugin/
//!     plugin.toml
//!     plugin.wasm
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::engine::Engine;
use crate::plugin::load_manifest;

// ── Toast notification ────────────────────────────────────────────────────────

/// Sent over the broadcast channel when a plugin is hot-loaded.
/// The IPC layer forwards this to Go as a `plugin_toast` message.
#[derive(Debug, Clone)]
pub struct PluginToast {
    pub plugin_name: String,
    pub version: String,
    pub plugin_type: String,
    pub message: String,
    pub is_error: bool,
}

// ── Discovery ─────────────────────────────────────────────────────────────────

pub struct Discovery {
    plugin_dir: PathBuf,
    engine: Arc<Engine>,
    toast_tx: broadcast::Sender<PluginToast>,
}

impl Discovery {
    pub fn new(
        plugin_dir: PathBuf,
        engine: Arc<Engine>,
        toast_tx: broadcast::Sender<PluginToast>,
    ) -> Self {
        Self { plugin_dir, engine, toast_tx }
    }

    /// Subscribe to plugin toast notifications.
    #[allow(dead_code)] // planned: will be called by TUI plugin discovery panel
    pub fn subscribe(&self) -> broadcast::Receiver<PluginToast> {
        self.toast_tx.subscribe()
    }

    /// Scan the plugin directory and load all valid plugins.
    /// Called once at startup, before the IPC loop starts.
    pub async fn scan_and_load(&self) -> Result<usize> {
        if !self.plugin_dir.exists() {
            std::fs::create_dir_all(&self.plugin_dir)?;
            info!(dir = %self.plugin_dir.display(), "created plugin directory");
            return Ok(0);
        }

        let dirs = self.collect_plugin_dirs()?;
        let total = dirs.len();
        let mut loaded = 0;

        info!(dir = %self.plugin_dir.display(), candidates = total, "scanning plugins");

        for dir in dirs {
            match self.load_one(&dir).await {
                Ok(name) => {
                    info!(plugin = %name, "loaded on startup");
                    loaded += 1;
                }
                Err(e) => {
                    warn!(dir = %dir.display(), error = %e, "skipping invalid plugin");
                }
            }
        }

        info!(loaded, total, "plugin scan complete");
        Ok(loaded)
    }

    /// Start the filesystem watcher in a background Tokio task.
    /// Returns immediately; the watcher runs until the process exits.
    pub fn start_watcher(self: Arc<Self>) {
        tokio::spawn(async move {
            if let Err(e) = self.watch_loop().await {
                error!("plugin watcher crashed: {e}");
            }
        });
    }

    // ── Internal ─────────────────────────────────────────────────────────

    fn collect_plugin_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = vec![];
        for entry in std::fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
        Ok(dirs)
    }

    async fn load_one(&self, dir: &Path) -> Result<String> {
        let manifest = load_manifest(dir)?;
        let name = manifest.plugin.name.clone();
        let _version = manifest.plugin.version.clone();
        let _ptype = manifest.plugin.plugin_type.to_string();

        // Engine.load_plugin does full validation + sandbox setup
        // We use the engine's existing method directly
        let engine = Arc::clone(&self.engine);
        let dir_owned = dir.to_path_buf();
        engine.load_plugin(&dir_owned).await?;

        Ok(name)
    }

    async fn watch_loop(&self) -> Result<()> {
        // notify requires a sync callback; we bridge to async via an mpsc channel
        let (tx, mut rx) = tokio::sync::mpsc::channel::<notify::Result<Event>>(64);

        let mut watcher: RecommendedWatcher = notify::recommended_watcher(
            move |res: notify::Result<Event>| {
                let _ = tx.blocking_send(res);
            }
        )?;

        watcher.watch(&self.plugin_dir, RecursiveMode::NonRecursive)?;
        info!(dir = %self.plugin_dir.display(), "plugin hot-reload watcher active");

        // Track which dirs we've already processed to avoid duplicate loads
        let mut seen: HashSet<PathBuf> = self.collect_plugin_dirs()
            .unwrap_or_default()
            .into_iter()
            .collect();

        while let Some(event) = rx.recv().await {
            match event {
                Ok(ev) => self.handle_fs_event(ev, &mut seen).await,
                Err(e) => warn!("watcher error: {e}"),
            }
        }
        Ok(())
    }

    async fn handle_fs_event(&self, event: Event, seen: &mut HashSet<PathBuf>) {
        // We only care about new directories appearing (plugin drops)
        let is_create = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
        );
        if !is_create {
            return;
        }

        for path in event.paths {
            // Only process directories we haven't seen yet
            if !path.is_dir() || seen.contains(&path) {
                continue;
            }

            // Wait for file writes to settle (avoids reading a half-written plugin)
            debug!(path = %path.display(), "new plugin dir detected — waiting for writes to settle");
            tokio::time::sleep(Duration::from_millis(500)).await;

            seen.insert(path.clone());
            self.try_hot_load(&path).await;
        }
    }

    async fn try_hot_load(&self, dir: &Path) {
        // Validate manifest first
        let manifest = match load_manifest(dir) {
            Ok(m) => m,
            Err(e) => {
                warn!(dir = %dir.display(), error = %e, "hot-load failed: invalid manifest");
                let _ = self.toast_tx.send(PluginToast {
                    plugin_name: dir.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    version: "?".into(),
                    plugin_type: "?".into(),
                    message: format!("Invalid plugin manifest: {e}"),
                    is_error: true,
                });
                return;
            }
        };

        let name = manifest.plugin.name.clone();
        let version = manifest.plugin.version.clone();
        let ptype = manifest.plugin.plugin_type.to_string();

        match self.engine.load_plugin(dir).await {
            Ok(_) => {
                info!(plugin = %name, version = %version, "hot-loaded new plugin");
                let _ = self.toast_tx.send(PluginToast {
                    plugin_name: name.clone(),
                    version,
                    plugin_type: ptype,
                    message: format!("Plugin '{name}' loaded"),
                    is_error: false,
                });
            }
            Err(e) => {
                error!(plugin = %name, error = %e, "hot-load failed");
                let _ = self.toast_tx.send(PluginToast {
                    plugin_name: name.clone(),
                    version,
                    plugin_type: ptype,
                    message: format!("Failed to load '{name}': {e}"),
                    is_error: true,
                });
            }
        }
    }
}
