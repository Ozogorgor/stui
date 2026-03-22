//! stui-runtime — async Rust backend for the stui TUI.
//!
//! Startup sequence:
//!   1. Scan ~/.stui/plugins/ and load all valid plugins
//!   2. Start the filesystem watcher for hot-reload
//!   3. Start the catalog (cache-first grid population)
//!   4. Enter the IPC request/response loop

mod abi;
mod aria2_bridge;
mod cache;
mod catalog;
mod catalog_engine;
mod config;
mod discovery;
mod engine;
mod error;
mod events;
mod indexer;
mod ipc;
mod logging;
mod media;
mod mpd_bridge;
mod player;
mod plugin;
mod providers;
mod quality;
mod resolver;
mod sandbox;
mod scraper;
mod stremio;
mod streamer;
mod pipeline;
mod plugin_rpc;
mod registry;
mod skipper;
mod watchhistory;
mod mediacache;
mod ipc_batcher;
pub mod storage;
mod auth;


use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use catalog::Catalog;
use discovery::{Discovery, PluginToast};
use engine::{Engine, TraceEmitter};
use config::ConfigManager;
use events::EventBus;
use ipc::{ErrorCode, GridUpdateMsg, Request, Response};
use mpd_bridge::MpdBridge;
use providers::{HealthRegistry, StreamBenchmarker};
use skipper::{Skipper, SkipperStore};
use storage::aria2_translator::Aria2Translator;

// ── Config ────────────────────────────────────────────────────────────────────
// Configuration is now loaded via config::load() which reads
// ~/.stui/config/stui.toml and applies STUI_* env-var overrides on top.
// The RuntimeConfig struct lives in runtime/src/config/types.rs.

// ── Entry point ───────────────────────────────────────────────────────────────

// ── Tokio runtime ─────────────────────────────────────────────────────────────
// Uses the multi-thread scheduler (one OS thread per logical CPU by default).
// Override the number of worker threads with the TOKIO_WORKER_THREADS env var.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // Logging is initialised after config load so the log level comes from
    // stui.toml (with STUI_LOG env var override). We use a temporary default
    // here and re-initialise below once config is loaded.
    logging::init_with_level("info");

    // ── Mode detection ────────────────────────────────────────────────────
    // `stui-runtime`            → stdin/stdout mode (launched by TUI process)
    // `stui-runtime daemon`     → Unix socket daemon mode
    // `stui-runtime daemon --socket /path/to/sock`
    let args: Vec<String> = std::env::args().collect();
    let daemon_mode = args.get(1).map(|a| a == "daemon").unwrap_or(false);
    let socket_path = args.windows(2)
        .find(|w| w[0] == "--socket")
        .and_then(|w| w.get(1))
        .map(|s| std::path::PathBuf::from(s))
        .unwrap_or_else(default_socket_path);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = if daemon_mode { "daemon" } else { "inline" },
        "stui-runtime starting"
    );

    // Load config from ~/.stui/config/stui.toml + STUI_* env overrides
    let cfg = config::load();
    // Re-init logging now that we have the configured level
    logging::init(&cfg.logging);
    std::fs::create_dir_all(&cfg.cache_dir)?;
    std::fs::create_dir_all(&cfg.data_dir)?;
    std::fs::create_dir_all(&cfg.plugin_dir)?;

    // ── Engine ────────────────────────────────────────────────────────────
    let engine = Arc::new(Engine::new(cfg.cache_dir.clone(), cfg.data_dir.clone()));

    // ── Watch history ──────────────────────────────────────────────────────
    let watch_history = Arc::new(watchhistory::WatchHistoryStore::new(
        watchhistory::default_history_path(),
    ));
    info!(path = %watchhistory::default_history_path().display(), "watch history loaded");

    // ── Media cache ─────────────────────────────────────────────────────────
    let media_cache = Arc::new(mediacache::MediaCacheStore::new(
        mediacache::default_cache_path(),
    ).await);
    info!(path = %mediacache::default_cache_path().display(), "media cache loaded");

    // ── Media storage ──────────────────────────────────────────────────────
    let storage = Arc::new(storage::MediaStorage::new(
        cfg.storage.movies.clone(),
        cfg.storage.series.clone(),
        cfg.storage.anime.clone(),
        cfg.storage.music.clone(),
        cfg.storage.podcasts.clone(),
    ));
    info!("media storage initialized: movies={}", cfg.storage.movies.display());

    // ── Discovery: scan + hot-reload watcher ──────────────────────────────
    let (toast_tx, _) = broadcast::channel::<PluginToast>(32);
    let discovery = Arc::new(Discovery::new(
        cfg.plugin_dir.clone(),
        Arc::clone(&engine),
        toast_tx.clone(),
    ));
    let plugins_loaded = discovery.scan_and_load().await?;
    info!(plugins_loaded, "startup plugin scan complete");
    Arc::clone(&discovery).start_watcher();
    let mut toast_rx = toast_tx.subscribe();

    // ── Stremio addon bridge ───────────────────────────────────────────────
    // Set STUI_STREMIO_ADDONS to a comma-separated list of manifest URLs:
    //   export STUI_STREMIO_ADDONS="https://torrentio.strem.fun/manifest.json"
    // NOTE: Stremio addons are loaded as WASM plugins via Discovery above.
    // This section kept for legacy Stremio adapter if needed.
    let _stremio_addons = stremio::adapter::StremioAddon::from_env().await;
    if _stremio_addons.is_empty() {
        debug!("no Stremio addons configured");
    }

    // ── Catalog uses Engine for WASM plugin-based providers ─────────────────
    // Built-in providers (tmdb, imdb, omdb, anilist, etc.) are now loaded as
    // WASM plugins via Discovery above and accessed through Engine's search().
    let catalog = Arc::new(Catalog::new(cfg.cache_dir.clone(), Arc::clone(&engine)));

    // ── Shared health registry + config manager ───────────────────────────
    let bus     = Arc::new(EventBus::new());
    let health  = Arc::new(HealthRegistry::new());
    let config  = Arc::new(ConfigManager::new(cfg.clone(), Arc::clone(&bus)));
    let bench   = StreamBenchmarker::new();
    let trace = {
        let t = Arc::new(TraceEmitter::new());
        if std::env::var("STUI_TRACE").is_ok() {
            t.enable();
        }
        t
    };
    { let c = Arc::clone(&catalog); tokio::spawn(async move { c.start().await }); }

    // ── Shared event channel (aria2 + mpv/player → Go) ──────────────────
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<String>(128);

    // ── aria2c bridge ─────────────────────────────────────────────────────
    // Initialize the aria2 translator for path organization
    let translator = Aria2Translator::new(
        cfg.data_dir.join("aria2-translations.json"),
    );
    translator.init().await?;
    let aria2 = aria2_bridge::Aria2Bridge::try_connect(translator).await;
    if let Some(ref bridge) = aria2 {
        bridge.spawn_monitors(event_tx.clone());
        info!("aria2: bridge active");
    }

    // ── MPD bridge ────────────────────────────────────────────────────────────
    let mpd_bridge = if cfg.mpd.host != "disabled" {
        let b = MpdBridge::new(cfg.mpd.clone(), event_tx.clone());
        b.apply_config().await;
        info!(host = %cfg.mpd.host, port = cfg.mpd.port, "MPD bridge initialized");
        Some(b)
    } else {
        info!("MPD bridge disabled (set mpd.host in config to enable)");
        None
    };

    // ── Skip detection ────────────────────────────────────────────────────
    let skipper = Skipper::new(
        cfg.skipper.clone(),
        SkipperStore::new(cfg.cache_dir.clone()),
        event_tx.clone(),
    );

    // ── mpv / player bridge ───────────────────────────────────────────────
    let player = player::PlayerBridge::new(
        Arc::clone(&engine),
        aria2.clone(),
        mpd_bridge.clone(),
        Arc::clone(&storage),
        Arc::clone(&watch_history),
        event_tx.clone(),
        cfg.data_dir.to_string_lossy().into_owned(),
        cfg.playback.clone(),
    );

    // ── IPC loop — two modes ──────────────────────────────────────────────
    if daemon_mode {
        // ── Daemon: Unix socket — accepts multiple sequential clients ─────
        // Remove stale socket from previous run
        let _ = std::fs::remove_file(&socket_path);
        // Ensure parent directory exists
        if let Some(dir) = socket_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let listener = tokio::net::UnixListener::bind(&socket_path)?;
        info!(socket = %socket_path.display(), "daemon listening");

        // Write PID file so the TUI can verify the daemon is alive
        let pid_path = socket_path.with_extension("pid");
        std::fs::write(&pid_path, std::process::id().to_string())?;

        loop {
            let (stream, _) = listener.accept().await?;
            info!("daemon: client connected");
            let (rx, tx) = tokio::io::split(stream);
            let mut reader = BufReader::new(rx).lines();
            let mut writer = tokio::io::BufWriter::new(tx);

            // Borrow all the shared state for this client session
            run_ipc_loop(
                &mut reader,
                &mut writer,
                &engine,
                &catalog,
                &player,
                mpd_bridge.as_ref(),
                &health,
                &config,
                &skipper,
                &watch_history,
                &media_cache,
                &bench,
                &trace,
                &mut event_rx,
                event_tx.clone(),
                &mut toast_rx,
            ).await?;
            info!("daemon: client disconnected");
        }
    } else {
        // ── Inline: stdin/stdout — single TUI process ─────────────────────
        let stdin  = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin).lines();
        let mut writer = tokio::io::BufWriter::new(stdout);

        info!("IPC loop ready (inline mode)");

        run_ipc_loop(
            &mut reader,
            &mut writer,
            &engine,
            &catalog,
            &player,
            mpd_bridge.as_ref(),
            &health,
            &config,
            &skipper,
            &watch_history,
            &media_cache,
            &bench,
            &trace,
            &mut event_rx,
            event_tx.clone(),
            &mut toast_rx,
        ).await?;
    }

    Ok(())
}

async fn run_ipc_loop<R, W>(
    reader:    &mut tokio::io::Lines<tokio::io::BufReader<R>>,
    mut writer: &mut tokio::io::BufWriter<W>,
    engine:    &Arc<Engine>,
    catalog:   &Arc<Catalog>,
    player:    &player::PlayerBridge,
    mpd:       Option<&MpdBridge>,
    health:    &Arc<HealthRegistry>,
    config:    &Arc<ConfigManager>,
    skipper:   &Arc<Skipper>,
    watch_history: &Arc<watchhistory::WatchHistoryStore>,
    media_cache: &Arc<mediacache::MediaCacheStore>,
    bench:     &StreamBenchmarker,
    trace:     &Arc<TraceEmitter>,
    // Receiver for async events pushed by background tasks (player, aria2, registry, …).
    event_rx:  &mut tokio::sync::mpsc::Receiver<String>,
    // Sender used to push responses from background-spawned tasks back into the loop.
    event_tx:  tokio::sync::mpsc::Sender<String>,
    toast_rx:  &mut tokio::sync::broadcast::Receiver<PluginToast>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let grid_rx = catalog.subscribe();
    let mut batcher = ipc_batcher::IpcBatcher::new(grid_rx);
    loop {
        tokio::select! {
            // Incoming request from Go TUI
            line = reader.next_line() => {
                match line? {
                    None => { info!("client disconnected"); break; }
                    Some(line) => {
                        let line = line.trim().to_string();
                        if line.is_empty() { continue; }
                        // Play commands are fire-and-forget — don't send a Response
                        let msg_type = serde_json::from_str::<serde_json::Value>(&line)
                            .ok()
                            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|s| s.to_string()))
                            .unwrap_or_default();
                        match msg_type.as_str() {
                            // ── Long-running ops — spawned in background so the IPC loop
                            //    stays responsive while network I/O is in flight.
                            "browse_registry" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let req_id = val["id"].as_str().unwrap_or("").to_string();
                                let config_c = Arc::clone(config);
                                let tx = event_tx.clone();
                                tokio::spawn(async move {
                                    let resp = pipeline::registry::run_browse_registry(&config_c).await;
                                    // Inject the correlation id so Go can route the response.
                                    if !req_id.is_empty() {
                                        if let Ok(mut v) = serde_json::to_value(&resp) {
                                            v["id"] = serde_json::Value::String(req_id);
                                            if let Ok(mut wire) = serde_json::to_string(&v) {
                                                wire.push('\n');
                                                let _ = tx.send(wire).await;
                                                return;
                                            }
                                        }
                                    }
                                    if let Ok(wire) = resp.to_wire() { let _ = tx.send(wire).await; }
                                });
                                continue;
                            }
                            "install_plugin" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let req_id = val["id"].as_str().unwrap_or("").to_string();
                                let req = ipc::InstallPluginRequest {
                                    name:       val["name"].as_str().unwrap_or("").to_string(),
                                    version:    val["version"].as_str().unwrap_or("").to_string(),
                                    binary_url: val["binary_url"].as_str().unwrap_or("").to_string(),
                                    checksum:   val["checksum"].as_str().unwrap_or("").to_string(),
                                };
                                let config_c = Arc::clone(config);
                                let tx = event_tx.clone();
                                tokio::spawn(async move {
                                    let resp = pipeline::registry::run_install_plugin(&config_c, req).await;
                                    if !req_id.is_empty() {
                                        if let Ok(mut v) = serde_json::to_value(&resp) {
                                            v["id"] = serde_json::Value::String(req_id);
                                            if let Ok(mut wire) = serde_json::to_string(&v) {
                                                wire.push('\n');
                                                let _ = tx.send(wire).await;
                                                return;
                                            }
                                        }
                                    }
                                    if let Ok(wire) = resp.to_wire() { let _ = tx.send(wire).await; }
                                });
                                continue;
                            }
                            "play" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let tab = val["tab"].as_str().and_then(|t| match t {
                                    "music"    => Some(ipc::MediaTab::Music),
                                    "radio"    => Some(ipc::MediaTab::Radio),
                                    "podcasts" => Some(ipc::MediaTab::Podcasts),
                                    _          => None,
                                });
                                let media_type = val["media_type"].as_str()
                                    .and_then(|s| serde_json::from_str::<ipc::MediaType>(s).ok());
                                let year = val["year"].as_u64().map(|y| y as u32);
                                pipeline::playback::run_play(
                                    player.clone(),
                                    Arc::clone(&skipper),
                                    Arc::clone(&engine),
                                    val["entry_id"].as_str().unwrap_or("").to_string(),
                                    val["provider"].as_str().unwrap_or("").to_string(),
                                    val["imdb_id"].as_str().unwrap_or("").to_string(),
                                    tab,
                                    media_type,
                                    year,
                                );
                                continue;
                            }
                            "get_mpd_outputs" => {
                                let resp = pipeline::playback::run_get_mpd_outputs(mpd).await;
                                if let Ok(wire) = resp.to_wire() {
                                    send_wire(&mut writer, &wire).await?;
                                }
                                continue;
                            }
                            "player_stop" => {
                                let p = player.clone();
                                tokio::spawn(async move { p.stop().await });
                                continue;
                            }
                            "download_stream" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let url   = val["url"].as_str().unwrap_or("").to_string();
                                let title = val["title"].as_str().unwrap_or("").to_string();
                                let media_type = val["media_type"].as_str()
                                    .and_then(|s| serde_json::from_str::<ipc::MediaType>(s).ok());
                                let year = val["year"].as_u64().map(|y| y as u32);
                                let p = player.clone();
                                tokio::spawn(async move { p.download_only(&url, &title, media_type, year).await });
                                continue;
                            }
                            "download_cancel" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let gid = val["gid"].as_str().unwrap_or("").to_string();
                                let p = player.clone();
                                tokio::spawn(async move { p.cancel_download(&gid).await });
                                continue;
                            }
                            "play_file" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let path  = val["path"].as_str().unwrap_or("").to_string();
                                let title = val["title"].as_str().unwrap_or("").to_string();
                                let p = player.clone();
                                tokio::spawn(async move { p.play_local_file(&path, &title).await });
                                continue;
                            }
                            "player_command" => {
                                let val: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                                let cmd  = val["cmd"].as_str().unwrap_or("").to_string();
                                let args = val["args"].as_array().cloned().unwrap_or_default();
                                let p = player.clone();
                                tokio::spawn(async move { p.send_command(&cmd, &args).await });
                                continue;
                            }
                            _ => {}
                        }
                        let resp = handle_line(&engine, &catalog, health, config, player, mpd, watch_history, media_cache, &bench, &trace, &line).await;
                        send_wire(&mut writer, &resp.to_wire()?).await?;
                    }
                }
            }

            // Grid updates from catalog (batched for efficiency)
            updates = batcher.recv() => {
                if let Some(batch) = updates {
                    for u in batch {
                        info!(tab=%u.tab, source=?u.source, count=u.entries.len(), "grid update");
                        let source = match u.source {
                            catalog::GridUpdateSource::Cache => "cache".to_string(),
                            catalog::GridUpdateSource::Live  => "live".to_string(),
                        };
                        let entries: Vec<ipc::MediaEntry> = u.entries.iter().map(|e| {
                            let tab = match e.tab.as_str() {
                                "movies"   => ipc::MediaTab::Movies,
                                "series"   => ipc::MediaTab::Series,
                                "music"    => ipc::MediaTab::Music,
                                "radio"    => ipc::MediaTab::Radio,
                                "podcasts" => ipc::MediaTab::Podcasts,
                                "videos"   => ipc::MediaTab::Videos,
                                _          => ipc::MediaTab::Library,
                            };
                            ipc::MediaEntry {
                                id: e.id.clone(), title: e.title.clone(), year: e.year.clone(), genre: e.genre.clone(),
                                rating: e.rating.clone(), ratings: e.ratings.clone(),
                                description: e.description.clone(),
                                poster_url: e.poster_url.clone(), provider: e.provider.clone(),
                                tab, media_type: e.media_type.clone(),
                            }
                        }).collect();

                        // Cache live updates in media cache
                        if u.source == catalog::GridUpdateSource::Live {
                            let mc = media_cache.clone();
                            let tab_name = u.tab.clone();
                            let entries_to_cache = entries.clone();
                            tokio::spawn(async move {
                                mc.save_tab(tab_name, entries_to_cache).await;
                            });
                        }

                        let msg = GridUpdateMsg { tab: u.tab, entries, source };
                        send_wire(&mut writer, &msg.to_wire()?).await?;
                    }
                } else {
                    info!("grid update channel closed");
                    break;
                }
            }

            // Plugin hot-load toast
            toast = toast_rx.recv() => {
                match toast {
                    Ok(t) => {
                        let wire = plugin_toast_wire(&t)?;
                        send_wire(&mut writer, &wire).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("toast channel lagged {n}");
                    }
                    Err(_) => {}
                }
            }

            // aria2 + mpv player events (progress, complete, started, ended …)
            Some(event_msg) = event_rx.recv() => {
                let mut line = event_msg;
                if !line.ends_with('\n') { line.push('\n'); }
                send_wire(&mut writer, &line).await?;
            }
        }
    }

    Ok(())
}

fn default_socket_path() -> std::path::PathBuf {
    // XDG_RUNTIME_DIR is preferred; fall back to ~/.local/run/
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
                .join(".local").join("run")
        });
    base.join("stui.sock")
}

async fn send_wire<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut tokio::io::BufWriter<W>,
    wire: &str,
) -> Result<()> {
    writer.write_all(wire.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

fn plugin_toast_wire(t: &PluginToast) -> Result<String> {
    let mut s = serde_json::to_string(&serde_json::json!({
        "type":        "plugin_toast",
        "plugin_name": t.plugin_name,
        "version":     t.version,
        "plugin_type": t.plugin_type,
        "message":     t.message,
        "is_error":    t.is_error,
    }))?;
    s.push('\n');
    Ok(s)
}

// ── Request dispatch ──────────────────────────────────────────────────────────

async fn handle_line(
    engine: &Arc<Engine>,
    catalog: &Arc<Catalog>,
    health: &Arc<HealthRegistry>,
    config: &Arc<ConfigManager>,
    player: &player::PlayerBridge,
    mpd: Option<&MpdBridge>,
    watch_history: &Arc<watchhistory::WatchHistoryStore>,
    media_cache: &Arc<mediacache::MediaCacheStore>,
    bench: &StreamBenchmarker,
    trace: &Arc<TraceEmitter>,
    line: &str,
) -> Response {
    let request: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return Response::error(None, ErrorCode::InvalidRequest, e.to_string()),
    };

    match request {
        Request::Ping { ipc_version } => {
            let version_ok = match ipc_version {
                Some(v) if v == ipc::CURRENT_VERSION => true,
                Some(v) => {
                    warn!(
                        client_ipc_version = v,
                        runtime_ipc_version = ipc::CURRENT_VERSION,
                        "IPC version mismatch — consider upgrading TUI or runtime"
                    );
                    false
                }
                None => {
                    warn!("TUI sent ping without ipc_version (old client) — assuming compatible");
                    true
                }
            };
            Response::Pong {
                ipc_version:     ipc::CURRENT_VERSION,
                runtime_version: env!("CARGO_PKG_VERSION").to_string(),
                version_ok,
            }
        }
        Request::Shutdown => Response::Ok,
        Request::ListPlugins => engine.list_plugins().await,

        Request::LoadPlugin(r) => {
            match engine.load_plugin(&PathBuf::from(&r.path)).await {
                Ok(resp) => resp,
                Err(e) => Response::error(None, ErrorCode::PluginLoadFailed, e.to_string()),
            }
        }

        Request::UnloadPlugin(r) => match engine.unload_plugin(&r.plugin_id).await {
            Ok(resp) => resp,
            Err(e) => Response::error(None, ErrorCode::PluginNotFound, e.to_string()),
        },

        Request::Search(r)    => pipeline::search::run_search(engine, catalog, trace, r).await,

        Request::Resolve(r)   => engine.resolve(&r.id, &r.entry_id, &r.provider).await,

        Request::GetStreams(r) => pipeline::resolve::run_get_streams(engine, catalog, config, health, bench, trace, r).await,

        Request::Metadata(r) => Response::error(
            Some(r.id), ErrorCode::MetadataFailed,
            "Metadata plugins not yet implemented".to_string(),
        ),

        Request::PlayerCommand(r) => pipeline::playback::run_player_command(player, r).await,

        Request::Cmd(cmd) => pipeline::playback::run_player_cmd(player, mpd, cmd).await,

        Request::SetConfig(r)         => pipeline::config::run_set_config(config, r).await,
        Request::GetProviderSettings  => pipeline::config::run_get_provider_settings(engine, config).await,
        Request::GetPluginRepos       => pipeline::config::run_get_plugin_repos(config).await,
        Request::SetPluginRepos(r)    => pipeline::config::run_set_plugin_repos(config, r).await,
        Request::BrowseRegistry       => pipeline::registry::run_browse_registry(config).await,
        Request::InstallPlugin(r)     => pipeline::registry::run_install_plugin(config, r).await,
        Request::RankStreams(r)       => pipeline::rank::run_rank_streams(r).await,

        // Watch history requests
        Request::GetWatchHistoryEntry(r) => {
            let entry: Option<watchhistory::WatchHistoryEntry> = watch_history.get(&r.id).await;
            Response::WatchHistoryEntry(ipc::WatchHistoryEntryResponse {
                entry: entry.map(ipc::WatchHistoryEntryWire::from),
            })
        }
        Request::GetWatchHistoryInProgress(r) => {
            let entries: Vec<watchhistory::WatchHistoryEntry> = watch_history.in_progress_for_tab(&r.tab).await;
            Response::WatchHistoryInProgress(ipc::WatchHistoryInProgressResponse {
                entries: entries.into_iter().map(ipc::WatchHistoryEntryWire::from).collect(),
            })
        }
        Request::UpsertWatchHistoryEntry(r) => {
            let entry: watchhistory::WatchHistoryEntry = r.entry.into();
            watch_history.upsert(entry).await;
            Response::WatchHistoryUpsert(ipc::WatchHistoryUpsertResponse { success: true })
        }
        Request::UpdateWatchHistoryPosition(r) => {
            let success = watch_history.update_position(&r.id, r.position, r.duration).await;
            Response::WatchHistoryPositionUpdate(ipc::WatchHistoryPositionUpdateResponse { success })
        }
        Request::MarkWatchHistoryCompleted(r) => {
            watch_history.mark_completed(&r.id).await;
            Response::WatchHistoryCompleted(ipc::WatchHistoryUpsertResponse { success: true })
        }
        Request::RemoveWatchHistoryEntry(r) => {
            watch_history.remove(&r.id).await;
            Response::WatchHistoryRemoved(ipc::WatchHistoryRemoveResponse { success: true })
        }

        // Media cache requests
        Request::GetMediaCacheTab(r) => {
            let entries = media_cache.entries_for_tab(&r.tab).await;
            let updated_at = media_cache.tab_updated_at(&r.tab).await;
            Response::MediaCacheTab(ipc::MediaCacheTabResponse {
                tab: r.tab,
                entries,
                updated_at,
            })
        }
        Request::GetMediaCacheAll(_) => {
            let entries = media_cache.all_entries().await;
            Response::MediaCacheAll(ipc::MediaCacheAllResponse { entries })
        }
        Request::GetMediaCacheStats(_) => {
            let total_count = media_cache.total_count().await;
            let last_updated = media_cache.last_updated().await;
            Response::MediaCacheStats(ipc::MediaCacheStatsResponse {
                total_count,
                last_updated,
            })
        }
        Request::ClearMediaCache(_) => {
            media_cache.clear().await;
            Response::MediaCacheCleared(ipc::MediaCacheClearResponse { success: true })
            // Note: clear() logs errors internally, so we return success=true 
            // if no panic occurred. The TUI can verify by checking cache stats.
        }

        // ── Storage paths ─────────────────────────────────────────────────────
        Request::GetStoragePaths => {
            let cfg = config.snapshot().await;
            Response::StoragePaths(ipc::StoragePathsResponse {
                movies: cfg.storage.movies.display().to_string(),
                series: cfg.storage.series.display().to_string(),
                music: cfg.storage.music.display().to_string(),
                anime: cfg.storage.anime.display().to_string(),
                podcasts: cfg.storage.podcasts.display().to_string(),
            })
        }
        Request::SetStoragePaths(r) => {
            let cfg = config.clone();
            let mut errors = Vec::new();
            use serde_json::Value;
            
            if let Some(ref path) = r.movies {
                if let Err(e) = cfg.set("storage.movies", Value::String(path.clone())).await {
                    errors.push(format!("movies: {}", e));
                }
            }
            if let Some(ref path) = r.series {
                if let Err(e) = cfg.set("storage.series", Value::String(path.clone())).await {
                    errors.push(format!("series: {}", e));
                }
            }
            if let Some(ref path) = r.music {
                if let Err(e) = cfg.set("storage.music", Value::String(path.clone())).await {
                    errors.push(format!("music: {}", e));
                }
            }
            if let Some(ref path) = r.anime {
                if let Err(e) = cfg.set("storage.anime", Value::String(path.clone())).await {
                    errors.push(format!("anime: {}", e));
                }
            }
            if let Some(ref path) = r.podcasts {
                if let Err(e) = cfg.set("storage.podcasts", Value::String(path.clone())).await {
                    errors.push(format!("podcasts: {}", e));
                }
            }
            
            if errors.is_empty() {
                Response::StoragePathsUpdated { success: true }
            } else {
                Response::error(None, ErrorCode::InvalidRequest, errors.join("; "))
            }
        }

        // ── Stream policy ─────────────────────────────────────────────────────
        Request::GetStreamPolicy => {
            let prefs = pipeline::policy_io::load_stream_policy();
            Response::StreamPolicy(ipc::StreamPolicyResponse {
                policy: prefs.into(),
            })
        }
        Request::SetStreamPolicy(req) => {
            let prefs: crate::quality::StreamPreferences = req.policy.into();
            if let Err(e) = pipeline::policy_io::save_stream_policy(&prefs) {
                warn!("save_stream_policy failed: {e}");
            }
            Response::StreamPolicyUpdated
        }

        Request::SetTrace { enabled } => {
            if enabled {
                trace.enable();
            }
            Response::Ok
        }

        // Play and PlayerStop are handled earlier in the IPC loop before
        // reaching handle_line — these arms are unreachable in practice.
        Request::Play(_) | Request::PlayerStop => Response::Ok,

        // GetMpdOutputs is handled earlier in the IPC loop (needs async mpd.outputs()).
        Request::GetMpdOutputs => Response::error(None, ErrorCode::InvalidRequest, "use get_mpd_outputs message type".to_string()),
    }
}

