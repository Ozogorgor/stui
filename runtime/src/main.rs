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
mod theme_watcher;
mod pipeline;
mod plugin_rpc;
mod registry;
mod skipper;


use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use catalog::Catalog;
use discovery::{Discovery, PluginToast};
use engine::Engine;
use config::ConfigManager;
use events::EventBus;
use ipc::{ErrorCode, GridUpdateMsg, Request, Response};
use mpd_bridge::MpdBridge;
use skipper::{Skipper, SkipperStore};
use providers::{HealthRegistry, metadata::{ImdbProvider, OmdbProvider, TmdbProvider}, Provider};
#[cfg(feature = "anime")]
use providers::metadata::{AniListProvider, JikanProvider};
#[cfg(feature = "music")]
use providers::metadata::{LastFmProvider, MusicBrainzProvider};

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

    // ── Built-in catalog providers ────────────────────────────────────────
    let mut built_in: Vec<Arc<dyn Provider>> = vec![];

    // Always-on metadata providers (no API key needed)
    if cfg.providers.enable_imdb {
        built_in.push(Arc::new(ImdbProvider::new()));
        info!("IMDB provider enabled");
    }

    #[cfg(feature = "anime")]
    {
        if cfg.providers.enable_anilist {
            built_in.push(Arc::new(AniListProvider::new()));
            info!("AniList provider enabled");
        }
        if cfg.providers.enable_jikan {
            built_in.push(Arc::new(JikanProvider::new()));
            info!("Jikan (MyAnimeList) provider enabled");
        }
    }

    #[cfg(feature = "music")]
    {
        if cfg.providers.enable_musicbrainz {
            built_in.push(Arc::new(MusicBrainzProvider::new()));
            info!("MusicBrainz provider enabled");
        }
        // Last.fm — enabled whenever an API key is available
        if let Some(p) = LastFmProvider::from_config(&cfg.api_keys) {
            info!("Last.fm provider enabled");
            built_in.push(Arc::new(p));
        } else {
            info!("Last.fm provider not active (no API key — configure via plugin settings)");
        }
    }

    // API-key-gated providers
    if cfg.providers.enable_omdb {
        if let Some(p) = OmdbProvider::from_config(&cfg.api_keys) {
            info!("OMDB provider enabled");
            built_in.push(Arc::new(p));
        } else {
            warn!("OMDB enabled in config but no API key — set via plugin settings or OMDB_API_KEY");
        }
    }
    if cfg.providers.enable_tmdb {
        if let Some(p) = TmdbProvider::from_config(&cfg.api_keys) {
            info!("TMDB provider enabled");
            built_in.push(Arc::new(p));
        } else {
            warn!("TMDB enabled in config but no API key — set via plugin settings or TMDB_API_KEY");
        }
    }

    // radio: no built-in provider yet — external plugins handle it.
    #[cfg(not(feature = "radio"))]
    let _ = &cfg.providers; // suppress unused warning if only radio was configured

    // ── Stremio addon bridge ───────────────────────────────────────────────
    // Set STUI_STREMIO_ADDONS to a comma-separated list of manifest URLs:
    //   export STUI_STREMIO_ADDONS="https://torrentio.strem.fun/manifest.json"
    let stremio_addons = stremio::adapter::StremioAddon::from_env().await;
    if stremio_addons.is_empty() {
        info!("no Stremio addons configured (set STUI_STREMIO_ADDONS to enable)");
    }
    for addon in stremio_addons {
        info!("stremio: registered addon '{}'", addon.name());
        built_in.push(Arc::new(addon));
    }

    let catalog = Arc::new(Catalog::new(cfg.cache_dir.clone(), built_in));
    let mut grid_rx = catalog.subscribe();

    // ── Shared health registry + config manager ───────────────────────────
    let bus     = Arc::new(EventBus::new());
    let health  = Arc::new(HealthRegistry::new());
    let config  = Arc::new(ConfigManager::new(cfg.clone(), Arc::clone(&bus)));
    { let c = Arc::clone(&catalog); tokio::spawn(async move { c.start().await }); }

    // ── Shared event channel (aria2 + mpv/player → Go) ──────────────────
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<String>(128);

    // ── aria2c bridge ─────────────────────────────────────────────────────
    let aria2 = aria2_bridge::Aria2Bridge::try_connect().await;
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
        event_tx.clone(),
        cfg.data_dir.to_string_lossy().into_owned(),
        cfg.playback.clone(),
    );

    // ── matugen theme watcher ─────────────────────────────────────────────
    let colors_path = theme_watcher::resolve_colors_path();
    let theme_mode  = cfg.theme_mode.clone();
    let mut theme_rx = theme_watcher::start_watcher(colors_path, theme_mode);

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
                &mut event_rx,
                event_tx.clone(),
                &mut grid_rx,
                &mut toast_rx,
                &mut theme_rx,
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
            &mut event_rx,
            event_tx.clone(),
            &mut grid_rx,
            &mut toast_rx,
            &mut theme_rx,
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
    // Receiver for async events pushed by background tasks (player, aria2, registry, …).
    event_rx:  &mut tokio::sync::mpsc::Receiver<String>,
    // Sender used to push responses from background-spawned tasks back into the loop.
    event_tx:  tokio::sync::mpsc::Sender<String>,
    grid_rx:   &mut tokio::sync::broadcast::Receiver<catalog::GridUpdate>,
    toast_rx:  &mut tokio::sync::broadcast::Receiver<PluginToast>,
    theme_rx:  &mut tokio::sync::broadcast::Receiver<theme_watcher::ThemeColors>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
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
                                    let mut resp = pipeline::registry::run_browse_registry(&config_c).await;
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
                                pipeline::playback::run_play(
                                    player.clone(),
                                    Arc::clone(&skipper),
                                    Arc::clone(&engine),
                                    val["entry_id"].as_str().unwrap_or("").to_string(),
                                    val["provider"].as_str().unwrap_or("").to_string(),
                                    val["imdb_id"].as_str().unwrap_or("").to_string(),
                                    tab,
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
                                let p = player.clone();
                                tokio::spawn(async move { p.download_only(&url, &title).await });
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
                        let resp = handle_line(&engine, &catalog, health, config, player, mpd, &line).await;
                        send_wire(&mut writer, &resp.to_wire()?).await?;
                    }
                }
            }

            // Grid update from catalog
            update = grid_rx.recv() => {
                match update {
                    Ok(u) => {
                        info!(tab=%u.tab, source=?u.source, count=u.entries.len(), "grid update");
                        let source = match u.source {
                            catalog::GridUpdateSource::Cache => "cache".to_string(),
                            catalog::GridUpdateSource::Live  => "live".to_string(),
                        };
                        let entries: Vec<ipc::MediaEntry> = u.entries.into_iter().map(|e| {
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
                                id: e.id, title: e.title, year: e.year, genre: e.genre,
                                rating: e.rating, ratings: e.ratings,
                                description: e.description,
                                poster_url: e.poster_url, provider: e.provider,
                                tab, media_type: e.media_type,
                            }
                        }).collect();
                        let msg = GridUpdateMsg { tab: u.tab, entries, source };
                        send_wire(&mut writer, &msg.to_wire()?).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("grid channel lagged {n}");
                    }
                    Err(_) => {}
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

            // matugen theme update
            theme = theme_rx.recv() => {
                match theme {
                    Ok(tc) => {
                        info!(colors=%tc.colors.len(), mode=%tc.mode, "theme update from matugen");
                        let mut buf = Vec::new();
                        if theme_watcher::emit_theme_update(&mut buf, &tc).is_ok() {
                            if let Ok(s) = String::from_utf8(buf) {
                                send_wire(&mut writer, &s).await?;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("theme channel lagged {n}");
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

async fn handle_line(engine: &Arc<Engine>, catalog: &Arc<Catalog>, health: &Arc<HealthRegistry>, config: &Arc<ConfigManager>, player: &player::PlayerBridge, mpd: Option<&MpdBridge>, line: &str) -> Response {
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

        Request::Search(r)    => pipeline::search::run_search(engine, catalog, r).await,

        Request::Resolve(r)   => engine.resolve(&r.id, &r.entry_id, &r.provider).await,

        Request::GetStreams(r) => pipeline::resolve::run_get_streams(engine, catalog, r).await,

        Request::Metadata(r) => Response::error(
            Some(r.id), ErrorCode::MetadataFailed,
            "Metadata plugins not yet implemented".to_string(),
        ),

        Request::PlayerCommand(r) => pipeline::playback::run_player_command(player, r).await,

        Request::Cmd(cmd) => pipeline::playback::run_player_cmd(player, mpd, cmd).await,

        Request::SetConfig(r)         => pipeline::config::run_set_config(config, r).await,
        Request::GetProviderSettings  => pipeline::config::run_get_provider_settings(catalog).await,
        Request::GetPluginRepos       => pipeline::config::run_get_plugin_repos(config).await,
        Request::SetPluginRepos(r)    => pipeline::config::run_set_plugin_repos(config, r).await,
        Request::BrowseRegistry       => pipeline::registry::run_browse_registry(config).await,
        Request::InstallPlugin(r)     => pipeline::registry::run_install_plugin(config, r).await,

        // Play and PlayerStop are handled earlier in the IPC loop before
        // reaching handle_line — these arms are unreachable in practice.
        Request::Play(_) | Request::PlayerStop => Response::Ok,

        // GetMpdOutputs is handled earlier in the IPC loop (needs async mpd.outputs()).
        Request::GetMpdOutputs => Response::error(None, ErrorCode::InvalidRequest, "use get_mpd_outputs message type".to_string()),
    }
}

