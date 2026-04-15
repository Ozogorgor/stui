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
mod dsp;
mod roon;
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
use dsp::{DspPipeline, OutputTarget};
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

    // Initialize process-wide exception store for music tag normalization.
    mediacache::normalize::store::init(
        mediacache::normalize::store::default_bundled_path(),
        mediacache::normalize::store::default_user_path(),
    );

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

    // ── Tag-write job state (for Action A tag normalization) ─────────────────
    let tag_job_store = Arc::new(mediacache::tag_write_job::JobStore::new());
    let tag_job_registry = Arc::new(mediacache::tag_write_job::JobRegistry::new());

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
    let (toast_tx, _) = broadcast::channel::<PluginToast>(256);
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
        let b = MpdBridge::new(cfg.mpd.clone(), event_tx.clone(), cfg.music.normalize.clone());
        b.apply_config().await;
        info!(host = %cfg.mpd.host, port = cfg.mpd.port, "MPD bridge initialized");
        Some(b)
    } else {
        info!("MPD bridge disabled (set mpd.host in config to enable)");
        None
    };

    // ── DSP pipeline ──────────────────────────────────────────────────────────
    let dsp_pipeline: Option<Arc<tokio::sync::Mutex<DspPipeline>>> = if cfg.dsp.enabled {
        let pipeline = dsp::DspPipeline::with_config_dir(cfg.dsp.clone(), cfg.data_dir.clone());
        info!(sample_rate = pipeline.output_sample_rate(), "DSP pipeline initialized");
        Some(Arc::new(tokio::sync::Mutex::new(pipeline)))
    } else {
        info!("DSP disabled (set dsp.enabled to enable)");
        None
    };

    // ── MPD → DSP FIFO integration ────────────────────────────────────────────
    // When both MPD and DSP are active, route MPD's audio through the DSP
    // pipeline via a named FIFO.  MPD writes raw 16-bit LE stereo PCM to the
    // FIFO; stui reads it, processes it through the DSP chain, and sends it
    // to the configured output (PipeWire / ALSA).
    if let (Some(ref dsp), Some(ref mpd)) = (&dsp_pipeline, &mpd_bridge) {
        let fifo_path  = dsp::mpd_config::DEFAULT_FIFO_PATH.to_string();
        let conf_path_opt = dsp::mpd_config::find_mpd_conf();
        let sample_rate: u32 = conf_path_opt.as_deref()
            .and_then(dsp::mpd_config::parse_fifo_sample_rate)
            .unwrap_or(44100);

        // Patch mpd.conf with the FIFO stanza if not already present.
        if let Some(conf_path) = conf_path_opt {
            match dsp::mpd_config::ensure_mpd_conf(&conf_path, &fifo_path, sample_rate) {
                Ok(true)  => info!(
                    path = %conf_path.display(),
                    "patched mpd.conf with stui-dsp FIFO output — restart MPD to apply"
                ),
                Ok(false) => {}
                Err(e)    => warn!(error = %e, "could not patch mpd.conf"),
            }
        } else {
            info!("mpd.conf not found — add the stui-dsp FIFO output stanza manually");
        }

        // Enable the MPD output if it already exists (from a previous restart).
        match mpd.ensure_dsp_output_enabled().await {
            Ok(true)  => info!("stui-dsp MPD FIFO output enabled"),
            Ok(false) => info!(
                path = %fifo_path,
                "stui-dsp output not found in MPD — restart MPD after mpd.conf is patched"
            ),
            Err(e)    => warn!(error = %e, "could not query/enable stui-dsp MPD output"),
        }

        // Spawn the long-running FIFO reader / DSP loop.
        // run_mpd_dsp_loop is intended to run forever; log if it ever exits.
        let pipeline_arc = Arc::clone(dsp);
        tokio::spawn(async move {
            dsp::run_mpd_dsp_loop(pipeline_arc, fifo_path, sample_rate).await;
            warn!("MPD DSP FIFO loop exited unexpectedly — DSP processing has stopped");
        });
        info!("MPD DSP integration active (FIFO → pipeline → output)");
    }

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
        dsp_pipeline.clone(),
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

            // Borrow all the shared state for this client session.
            // Errors are connection-level (broken pipe, client crash) and must
            // not propagate to main() — that would kill the whole daemon.
            match run_ipc_loop(
                &mut reader,
                &mut writer,
                &engine,
                &catalog,
                &player,
                mpd_bridge.as_ref(),
                dsp_pipeline.as_ref(),
                &health,
                &config,
                &skipper,
                &watch_history,
                &media_cache,
                &bench,
                &trace,
                &tag_job_store,
                &tag_job_registry,
                &mut event_rx,
                event_tx.clone(),
                &mut toast_rx,
            ).await {
                Ok(()) => info!("daemon: client disconnected"),
                Err(e) => warn!(error = %e, "daemon: client session ended with error"),
            }
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
            dsp_pipeline.as_ref(),
            &health,
            &config,
            &skipper,
            &watch_history,
            &media_cache,
            &bench,
            &trace,
            &tag_job_store,
            &tag_job_registry,
            &mut event_rx,
            event_tx.clone(),
            &mut toast_rx,
        ).await?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_ipc_loop<R, W>(
    reader:    &mut tokio::io::Lines<tokio::io::BufReader<R>>,
    mut writer: &mut tokio::io::BufWriter<W>,
    engine:    &Arc<Engine>,
    catalog:   &Arc<Catalog>,
    player:    &player::PlayerBridge,
    mpd:       Option<&MpdBridge>,
    dsp:       Option<&Arc<tokio::sync::Mutex<DspPipeline>>>,
    health:    &Arc<HealthRegistry>,
    config:    &Arc<ConfigManager>,
    skipper:   &Arc<Skipper>,
    watch_history: &Arc<watchhistory::WatchHistoryStore>,
    media_cache: &Arc<mediacache::MediaCacheStore>,
    bench:     &StreamBenchmarker,
    trace:     &Arc<TraceEmitter>,
    tag_job_store:    &Arc<mediacache::tag_write_job::JobStore>,
    tag_job_registry: &Arc<mediacache::tag_write_job::JobRegistry>,
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
                        // Limit IPC message size to prevent memory exhaustion
                        const MAX_MSG_SIZE: usize = 1024 * 1024; // 1MB
                        if line.len() > MAX_MSG_SIZE {
                            warn!(size = line.len(), "IPC message exceeds size limit, rejecting");
                            let _ = send_error(&mut writer, "payload_too_large", &format!("message size {} exceeds limit {}", line.len(), MAX_MSG_SIZE)).await;
                            continue;
                        }
                        // Parse once so every branch works from a consistent value and
                        // malformed JSON is caught early rather than silently producing
                        // empty req_ids that can't be routed back to the Go caller.
                        let val: serde_json::Value = match serde_json::from_str(&line) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!("IPC: malformed JSON: {}", e);
                                let _ = send_error(&mut writer, "bad_request", "malformed JSON").await;
                                continue;
                            }
                        };
                        let msg_type = val.get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string();
                        match msg_type.as_str() {
                            // ── Long-running ops — spawned in background so the IPC loop
                            //    stays responsive while network I/O is in flight.
                            "browse_registry" => {
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
                                let gid = val["gid"].as_str().unwrap_or("").to_string();
                                let p = player.clone();
                                tokio::spawn(async move { p.cancel_download(&gid).await });
                                continue;
                            }
                            "play_file" => {
                                let path  = val["path"].as_str().unwrap_or("").to_string();
                                let title = val["title"].as_str().unwrap_or("").to_string();
                                let p = player.clone();
                                tokio::spawn(async move { p.play_local_file(&path, &title).await });
                                continue;
                            }
                            "player_command" => {
                                let cmd  = val["cmd"].as_str().unwrap_or("").to_string();
                                let args = val["args"].as_array().cloned().unwrap_or_default();
                                let p = player.clone();
                                tokio::spawn(async move { p.send_command(&cmd, &args).await });
                                continue;
                            }
                            _ => {}
                        }
                        let resp = handle_line(&engine, &catalog, health, config, player, mpd, dsp, watch_history, media_cache, &bench, &trace, &tag_job_store, &tag_job_registry, &line).await;
                        // Echo the request's `id` (if present) into the response envelope so the
                        // TUI's pending-request router can match the response to its caller.
                        // Variants whose struct already includes `id` are left alone.
                        let req_id = val.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let wire = inject_id_into_response(&resp.to_wire()?, req_id.as_deref());
                        send_wire(&mut writer, &wire).await?;
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
                                imdb_id: e.imdb_id.clone(),
                                tmdb_id: e.tmdb_id.clone(),
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

/// Ensure the outgoing response carries the caller's request `id` field.
///
/// The TUI's IPC client routes responses to waiting callers via a pending-id
/// map; a response without an `id` falls through to the unsolicited dispatcher,
/// which silently drops messages it doesn't recognize — blocking the caller
/// forever.  Some `Response` variants already embed `id` in their struct; for
/// those, this is a no-op.  For the rest, we inject the request id into the
/// top-level JSON object before sending.
fn inject_id_into_response(wire: &str, req_id: Option<&str>) -> String {
    let Some(id) = req_id else { return wire.to_string(); };
    let trimmed = wire.trim_end_matches('\n');
    let mut value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return wire.to_string(),
    };
    if let Some(map) = value.as_object_mut() {
        if !map.contains_key("id") {
            map.insert("id".to_string(), serde_json::Value::String(id.to_string()));
        }
    }
    let mut out = serde_json::to_string(&value).unwrap_or_else(|_| trimmed.to_string());
    out.push('\n');
    out
}

async fn send_error<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut tokio::io::BufWriter<W>,
    code: &str,
    message: &str,
) -> Result<()> {
    let wire = serde_json::to_string(&serde_json::json!({
        "type": "error",
        "code": code,
        "message": message,
    }))?;
    let mut wire = wire;
    wire.push('\n');
    send_wire(writer, &wire).await
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
    dsp: Option<&Arc<tokio::sync::Mutex<DspPipeline>>>,
    watch_history: &Arc<watchhistory::WatchHistoryStore>,
    media_cache: &Arc<mediacache::MediaCacheStore>,
    bench: &StreamBenchmarker,
    trace: &Arc<TraceEmitter>,
    tag_job_store:    &Arc<mediacache::tag_write_job::JobStore>,
    tag_job_registry: &Arc<mediacache::tag_write_job::JobRegistry>,
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

        Request::Search(r)    => pipeline::search::run_search(engine, catalog, trace, config, r).await,

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
            } else {
                trace.disable();
            }
            Response::Ok
        }

        // Play and PlayerStop are handled earlier in the IPC loop before
        // reaching handle_line — these arms are unreachable in practice.
        Request::Play(_) | Request::PlayerStop => Response::Ok,

        // GetMpdOutputs is handled earlier in the IPC loop (needs async mpd.outputs()).
        Request::GetMpdOutputs => Response::error(None, ErrorCode::InvalidRequest, "use get_mpd_outputs message type".to_string()),

        // ── MPD library / browse pull requests ─────────────────────────────
        Request::MpdGetQueue(r) => match mpd {
            None => Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string()),
            Some(bridge) => match bridge.get_queue().await {
                Ok(tracks) => Response::MpdGetQueue(ipc::MpdGetQueueResponse { id: r.id, tracks }),
                Err(e) => Response::error(Some(r.id), ErrorCode::Internal, format!("mpd_error: {e}")),
            },
        },
        Request::MpdList(r) => match mpd {
            None => Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string()),
            Some(bridge) => {
                let result = match r.what.as_str() {
                    "artists" => bridge.list_artists().await.map(|a| ipc::MpdListResponse {
                        id: r.id.clone(), artists: a, albums: vec![], songs: vec![],
                    }),
                    "albums" => bridge.list_albums(&r.artist).await.map(|a| ipc::MpdListResponse {
                        id: r.id.clone(), artists: vec![], albums: a, songs: vec![],
                    }),
                    "songs" => bridge.list_songs(&r.artist, &r.album, &r.date).await.map(|s| ipc::MpdListResponse {
                        id: r.id.clone(), artists: vec![], albums: vec![], songs: s,
                    }),
                    other => {
                        return Response::error(
                            Some(r.id),
                            ErrorCode::InvalidRequest,
                            format!("mpd_list: unknown `what` value `{other}` (expected artists|albums|songs)"),
                        );
                    }
                };
                match result {
                    Ok(resp) => Response::MpdList(resp),
                    Err(e) => Response::error(Some(r.id), ErrorCode::Internal, format!("mpd_error: {e}")),
                }
            }
        },
        Request::MpdBrowse(r) => match mpd {
            None => Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string()),
            Some(bridge) => match bridge.browse(&r.path).await {
                Ok(entries) => Response::MpdBrowse(ipc::MpdBrowseResponse { id: r.id, entries }),
                Err(e) => Response::error(Some(r.id), ErrorCode::Internal, format!("mpd_error: {e}")),
            },
        },
        Request::MpdGetPlaylists(r) => match mpd {
            None => Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string()),
            Some(bridge) => match bridge.get_playlists().await {
                Ok(playlists) => Response::MpdGetPlaylists(ipc::MpdGetPlaylistsResponse { id: r.id, playlists }),
                Err(e) => Response::error(Some(r.id), ErrorCode::Internal, format!("mpd_error: {e}")),
            },
        },
        Request::MpdGetPlaylist(r) => match mpd {
            None => Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string()),
            Some(bridge) => match bridge.get_playlist_tracks(&r.name).await {
                Ok(tracks) => Response::MpdGetPlaylist(ipc::MpdGetPlaylistResponse { id: r.id, tracks }),
                Err(e) => Response::error(Some(r.id), ErrorCode::Internal, format!("mpd_error: {e}")),
            },
        },

        // ── DSP requests ─────────────────────────────────────────────────────────
        Request::GetDspStatus => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let pipeline = d.lock().await;
            let cfg = pipeline.config().await;
            Response::DspStatus(ipc::DspStatusResponse {
                enabled: cfg.enabled,
                output_sample_rate: cfg.output_sample_rate,
                resample_enabled: cfg.resample_enabled,
                dsd_to_pcm_enabled: cfg.dsd_to_pcm_enabled,
                convolution_enabled: cfg.convolution_enabled,
                convolution_bypass: cfg.convolution_bypass,
                active: pipeline.is_active(),
            })
        }
        Request::SetDspConfig(r) => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let mut pipeline = d.lock().await;
            let mut cfg = pipeline.config().await;
            if let Some(v) = r.enabled { cfg.enabled = v; }
            if let Some(v) = r.output_sample_rate { cfg.output_sample_rate = v; }
            if let Some(v) = r.upsample_ratio { cfg.upsample_ratio = v; }
            if let Some(v) = r.filter_type {
                cfg.filter_type = match v.as_str() {
                    "fast" => dsp::FilterType::Fast,
                    "slow" => dsp::FilterType::Slow,
                    _ => dsp::FilterType::Synchronous,
                };
            }
            if let Some(v) = r.resample_enabled { cfg.resample_enabled = v; }
            if let Some(v) = r.dsd_to_pcm_enabled { cfg.dsd_to_pcm_enabled = v; }
            if let Some(v) = r.output_mode {
                cfg.output_mode = match v.as_str() {
                    "dsd" => dsp::OutputMode::Dsd,
                    "dsd_to_pcm" => dsp::OutputMode::DsdToPcm,
                    _ => dsp::OutputMode::Pcm,
                };
            }
            if let Some(v) = r.convolution_enabled { cfg.convolution_enabled = v; }
            if let Some(v) = r.convolution_bypass { cfg.convolution_bypass = v; }
            if let Some(v) = r.buffer_size {
                if v == 0 {
                    return Response::error(None, ErrorCode::InvalidRequest, "buffer_size must be non-zero".to_string());
                }
                cfg.buffer_size = v;
            }
            pipeline.update_config(cfg).await;
            Response::DspConfigUpdated { success: true }
        }
        Request::LoadConvolutionFilter(r) => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let mut pipeline = d.lock().await;
            match pipeline.load_convolution_filter(&r.path) {
                Ok(()) => Response::ConvolutionFilterLoaded { success: true },
                Err(e) => Response::error(None, ErrorCode::Internal, e),
            }
        }
        Request::BindDspToMpd => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let cfg = d.lock().await.config().await;
            if cfg.output_target != OutputTarget::Mpd {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP output_target is not set to 'mpd'".to_string());
            }
            let mpd_config = "audio_output {\n    type \"pipewire\"\n    name \"STUI DSP\"\n}\n".to_string();
            Response::DspBoundToMpd { success: true, config: mpd_config }
        }
        Request::ListDspProfiles => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let profiles = d.lock().await.profile_store().list_profiles();
            Response::DspProfilesListed { profiles }
        }
        Request::SaveDspProfile(r) => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let mut pipeline = d.lock().await;
            let cfg = pipeline.config().await;
            // Preserve existing profile for rollback in case save fails
            let existing = pipeline.profile_store().get_profile(&r.name).cloned();
            let profile = crate::dsp::config::DspProfileConfig::from_config(&cfg, r.name.clone());
            pipeline.profile_store_mut().add_profile(r.name.clone(), profile);
            match pipeline.profile_store_mut().save() {
                Ok(_) => Response::DspProfileSaved { success: true },
                Err(e) => {
                    // Rollback: restore original or remove if new
                    match existing {
                        Some(orig) => { pipeline.profile_store_mut().add_profile(r.name.clone(), orig); }
                        None => { let _ = pipeline.profile_store_mut().remove_profile(&r.name); }
                    }
                    Response::error(None, ErrorCode::Internal, e)
                }
            }
        }
        Request::LoadDspProfile(r) => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let mut pipeline = d.lock().await;
            let mut cfg = pipeline.config().await;
            if pipeline.profile_store().apply_profile(&r.name, &mut cfg) {
                pipeline.update_config(cfg).await;
                Response::DspProfileLoaded { success: true }
            } else {
                Response::error(None, ErrorCode::InvalidRequest, format!("profile '{}' not found", r.name))
            }
        }
        Request::DeleteDspProfile(r) => {
            let Some(d) = dsp else {
                return Response::error(None, ErrorCode::InvalidRequest, "DSP not configured".to_string());
            };
            let mut pipeline = d.lock().await;
            let Some(backup) = pipeline.profile_store().get_profile(&r.name).cloned() else {
                return Response::error(None, ErrorCode::InvalidRequest, format!("profile '{}' not found", r.name));
            };
            if pipeline.profile_store_mut().remove_profile(&r.name) {
                match pipeline.profile_store_mut().save() {
                    Ok(_) => Response::DspProfileDeleted { success: true },
                    Err(e) => {
                        pipeline.profile_store_mut().add_profile(r.name.clone(), backup);
                        Response::error(None, ErrorCode::Internal, e)
                    }
                }
            } else {
                Response::error(None, ErrorCode::InvalidRequest, format!("profile '{}' not found", r.name))
            }
        }

        // ── Tag normalization ────────────────────────────────────────────────
        Request::MarkTagException(r) => {
            use mediacache::normalize::{self as norm, exceptions::ExceptionField};
            let field = match ExceptionField::from_str(&r.field) {
                Some(f) => f,
                None => return Response::error(
                    Some(r.id),
                    ErrorCode::InvalidRequest,
                    format!("unknown field `{}` (expected artist|album_artist|album|title|genre)", r.field),
                ),
            };
            let Some(store) = norm::store::global() else {
                return Response::error(
                    Some(r.id),
                    ErrorCode::Internal,
                    "exception store not initialized".to_string(),
                );
            };
            match store.add_user_exception(field, &r.raw_value) {
                Ok(added) => Response::MarkTagException(ipc::MarkTagExceptionResponse { id: r.id, added }),
                Err(e) => Response::error(
                    Some(r.id),
                    ErrorCode::Internal,
                    format!("add_user_exception failed: {e}"),
                ),
            }
        }

        Request::ActionATagsPreview(r) => {
            use mediacache::{
                normalize::{self as norm, NormalizationConfig},
                tag_write_job,
            };
            let Some(bridge) = mpd else {
                return Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string());
            };
            let cfg_snap = config.snapshot().await;
            let Some(music_dir) = cfg_snap.mpd.music_dir.clone() else {
                return Response::error(
                    Some(r.id),
                    ErrorCode::InvalidRequest,
                    "[mpd.music_dir] not configured — can't resolve tag file paths".to_string(),
                );
            };
            let raw_files = match bridge.gather_scope_files(&r.scope, &music_dir).await {
                Ok(f) => f,
                Err(e) => return Response::error(
                    Some(r.id), ErrorCode::Internal, format!("gather scope files: {e}"),
                ),
            };
            let exceptions = norm::store::global()
                .map(|s| s.get())
                .unwrap_or_default();
            let cfg = NormalizationConfig {
                enabled: true,
                use_lookup: cfg_snap.music.normalize.use_lookup,
                exceptions: &exceptions,
            };
            let lookups = std::collections::HashMap::new(); // v1: always empty
            let diff = tag_write_job::build_diff(raw_files, &cfg, &lookups);
            let total_files = diff.len();
            let rows = tag_write_job::to_wire_rows(&diff);
            let job_id = uuid::Uuid::new_v4().to_string();
            tag_job_store.insert(job_id.clone(), diff);
            Response::ActionATagsPreview(ipc::ActionATagsPreviewResponse {
                id: r.id,
                job_id,
                rows,
                total_files,
            })
        }

        Request::ActionATagsApply(r) => {
            use mediacache::tag_write_job;
            let Some(bridge) = mpd else {
                return Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string());
            };
            let Some(diff) = tag_job_store.take(&r.job_id) else {
                return Response::error(
                    Some(r.id),
                    ErrorCode::InvalidRequest,
                    format!("unknown job_id: {}", r.job_id),
                );
            };
            let cancel_flag = tag_job_registry.register(&r.job_id);
            let files_for_rescan: Vec<std::path::PathBuf> =
                diff.iter().map(|d| d.file.clone()).collect();
            let outcome = tag_write_job::apply(
                r.job_id.clone(),
                diff,
                cancel_flag,
                None, // v1: no streaming progress (see spec)
            ).await;
            tag_job_registry.done(&r.job_id);

            let rescan_path = tag_write_job::common_ancestor(&files_for_rescan)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if !rescan_path.is_empty() {
                if let Err(e) = bridge.update_library(Some(&rescan_path)).await {
                    warn!(error = %e, path = %rescan_path, "mpd rescan after tag write failed");
                }
            }

            let failed_count = outcome.failed.len();
            let failures = outcome.failed.iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            Response::ActionATagsApply(ipc::ActionATagsApplyResponse {
                id: r.id,
                succeeded: outcome.succeeded,
                failed: failed_count,
                skipped_cancelled: outcome.skipped_cancelled,
                failures,
                rescan_path,
            })
        }

        Request::ActionATagsCancel(r) => {
            let cancelled = tag_job_registry.cancel(&r.job_id);
            Response::ActionATagsCancel(ipc::ActionATagsCancelResponse {
                id: r.id,
                cancelled,
            })
        }
    }
}

