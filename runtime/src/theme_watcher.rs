// theme_watcher.rs — watches matugen's colors.json and pushes theme_update
// messages to the Go TUI whenever the file changes.
//
// Matugen rewrites colors.json atomically (rename-into-place) every time the
// wallpaper changes. The notify watcher catches both direct writes and the
// rename event, debounces for 150 ms, then reads and parses the file.
//
// Wire message emitted on change:
//   {"type":"theme_update","colors":{"primary":"#adc6ff","background":"#1b1b1f",...},"mode":"dark"}
//
// JSON structure produced by `matugen image <img> --json hex`:
//   {
//     "colors": {
//       "dark":  { "background": "#1b1b1f", "primary": "#adc6ff", ... },
//       "light": { "background": "#fffbff", "primary": "#005ac1", ... },
//       "amoled": { ... }
//     }
//   }
//
// The watcher reads the "dark" sub-object and emits it as-is.
// The Go side maps M3 role names → stui semantic colors via theme.FromMatugen().
//
// Default watch path: ~/.config/matugen/colors.json
// Override: STUI_MATUGEN_COLORS env var, or config key `matugen_colors_path`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

/// Message sent over the broadcast channel to the IPC loop.
#[derive(Clone, Debug)]
pub struct ThemeColors {
    pub colors: HashMap<String, String>,
    pub mode: String,
}

/// Resolve the path to matugen's colors.json.
/// Priority: STUI_MATUGEN_COLORS env var → ~/.config/matugen/colors.json
pub fn resolve_colors_path() -> PathBuf {
    if let Ok(p) = std::env::var("STUI_MATUGEN_COLORS") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".config/matugen/colors.json")
}

/// Parse matugen's JSON output and extract the "dark" color map.
/// Returns None if the file cannot be read or parsed.
pub fn parse_colors_json(path: &Path, mode: &str) -> Option<HashMap<String, String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&content).ok()?;

    // New format: { "colors": { "dark": { "primary": "#adc6ff", ... } } }
    if let Some(scheme) = root.get("colors").and_then(|c| c.get(mode)) {
        if let Some(obj) = scheme.as_object() {
            let map: HashMap<String, String> = obj
                .iter()
                .filter_map(|(k, v)| {
                    v.as_str().map(|s| (k.clone(), s.to_string()))
                })
                .collect();
            if !map.is_empty() {
                return Some(map);
            }
        }
    }

    // Flat format (older matugen or custom templates):
    // { "primary": "#adc6ff", "background": "#1b1b1f", ... }
    if let Some(obj) = root.as_object() {
        let map: HashMap<String, String> = obj
            .iter()
            .filter_map(|(k, v)| {
                v.as_str().map(|s| (k.clone(), s.to_string()))
            })
            .collect();
        if !map.is_empty() {
            return Some(map);
        }
    }

    None
}

/// Spawn the background watcher task.
/// Returns a broadcast::Receiver that yields ThemeColors whenever the file changes.
/// This function is non-blocking — the watcher runs in a dedicated OS thread via
/// notify, forwarding events into a tokio channel.
pub fn start_watcher(
    colors_path: PathBuf,
    mode: String,
) -> broadcast::Receiver<ThemeColors> {
    let (tx, rx) = broadcast::channel::<ThemeColors>(4);

    // Clone for the background thread
    let path_clone = colors_path.clone();
    let mode_clone = mode.clone();
    let tx_clone = tx.clone();

    std::thread::spawn(move || {
        // Read and emit the current colors immediately on startup, so the TUI
        // gets the right colors even before the next wallpaper change.
        if let Some(colors) = parse_colors_json(&path_clone, &mode_clone) {
            let _ = tx_clone.send(ThemeColors {
                colors,
                mode: mode_clone.clone(),
            });
        }

        let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();

        // notify watcher — uses inotify on Linux, kqueue on macOS
        let mut watcher = match RecommendedWatcher::new(
            event_tx,
            Config::default().with_poll_interval(Duration::from_millis(100)),
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[theme_watcher] failed to create watcher: {e}");
                return;
            }
        };

        // Watch the parent directory so we catch rename-into-place atomics.
        // Matugen writes to a temp file then renames — watching only the file
        // itself would miss that on some OS/fs combinations.
        let watch_dir = path_clone
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            eprintln!("[theme_watcher] failed to watch {}: {e}", watch_dir.display());
            return;
        }

        eprintln!(
            "[theme_watcher] watching {} for matugen updates (mode: {})",
            path_clone.display(),
            mode_clone
        );

        // Debounce: only re-read after 150 ms of quiet
        let debounce = Duration::from_millis(150);
        let mut pending = false;
        let mut deadline = std::time::Instant::now();

        loop {
            // Block with timeout so we can fire the debounced read
            let timeout = if pending {
                deadline.saturating_duration_since(std::time::Instant::now())
            } else {
                Duration::from_secs(60)
            };

            match event_rx.recv_timeout(timeout) {
                Ok(Ok(event)) => {
                    // Only care about events that touch our specific file
                    let relevant = event.paths.iter().any(|p| p == &path_clone)
                        || matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(ModifyKind::Data(_))
                        );

                    if relevant {
                        pending = true;
                        deadline = std::time::Instant::now() + debounce;
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("[theme_watcher] notify error: {e}");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Debounce timer fired — do the read
                    if pending {
                        pending = false;
                        if let Some(colors) =
                            parse_colors_json(&path_clone, &mode_clone)
                        {
                            eprintln!(
                                "[theme_watcher] colors.json changed — sending {} colors",
                                colors.len()
                            );
                            let _ = tx_clone.send(ThemeColors {
                                colors,
                                mode: mode_clone.clone(),
                            });
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    rx
}

// ── Wire serialisation ────────────────────────────────────────────────────────

/// JSON shape sent to the Go TUI over IPC stdout.
#[derive(Serialize)]
struct ThemeUpdateWire<'a> {
    r#type: &'static str,
    colors: &'a HashMap<String, String>,
    mode: &'a str,
}

/// Serialise a ThemeColors into a single NDJSON line and write to stdout.
pub fn emit_theme_update<W: Write>(out: &mut W, tc: &ThemeColors) -> std::io::Result<()> {
    let wire = ThemeUpdateWire {
        r#type: "theme_update",
        colors: &tc.colors,
        mode: &tc.mode,
    };
    let line = serde_json::to_string(&wire)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    writeln!(out, "{}", line)
}
