//! stui-runtime — async Rust backend for the stui TUI.
//!
//! Startup sequence:
//!   1. Scan ~/.stui/plugins/ and load all valid plugins
//!   2. Start the filesystem watcher for hot-reload
//!   3. Start the catalog (cache-first grid population)
//!   4. Enter the IPC request/response loop

mod abi;
// Scheduled for removal in Task 9 of the librqbit migration.
#[allow(dead_code)]
mod aria2_bridge;
mod cache;
mod catalog;
mod catalog_engine;
mod config;
mod discovery;
mod engine;
mod error;
mod events;
mod fanart;
mod ipc;
mod lastfm;
mod logging;
mod mdblist;
mod media;
mod mpd_bridge;
mod player;
mod plugin;
mod providers;
mod quality;
mod rating_aggregator;
mod resolver;
mod sandbox;
mod scraper;
mod stremio;
// Scheduled for removal in Task 9 of the librqbit migration.
#[allow(dead_code)]
mod streamer;
mod torrent_engine;
mod tvdb;
mod anime_bridge;
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

use anyhow::{Context, Result};
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
use storage::download_translator::DownloadTranslator;

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
    let args: Vec<String> = std::env::args().collect();

    // `stui-runtime cache <stats|clear|inspect|list>` — admin CLI for the
    // on-disk response cache. Operates on `~/.cache/stui/response.db`
    // directly; no running daemon required. Writes (clear) while the daemon
    // is running are safe (SQLite WAL) but new rows may arrive from plugin
    // searches seconds later.
    //
    // Short-circuits BEFORE logging init so the CLI output isn't polluted
    // with tracing preamble. Tracing is wired inside the subcommand at
    // `error` level for anything that genuinely needs to surface.
    if args.get(1).map(|a| a.as_str()) == Some("cache") {
        logging::init_with_level("error");
        return run_cache_subcommand(&args);
    }

    // Logging is initialised after config load so the log level comes from
    // stui.toml (with STUI_LOG env var override). We use a temporary default
    // here and re-initialise below once config is loaded.
    logging::init_with_level("info");

    // ── Mode detection ────────────────────────────────────────────────────
    // `stui-runtime`            → stdin/stdout mode (launched by TUI process)
    // `stui-runtime daemon`     → Unix socket daemon mode
    // `stui-runtime daemon --socket /path/to/sock`

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

    // One-shot migration of any legacy ~/.stui/ tree to the
    // XDG-compliant split (~/.config/stui/ for config + plugins +
    // data; ~/.cache/stui/ for caches). Idempotent: re-runs are
    // no-ops, and any path whose new home already exists is left
    // alone — we never overwrite. Caches don't need migration.
    config::migrate::migrate_legacy_paths();

    // Load config from ~/.config/stui/runtime.toml + STUI_* env overrides
    let mut cfg = config::load();

    // Push the user's rating-source weights into the catalog
    // aggregator's process-wide overlay. apply_weighted_rating reads
    // this overlay on every invocation so all subsequent enrichment
    // passes see the current weights. A future IPC config_update
    // path will call set_user_rating_weights again to apply changes
    // live without restart (see SCAFFOLD_TODOS §26).
    catalog_engine::aggregator::set_user_rating_weights(cfg.rating_weights.clone());

    // Auto-detect MPD paths from mpd.conf if not set in stui.toml.
    {
        let mpd_paths = mpd_bridge::mpd_conf::detect();
        if cfg.mpd.music_dir.is_none() {
            if let Some(p) = mpd_paths.music_directory {
                info!(path = %p.display(), "auto-detected music_directory from mpd.conf");
                cfg.mpd.music_dir = Some(p);
            }
        }
        if cfg.mpd.playlist_dir.is_none() {
            if let Some(p) = mpd_paths.playlist_directory {
                info!(path = %p.display(), "auto-detected playlist_directory from mpd.conf");
                cfg.mpd.playlist_dir = Some(p);
            }
        }
    }

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
    let mut engine = Engine::new(
        cfg.cache_dir.clone(),
        cfg.data_dir.clone(),
        cfg.catalog.anime_ratio,
        cfg.plugins.clone(),
    );
    engine.set_mdblist_lists(cfg.mdblist.clone());
    let engine = Arc::new(engine);

    // Spawn the anime-bridge background refresh task. Must run AFTER the
    // tokio runtime is up (we're inside `#[tokio::main]` here) — the bundled
    // snapshot is already loaded synchronously in `Engine::new`. Refresh is
    // a best-effort enhancement: if the HTTP client fails to init or upstream
    // is down the daemon keeps running on the bundled snapshot.
    engine.start_anime_bridge_refresh(engine.cache_dir().to_path_buf());

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

    // ── Catalog + cached-grid emission (runs in parallel with plugin load) ──
    // Constructed + started BEFORE `discovery.scan_and_load().await?` below,
    // which takes 3–5s for 7 WASM plugins. The disk grid cache at
    // ~/.stui/cache/grid/{tab}.json doesn't need any plugin — it's just a
    // file read + broadcast — so waiting for plugin init to finish before
    // emitting makes the UI feel slow even when the cache is fresh.
    //
    // Race window: if a tab's disk cache is TTL-stale, catalog.init_tab
    // calls refresh_tab → search_catalog_entries → needs plugins. When
    // plugins haven't finished loading yet, the registry is empty and the
    // refresh returns no new entries. That's fine: the cached grid was
    // already emitted, and the user can hit `R` once plugins are live.
    let catalog = Arc::new(Catalog::new(cfg.cache_dir.clone(), Arc::clone(&engine)));

    // Persistent subscriber that keeps media_cache in sync with the catalog
    // broadcast regardless of whether a client is connected. Without this, any
    // GridUpdate emitted while the UI is still starting (e.g. the startup
    // cache-serve broadcast) is dropped and `GetMediaCacheTab("movies")`
    // returns nothing — leaving Movies empty until the user manually searches.
    // Subscribe BEFORE spawning `catalog.start()` so we catch its first
    // per-tab emission.
    {
        let mut rx = catalog.subscribe();
        let mc = media_cache.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(u) => {
                        let entries: Vec<ipc::MediaEntry> =
                            u.entries.iter().map(catalog_entry_to_media_entry).collect();
                        mc.save_tab(u.tab, entries).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "media_cache sync subscriber lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    { let c = Arc::clone(&catalog); tokio::spawn(async move { c.start().await }); }

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
    // Unblock any catalog refresh_tab calls that were waiting on us.
    // init_tab emitted its cached grid synchronously; this signal releases
    // the live-refresh step so the tabs that need a network fan-out finally
    // run it against a populated plugin registry.
    catalog.mark_plugins_ready();
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
    // Catalog creation + cached-grid emission spawn moved above, to run
    // concurrently with `discovery.scan_and_load().await?`.

    // Periodic cache eviction. Every 5 minutes we walk the in-memory TTL
    // caches (SearchCache, MetadataCache) and drop expired entries so a
    // long-running daemon doesn't accumulate them indefinitely. The caches
    // short-circuit on read when entries are stale already, so missing a
    // cycle is harmless — this just bounds memory.
    {
        let e = Arc::clone(&engine);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5 * 60));
            // First tick fires immediately; skip it so we don't run before
            // any caches have content.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                e.cache.search.evict_expired().await;
                e.cache.metadata.evict_expired().await;
            }
        });
    }

    // Disk purge (Phase 2). Expired rows in ~/.cache/stui/response.db stay
    // until deleted explicitly; physical deletion reclaims disk space. Runs
    // once at boot (so a multi-day-stopped daemon cleans up its own stale
    // rows without waiting 24h) and daily thereafter.
    if let Some(disk) = engine.cache.disk.clone() {
        tokio::spawn(async move {
            match disk.purge_expired() {
                Ok(n) if n > 0 => info!(deleted = n, "response cache: purged expired rows at boot"),
                Ok(_) => {}
                Err(e) => warn!(err = %e, "response cache: boot purge failed"),
            }
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
            ticker.tick().await; // skip immediate first tick
            loop {
                ticker.tick().await;
                match disk.purge_expired() {
                    Ok(n) if n > 0 => info!(deleted = n, "response cache: daily purge"),
                    Ok(_) => {}
                    Err(e) => warn!(err = %e, "response cache: daily purge failed"),
                }
            }
        });
    }

    // ── Shared event channel (aria2 + mpv/player → Go) ──────────────────
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<String>(128);

    // ── aria2c bridge ─────────────────────────────────────────────────────
    // Initialize the aria2 translator for path organization
    let translator = DownloadTranslator::new(
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

    // ── Embedded torrent engine (librqbit) ────────────────────────────────
    // Replaces the prior external aria2 daemon. Task 10 will polish this
    // (config knob for staging dir, telemetry, etc.); for Task 8 we just
    // need it constructed before PlayerBridge::new.
    let torrent_staging_dir = cfg.cache_dir.join("torrents");
    std::fs::create_dir_all(&torrent_staging_dir)?;
    let torrents = Arc::new(
        torrent_engine::TorrentEngine::new(torrent_staging_dir.clone())
            .await
            .map_err(|e| anyhow::anyhow!("torrent_engine boot failed: {e}"))?,
    );
    info!(staging = %torrent_staging_dir.display(), "torrent_engine booted");

    // ── mpv / player bridge ───────────────────────────────────────────────
    let player = player::PlayerBridge::new(
        Arc::clone(&engine),
        Arc::clone(&config),
        Arc::clone(&torrents),
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
    let mut stale_rx = catalog.subscribe_stale();

    // Replay the latest known entries for each tab to the freshly-connected
    // client. catalog.start() fires its cached-grid broadcast a few hundred
    // milliseconds before the TUI's IPC reader attaches; without this replay
    // the client would only see live refreshes (which require plugins to be
    // loaded), and the grid would stay populated from whatever stale data
    // was in ~/.config/stui/mediacache.json until the first refresh.
    {
        let snapshot = catalog.snapshot_all().await;
        for u in snapshot {
            let entries: Vec<ipc::MediaEntry> =
                u.entries.iter().map(catalog_entry_to_media_entry).collect();
            let source = match u.source {
                catalog::GridUpdateSource::Cache => "cache".to_string(),
                catalog::GridUpdateSource::Live  => "live".to_string(),
            };
            let msg = GridUpdateMsg { tab: u.tab, entries, source };
            send_wire(&mut writer, &msg.to_wire()?).await?;
        }
    }

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
                                let engine_c = Arc::clone(&engine);
                                let tx = event_tx.clone();
                                tokio::spawn(async move {
                                    let resp = pipeline::registry::run_browse_registry(&config_c, &engine_c).await;
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
                        let resp = handle_line(&engine, &catalog, health, config, player, mpd, dsp, watch_history, media_cache, &bench, &trace, &tag_job_store, &tag_job_registry, event_tx.clone(), &line).await;
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
                        let entries: Vec<ipc::MediaEntry> =
                            u.entries.iter().map(catalog_entry_to_media_entry).collect();

                        // Persistence into media_cache now happens in a separate
                        // daemon-level subscriber (see the `catalog.subscribe()`
                        // task spawned near media_cache init). That subscriber
                        // runs even when no client is connected, so the startup
                        // cache broadcast no longer gets dropped on the floor.

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

            // Catalog refresh attempted but got zero entries — forward to
            // the TUI as a `catalog_stale` event so it can surface an
            // "Offline — showing cached" status line.
            stale = stale_rx.recv() => {
                match stale {
                    Ok(s) => {
                        let ev = ipc::CatalogStaleMsg {
                            tab: s.tab,
                            reason: s.reason,
                        };
                        send_wire(&mut writer, &ev.to_wire()?).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Lost a stale notification — non-critical. Next
                        // refresh failure will generate a new one.
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Catalog dropped — runtime shutting down.
                    }
                }
            }
        }
    }

    Ok(())
}

/// Admin CLI for the on-disk response cache.
///
///   stui-runtime cache stats                          — per-namespace summary
///   stui-runtime cache clear [--namespace <ns>]       — wipe rows
///   stui-runtime cache list <namespace> [--limit N]   — enumerate keys
///   stui-runtime cache inspect <namespace> <key>      — pretty-print a value
fn run_cache_subcommand(args: &[String]) -> Result<()> {
    let usage = "\
usage: stui-runtime cache <subcommand>
  stats                                 summary of every namespace
  clear [--namespace <ns>]              wipe all rows, or just one namespace
  list <namespace> [--limit N]          list keys (default limit 20)
  inspect <namespace> <key>             pretty-print the cached value";

    let verb = args
        .get(2)
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!(usage))?;

    let path = cache::default_cache_db_path();
    let kv = cache::SqliteKv::open(&path)
        .with_context(|| format!("opening cache DB at {}", path.display()))?;

    match verb {
        "stats" => cmd_cache_stats(&kv, &path),
        "clear" => cmd_cache_clear(&kv, &args[3..]),
        "list" => cmd_cache_list(&kv, &args[3..]),
        "inspect" => cmd_cache_inspect(&kv, &args[3..]),
        _ => anyhow::bail!("unknown verb '{verb}'\n\n{usage}"),
    }
}

fn cmd_cache_stats(kv: &cache::SqliteKv, path: &std::path::Path) -> Result<()> {
    let stats = kv.namespace_stats()?;
    println!("cache: {}", path.display());
    if stats.is_empty() {
        println!("(empty)");
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    println!(
        "{:<14} {:>7} {:>10}   {:<20} {:<20}",
        "NAMESPACE", "ROWS", "SIZE", "OLDEST EXPIRES IN", "NEWEST EXPIRES IN"
    );
    for s in stats {
        println!(
            "{:<14} {:>7} {:>10}   {:<20} {:<20}",
            s.namespace,
            s.rows,
            format_bytes(s.total_bytes),
            s.oldest_expiry
                .map(|t| format_duration_signed(t - now))
                .unwrap_or_else(|| "-".into()),
            s.newest_expiry
                .map(|t| format_duration_signed(t - now))
                .unwrap_or_else(|| "-".into()),
        );
    }
    Ok(())
}

fn cmd_cache_clear(kv: &cache::SqliteKv, rest: &[String]) -> Result<()> {
    let mut ns: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--namespace" {
            ns = rest.get(i + 1).map(|s| s.as_str());
            i += 2;
        } else {
            anyhow::bail!("unknown flag '{}'", rest[i]);
        }
    }
    let deleted = if let Some(ns) = ns {
        let n = kv.clear_namespace(ns)?;
        eprintln!("✓ cleared {n} rows from namespace '{ns}'");
        n
    } else {
        let n = kv.clear_all()?;
        eprintln!("✓ cleared {n} rows (all namespaces)");
        n
    };
    let _ = deleted;
    Ok(())
}

fn cmd_cache_list(kv: &cache::SqliteKv, rest: &[String]) -> Result<()> {
    let ns = rest
        .first()
        .ok_or_else(|| anyhow::anyhow!("usage: cache list <namespace> [--limit N]"))?;
    let mut limit: usize = 20;
    let mut i = 1;
    while i < rest.len() {
        if rest[i] == "--limit" {
            limit = rest
                .get(i + 1)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("--limit expects a positive integer"))?;
            i += 2;
        } else {
            anyhow::bail!("unknown flag '{}'", rest[i]);
        }
    }
    let keys = kv.list_keys(ns, limit)?;
    if keys.is_empty() {
        eprintln!("(no keys in namespace '{ns}')");
        return Ok(());
    }
    for k in keys {
        println!("{k}");
    }
    Ok(())
}

fn cmd_cache_inspect(kv: &cache::SqliteKv, rest: &[String]) -> Result<()> {
    let ns = rest
        .first()
        .ok_or_else(|| anyhow::anyhow!("usage: cache inspect <namespace> <key>"))?;
    let key = rest
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("usage: cache inspect <namespace> <key>"))?;
    match kv.get(ns, key) {
        Some(bytes) => {
            // Attempt JSON pretty-print; fall back to lossy UTF-8 dump.
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("{}", String::from_utf8_lossy(&bytes));
            }
            Ok(())
        }
        None => {
            eprintln!("(no entry for {ns}/{key} — either absent or expired)");
            std::process::exit(1);
        }
    }
}

fn format_bytes(n: i64) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Human-readable duration: positive = time until, negative = time since.
fn format_duration_signed(secs: i64) -> String {
    let abs = secs.unsigned_abs();
    let s = if abs < 60 {
        format!("{}s", abs)
    } else if abs < 3600 {
        format!("{}m", abs / 60)
    } else if abs < 86_400 {
        format!("{}h {}m", abs / 3600, (abs % 3600) / 60)
    } else {
        format!("{}d {}h", abs / 86_400, (abs % 86_400) / 3600)
    };
    if secs < 0 {
        format!("-{s} (expired)")
    } else {
        format!("in {s}")
    }
}

fn catalog_entry_to_media_entry(e: &catalog::CatalogEntry) -> ipc::MediaEntry {
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
        tab, media_type: e.media_type,
        imdb_id: e.imdb_id.clone(),
        tmdb_id: e.tmdb_id.clone(),
        mal_id: e.mal_id.clone(),
        // CatalogEntry doesn't carry anilist/kitsu ids — those are
        // pre-merge enrichment fodder, dropped before the wire shape.
        // The TUI only needs imdb/tmdb/mal for display routing.
        anilist_id: None,
        kitsu_id: None,
        original_language: e.original_language.clone(),
        kind: Default::default(),
        source: String::new(),
        artist_name: e.artist.clone(),
        album_name: None,
        track_number: None,
        season: None,
        episode: None,
        season_count: None,
    }
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

// ── Episodes verb dispatch ────────────────────────────────────────────────────

/// Resolve the plugin id that owns this entry. Empty `id_source` peels a
/// `<provider>-<rest>` prefix from `entry_id`, falling back to `"tmdb"`
/// since that is the only provider whose `episodes()` is wired today.
fn id_source_for_episodes(entry_id: &str, id_source: &str) -> String {
    if !id_source.is_empty() {
        return id_source.to_string();
    }
    if let Some((p, _)) = entry_id.split_once('-') {
        if matches!(p, "tmdb" | "tvdb" | "anilist" | "kitsu" | "imdb" | "omdb") {
            return p.to_string();
        }
    }
    "tmdb".to_string()
}

/// Strip the `"<provider>-"` prefix from a stui composite id, leaving the
/// provider-native id the plugin's `episodes()` expects in `series_id`.
fn strip_provider_prefix<'a>(entry_id: &'a str, provider: &str) -> &'a str {
    let pat = format!("{provider}-");
    entry_id.strip_prefix(&pat).unwrap_or(entry_id)
}

/// Handle a `Metadata { kind = "episodes" }` request: route to the resolved
/// plugin's `stui_episodes` verb and emit an `EpisodesLoaded` response (or
/// a structured `Error` envelope on failure). The TUI's `LoadEpisodes`
/// path consumes both shapes.
async fn run_load_episodes(engine: &Arc<Engine>, r: ipc::MetadataRequest) -> Response {
    if r.season < 1 {
        warn!("load_episodes: season must be >= 1");
        return Response::error(
            Some(r.id),
            ipc::ErrorCode::InvalidRequest,
            "season must be >= 1".to_string(),
        );
    }

    let id_source = id_source_for_episodes(&r.entry_id, &r.id_source);
    let series_id = strip_provider_prefix(&r.entry_id, &id_source).to_string();

    // TVDB is a runtime-native provider, not a WASM plugin — short-circuit
    // before the supervisor dispatch, since `engine.supervisor_episodes`
    // would have no plugin to call for it.
    if id_source == "tvdb" {
        return run_load_episodes_tvdb(engine, r.id, &series_id, r.season).await;
    }

    let req = crate::abi::types::EpisodesRequest {
        series_id,
        id_source: id_source.clone(),
        season: r.season,
    };

    info!(plugin = %id_source, season = r.season, "load_episodes: dispatching to plugin");

    match engine.supervisor_episodes(&id_source, req, crate::engine::CallPriority::Foreground).await {
        Ok(episodes) => {
            info!(plugin = %id_source, episodes = episodes.len(), "load_episodes: plugin returned");
            let wire: Vec<ipc::EpisodeEntryWire> = episodes.into_iter().map(Into::into).collect();
            // Truncated preview keeps the log line bounded; full data is on the wire.
            let preview = serde_json::to_string(&serde_json::json!({ "episodes": &wire }))
                .unwrap_or_default();
            let preview = if preview.len() > 200 { &preview[..200] } else { &preview[..] };
            info!(preview = %preview, "ipc episodes wire preview");
            Response::EpisodesLoaded(ipc::EpisodesLoadedResponse {
                id: r.id,
                episodes: wire,
            })
        }
        Err(err) => {
            warn!(plugin = %id_source, err = %err, "load_episodes: primary failed, trying fallback");
            // Fallback chain: when the primary plugin (typically TMDB)
            // fails, retry against TVDB if the catalog entry carried a
            // TVDB id. Skipped when TMDB is itself the primary source
            // we just failed against — that path was already taken at
            // the top of this function.
            if id_source != "tvdb" {
                if let Some(tvdb_id) = r.external_ids.get("tvdb").filter(|s| !s.is_empty()) {
                    info!(tvdb_id = %tvdb_id, "load_episodes: falling back to TVDB");
                    let resp = run_load_episodes_tvdb(engine, r.id.clone(), tvdb_id, r.season).await;
                    if matches!(resp, Response::EpisodesLoaded(_)) {
                        return resp;
                    }
                    warn!("load_episodes: TVDB fallback also failed");
                }
            }
            // All providers exhausted — surface a friendly message.
            // Raw `err.to_string()` includes the upstream HTTP URL with
            // query string; `Response::error` runs sanitize_secrets so
            // any api_key=... is redacted before reaching the wire.
            Response::error(
                Some(r.id),
                ipc::ErrorCode::MetadataFailed,
                format!(
                    "Could not load episodes from {} (and TVDB fallback unavailable). Check network or API keys.",
                    id_source
                ),
            )
        }
    }
}

/// Runtime-native counterpart to `run_load_episodes`. TVDB ships in the
/// runtime binary so its episodes verb bypasses the WASM supervisor and
/// goes straight to `TvdbClient`. Surfaces the same `EpisodesLoaded` /
/// `Error` envelopes so the TUI's `LoadEpisodes` consumer is unaware of
/// the dispatch split.
async fn run_load_episodes_tvdb(
    engine: &Arc<Engine>,
    request_id: String,
    series_id: &str,
    season: u32,
) -> Response {
    let Some(client) = engine.tvdb() else {
        warn!("load_episodes (tvdb): client unavailable — no api key");
        return Response::error(
            Some(request_id),
            ipc::ErrorCode::MetadataFailed,
            "tvdb is not configured (no API key)".to_string(),
        );
    };
    info!(provider = "tvdb", season = season, "load_episodes: dispatching to runtime-native tvdb");
    match client.episodes(series_id, season).await {
        Ok(eps) => {
            info!(provider = "tvdb", episodes = eps.len(), "load_episodes: tvdb returned");
            let wire: Vec<ipc::EpisodeEntryWire> = eps.into_iter().map(Into::into).collect();
            Response::EpisodesLoaded(ipc::EpisodesLoadedResponse {
                id: request_id,
                episodes: wire,
            })
        }
        Err(err) => {
            warn!(provider = "tvdb", err = %err, "load_episodes: tvdb call failed");
            Response::error(
                Some(request_id),
                ipc::ErrorCode::MetadataFailed,
                format!("tvdb episodes: {err}"),
            )
        }
    }
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
    event_tx: ipc::EventSender,
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

        Request::SetPluginEnabled(r) => match engine.set_plugin_enabled(&r.plugin_id, r.enabled).await {
            Ok(resp) => resp,
            Err(e) => Response::error(None, ErrorCode::PluginNotFound, e.to_string()),
        },

        Request::CatalogRefresh(r) => {
            // Manual refresh: wipe mem AND disk SearchCache so the next
            // provider fan-out actually hits the network. Mem-only clear
            // left disk-tier rows that would warm mem back up inside
            // search_catalog_entries, short-circuiting the refresh — which
            // violates the IPC contract ("as if TTL had expired"). Result
            // flows back to the TUI via the existing GridUpdate broadcast;
            // we ack here with Ok so the caller knows the request landed.
            engine.cache.search.clear_all().await;
            let catalog_c = Arc::clone(catalog);
            let tab = r.tab.clone();
            tokio::spawn(async move {
                catalog_c.refresh_tab(tab).await;
            });
            Response::Ok
        }

        Request::Search(r)    => {
            // Fire-and-forget: results stream back as Event::ScopeResults messages
            // keyed by query_id.  Response::SearchResult (synchronous path) has
            // no Rust-side producers after Engine::search retirement (Task 7.0 #3).
            let engine_c   = Arc::clone(engine);
            let event_tx_c = event_tx.clone();
            let query_id = r.query_id;
            tokio::spawn(async move {
                pipeline::search::run_search((*engine_c).clone(), r, event_tx_c).await;
                tracing::debug!(query_id, "search spawn finished");
            });
            // No synchronous response — streaming scope_results events carry the
            // results.  Return Ok so the IPC loop doesn't write a SearchResult
            // placeholder.  The TUI (Task 4.3) will be rewritten to consume
            // ScopeResults events instead of a synchronous SearchResult.
            Response::Ok
        }

        Request::Resolve(r)   => engine.resolve(&r.id, &r.entry_id, &r.provider).await,

        Request::GetStreams(r) => pipeline::resolve::run_get_streams(engine, catalog, config, health, bench, trace, event_tx.clone(), r).await,

        Request::Metadata(r) => {
            if r.kind == "episodes" {
                run_load_episodes(engine, r).await
            } else {
                Response::error(
                    Some(r.id), ErrorCode::MetadataFailed,
                    "Metadata plugins not yet implemented".to_string(),
                )
            }
        }

        Request::GetDetailMetadata(r) => {
            // Fire-and-forget: four verb partials stream back via
            // `event_tx` as `Response::DetailMetadataPartial` wire lines.
            // No synchronous response body — return Ok so the IPC loop
            // doesn't write a placeholder.
            let engine_c = Arc::clone(engine);
            let event_tx_c = event_tx.clone();
            let cfg_snapshot = config.snapshot().await;
            tokio::spawn(async move {
                let mut req = engine::metadata::DetailMetadataRequest::from_wire(r);
                req.per_verb_timeout = std::time::Duration::from_millis(
                    cfg_snapshot.metadata.per_verb_timeout_ms,
                );
                let dispatch = engine::metadata::EngineMetadataDispatch::new(
                    engine_c,
                    cfg_snapshot.metadata.sources.clone(),
                )
                .await;
                // Small buffer — the orchestrator emits at most 4 partials
                // and the drain task pulls them off immediately.
                let (p_tx, mut p_rx) =
                    tokio::sync::mpsc::channel::<engine::metadata::DetailMetadataPartial>(4);
                let drain = tokio::spawn(async move {
                    while let Some(partial) = p_rx.recv().await {
                        let resp = Response::DetailMetadataPartial(partial.into_wire());
                        match resp.to_wire() {
                            Ok(line) => {
                                if event_tx_c.send(line).await.is_err() {
                                    // Client hung up — nothing to do.
                                    break;
                                }
                            }
                            Err(e) => warn!(err = %e, "serialize DetailMetadataPartial failed"),
                        }
                    }
                });
                engine::metadata::fetch_detail_metadata(dispatch, req, p_tx).await;
                // `fetch_detail_metadata` returns once every verb has
                // emitted (or the receiver was dropped). Joining the
                // drain is optional — the channel close lets it exit.
                let _ = drain.await;
            });
            Response::Ok
        }

        Request::PlayerCommand(r) => pipeline::playback::run_player_command(player, r).await,

        Request::Cmd(cmd) => pipeline::playback::run_player_cmd(player, mpd, cmd).await,

        Request::SetConfig(r)         => pipeline::config::run_set_config(config, engine, r).await,
        Request::GetProviderSettings  => pipeline::config::run_get_provider_settings(engine, config).await,
        Request::GetPluginRepos       => pipeline::config::run_get_plugin_repos(config).await,
        Request::SetPluginRepos(r)    => pipeline::config::run_set_plugin_repos(config, r).await,
        Request::BrowseRegistry       => pipeline::registry::run_browse_registry(config, engine).await,
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

        Request::MpdSearch(r) => match mpd {
            None => Response::error(Some(r.id), ErrorCode::Internal, "MPD not available".to_string()),
            Some(bridge) => {
                let result = bridge.search(r).await;
                Response::MpdSearch(result)
            }
        },

        // ── lastfm direct fetchers ───────────────────────────────────────────────
        // Used by the Music Browse → AlbumDetail flow. Hit lastfm
        // directly because the WASM plugin can't surface tracks
        // through the current SDK shape (PluginEntry has no
        // tracks-on-album field). When the SDK gains that, this
        // can move into a plugin verb call.
        Request::LastfmAlbumTracks(r) => {
            match crate::lastfm::album_tracks::fetch(&r.artist, &r.album).await {
                Ok(tracks) => Response::LastfmAlbumTracks(ipc::LastfmAlbumTracksResponse {
                    id: r.id,
                    artist: r.artist,
                    album: r.album,
                    tracks,
                }),
                Err(e) => Response::error(
                    Some(r.id),
                    ErrorCode::Internal,
                    format!("lastfm album.getInfo: {e}"),
                ),
            }
        }

        // ── Metadata sources discovery ──────────────────────────────────────────
        // The Settings → Metadata Sources screen calls this to populate
        // its per-kind row list. Returns priority + discovered + disabled
        // separately so the UI can render status chips and decide what
        // toggling means (move to/from disabled list, push via set_config).
        Request::MetadataPluginsForKind(r) => {
            // Snapshot the user's current priority/disabled lists from
            // the live config (not the on-disk one — set_config edits
            // happen in-memory and flush async).
            let cfg = config.snapshot().await;
            let (priority, disabled) = match r.kind.as_str() {
                "movies" => (cfg.metadata.sources.movies.clone(), cfg.metadata.sources.movies_disabled.clone()),
                "series" => (cfg.metadata.sources.series.clone(), cfg.metadata.sources.series_disabled.clone()),
                "anime"  => (cfg.metadata.sources.anime.clone(),  cfg.metadata.sources.anime_disabled.clone()),
                "music"  => (cfg.metadata.sources.music.clone(),  cfg.metadata.sources.music_disabled.clone()),
                other => {
                    return Response::error(
                        Some(r.id),
                        ErrorCode::InvalidRequest,
                        format!("metadata_plugins_for_kind: unknown kind '{other}' (expected movies/series/anime/music)"),
                    );
                }
            };

            // Use a freshly-built probe to enumerate discovered plugins.
            // This walks the live registry so the result reflects any
            // plugins that just hot-loaded from the watcher.
            let probe = crate::engine::metadata::ManifestCapabilityProbe::from_engine(engine).await;
            // Discover for the Enrich verb — it's the most commonly
            // declared metadata verb, and the settings screen treats
            // "supports any of the four detail verbs" as the inclusion
            // signal. A future refinement could OR across all four verbs.
            use crate::cache::metadata_key::MetadataVerb;
            use crate::engine::metadata::sources::SourceCapabilityProbe;
            let mut discovered: Vec<String> =
                probe.discover(MetadataVerb::Enrich, &r.kind);
            // Drop anything already in the priority list — discovered is
            // the "auto-included tail", not a duplicate of priority.
            let priority_set: std::collections::HashSet<&str> =
                priority.iter().map(|s| s.as_str()).collect();
            discovered.retain(|d| !priority_set.contains(d.as_str()));

            Response::MetadataPluginsForKind(ipc::MetadataPluginsForKindResponse {
                id: r.id,
                kind: r.kind,
                priority,
                discovered,
                disabled,
            })
        }

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
        Request::GetAlbumArt(r) => {
            info!(file = %r.file, "GetAlbumArt request received");
            let music_dir = config.snapshot().await.mpd.music_dir.clone();
            let path = match music_dir {
                Some(dir) => {
                    let audio_path = dir.join(&r.file);
                    // spawn_blocking: lofty does sync file I/O that would
                    // block the IPC loop and cause timeouts.
                    tokio::task::spawn_blocking(move || {
                        mediacache::album_art::extract(&audio_path)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default()
                    }).await.unwrap_or_default()
                }
                None => String::new(),
            };
            Response::GetAlbumArt(ipc::GetAlbumArtResponse { id: r.id, path })
        }

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

        // ── Plugin verb dispatch ──────────────────────────────────────────────

        Request::Lookup(req) => {
            let ipc::LookupIpcRequest { query_id, plugin, inner } = req;
            match engine.supervisor_lookup(&plugin, inner, crate::engine::CallPriority::Foreground).await {
                Ok(entry) => {
                    tracing::debug!(query_id, plugin = %plugin, verb = "lookup", "plugin verb dispatch ok");
                    Response::Lookup(ipc::LookupIpcResponse { query_id, entry })
                }
                Err(e) => Response::error(Some(query_id.to_string()), ErrorCode::Internal, e.to_string()),
            }
        }

        Request::Enrich(req) => {
            let ipc::EnrichIpcRequest { query_id, plugin, inner } = req;
            match engine.supervisor_enrich(&plugin, inner, crate::engine::CallPriority::Foreground).await {
                Ok(entry) => {
                    tracing::debug!(query_id, plugin = %plugin, verb = "enrich", "plugin verb dispatch ok");
                    Response::Enrich(ipc::EnrichIpcResponse { query_id, entry })
                }
                Err(e) => Response::error(Some(query_id.to_string()), ErrorCode::Internal, e.to_string()),
            }
        }

        Request::GetArtwork(req) => {
            let ipc::ArtworkIpcRequest { query_id, plugin, inner } = req;
            match engine.supervisor_get_artwork(&plugin, inner, crate::engine::CallPriority::Foreground).await {
                Ok(inner) => {
                    tracing::debug!(query_id, plugin = %plugin, verb = "get_artwork", "plugin verb dispatch ok");
                    Response::GetArtwork(ipc::ArtworkIpcResponse { query_id, inner })
                }
                Err(e) => Response::error(Some(query_id.to_string()), ErrorCode::Internal, e.to_string()),
            }
        }

        Request::GetCredits(req) => {
            let ipc::CreditsIpcRequest { query_id, plugin, inner } = req;
            match engine.supervisor_get_credits(&plugin, inner, crate::engine::CallPriority::Foreground).await {
                Ok(inner) => {
                    tracing::debug!(query_id, plugin = %plugin, verb = "get_credits", "plugin verb dispatch ok");
                    Response::GetCredits(ipc::CreditsIpcResponse { query_id, inner })
                }
                Err(e) => Response::error(Some(query_id.to_string()), ErrorCode::Internal, e.to_string()),
            }
        }

        Request::Related(req) => {
            let ipc::RelatedIpcRequest { query_id, plugin, inner } = req;
            match engine.supervisor_related(&plugin, inner, crate::engine::CallPriority::Foreground).await {
                Ok(items) => {
                    tracing::debug!(query_id, plugin = %plugin, verb = "related", "plugin verb dispatch ok");
                    Response::Related(ipc::RelatedIpcResponse { query_id, items })
                }
                Err(e) => Response::error(Some(query_id.to_string()), ErrorCode::Internal, e.to_string()),
            }
        }
    }
}

