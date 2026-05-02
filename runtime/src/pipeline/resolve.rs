//! Stream-resolution pipeline — rank candidates and map to wire types.

use std::sync::Arc;
use std::collections::{HashMap, HashSet};

use futures::stream::{FuturesUnordered, StreamExt};

use crate::catalog::Catalog;
use crate::config::ConfigManager;
use crate::engine::{CallPriority, Engine, TraceEmitter};
use crate::ipc::{
    GetStreamsRequest, Response, StreamInfoWire, StreamsCompleteWire,
    StreamsPartialWire, StreamsResponse,
};
use crate::providers::{HealthRegistry, StreamBenchmarker};

/// Convert a plugin-side `Stream` (rich shape from
/// `StreamProvider::find_streams`) into the runtime's internal
/// `providers::Stream` shape used by the ranker / benchmarker. Maps
/// the optional-quality string into the typed `StreamQuality` enum
/// and detects torrent-vs-https from the URL scheme to set
/// `protocol` correctly so downstream playback can pick the right
/// transport (aria2 vs mpv-direct).
fn plugin_stream_to_provider(s: crate::abi::types::Stream) -> crate::providers::Stream {
    let quality_label = s.quality.clone().unwrap_or_else(|| "Unknown".to_string());
    let protocol = if s.url.starts_with("magnet:") {
        Some("torrent".to_string())
    } else if s.url.starts_with("https://") {
        Some("https".to_string())
    } else if s.url.starts_with("http://") {
        Some("http".to_string())
    } else {
        None
    };
    let hdr = if s.hdr {
        crate::providers::HdrFormat::Hdr10
    } else {
        crate::providers::HdrFormat::None
    };
    crate::providers::Stream {
        id: s.url.clone(),
        name: s.title.clone(),
        url: s.url,
        mime: None,
        quality: crate::providers::StreamQuality::from_label(&quality_label),
        provider: s.provider,
        protocol,
        seeders: s.seeders,
        bitrate_kbps: None,
        codec: s.codec,
        resolution: None,
        hdr,
        size_bytes: s.size_bytes,
        latency_ms: None,
        speed_mbps: None,
        audio_channels: None,
        language: s.language,
    }
}

fn stream_to_wire(stream: crate::providers::Stream, score: u32) -> StreamInfoWire {
    StreamInfoWire {
        url: stream.url.clone(),
        name: stream.name.clone(),
        quality: stream.quality.label().to_string(),
        provider: stream.provider.clone(),
        score,
        codec: stream.codec.clone(),
        source: None,
        hdr: matches!(stream.hdr, crate::providers::HdrFormat::None),
        seeders: stream.seeders,
        size_bytes: stream.size_bytes,
        speed_mbps: stream.speed_mbps,
        latency_ms: stream.latency_ms,
    }
}

/// Round-robin pick from quality buckets, best-first within each, until
/// `max_total` is reached. Input is assumed already ranked best-first
/// inside each quality (which is true after `quality::rank`).
///
/// The result keeps a balanced spread across resolutions instead of
/// the "all-4K, no usable 1080p" shape that plain `take(N)` produces
/// on aggregator responses.
fn diversify_by_quality(
    candidates: Vec<crate::quality::StreamCandidate>,
    max_total: usize,
) -> Vec<crate::quality::StreamCandidate> {
    use std::cmp::Reverse;
    use std::collections::BTreeMap;

    // BTreeMap<Reverse<StreamQuality>> iterates buckets best-quality
    // first (Uhd4k → Hd1080 → Hd720 → Unknown → Sd) so the round-robin
    // hands out 4K, then 1080p, then 720p, … on the first pass.
    let mut by_quality: BTreeMap<Reverse<crate::providers::StreamQuality>, Vec<crate::quality::StreamCandidate>>
        = BTreeMap::new();
    for c in candidates {
        by_quality.entry(Reverse(c.stream.quality.clone())).or_default().push(c);
    }

    let mut iters: Vec<_> = by_quality.into_values().map(Vec::into_iter).collect();
    let mut result = Vec::with_capacity(max_total);
    'outer: loop {
        let mut any = false;
        for it in iters.iter_mut() {
            if let Some(item) = it.next() {
                result.push(item);
                any = true;
                if result.len() >= max_total { break 'outer; }
            }
        }
        if !any { break; }
    }
    result
}

/// Streaming variant of the `find_streams` flow. Emits one
/// `StreamsPartial` per provider as soon as it returns, plus a
/// `StreamsComplete` marker after every provider has either responded
/// or hit the overall deadline.
///
/// The user-facing payoff: the streams column populates the moment
/// the first fast provider (Torrentio at ~300 ms) responds, and slow
/// aggregators (Jackett's 25 s Torznab fan-out) keep contributing
/// without blocking the early UI update. Earlier code waited on
/// `join_all` then sent one synchronous `StreamsResult`, so the user
/// stared at a spinner for the slowest provider's wall-time.
async fn run_find_streams_streaming(
    engine: &Arc<Engine>,
    config: &Arc<ConfigManager>,
    health: &Arc<HealthRegistry>,
    event_tx: &tokio::sync::mpsc::Sender<String>,
    trace: &Arc<TraceEmitter>,
    r: &GetStreamsRequest,
) {
    let cfg = config.snapshot().await;
    let health_map = health.all_reliability_scores();

    let reg = engine.registry().read().await;
    let provider_names: Vec<String> = reg.find_stream_providers()
        .into_iter()
        .map(|p| p.manifest.plugin.name.clone())
        .collect();
    drop(reg);

    let kind = match r.kind.as_deref() {
        Some("Movie")   => stui_plugin_sdk::EntryKind::Movie,
        Some("Series")  => stui_plugin_sdk::EntryKind::Series,
        Some("Episode") => stui_plugin_sdk::EntryKind::Episode,
        _               => stui_plugin_sdk::EntryKind::Movie,
    };

    // ── imdb_id late resolution ────────────────────────────────────────
    // Without an IMDb id, torrentio (the fastest stream provider, ~300ms)
    // skips the entry entirely and we fall back to slow scrapers / Torznab
    // fan-outs. The id is missing here when the user pressed Enter on the
    // Streams tab before catalog enrichment / detail-open enrichment had
    // landed for this entry. We have at least a TMDB id or title+year, so
    // ask the tmdb plugin to resolve. Cap at 4s — tmdb's enrich is sub-
    // second hot, ~1s cold; well below the overall 45s find_streams budget.
    let mut external_ids = r.external_ids.clone();
    let mut imdb_id = r.imdb_id.clone().filter(|s| !s.is_empty());
    if imdb_id.is_none() {
        if let Some(id) = external_ids.get("imdb").filter(|s| !s.is_empty()).cloned() {
            imdb_id = Some(id);
        } else if let Some(resolved) = resolve_imdb_id(engine, r, kind).await {
            tracing::info!(
                title = %r.title,
                imdb_id = %resolved,
                "find_streams: resolved missing imdb_id via tmdb enrich"
            );
            external_ids.insert("imdb".to_string(), resolved.clone());
            imdb_id = Some(resolved);
        }
    }

    let req = crate::abi::types::FindStreamsRequest {
        title: r.title.clone(),
        year: r.year,
        kind,
        season: r.season,
        episode: r.episode,
        external_ids,
        imdb_id,
        tmdb_id: r.tmdb_id.clone(),
    };

    // Hard upper bound on the entire fan-out. Each provider runs to
    // completion (or its own internal timeout); only the wall-clock
    // ceiling will cut off slow stragglers. Sized to comfortably
    // cover the slowest expected provider (Jackett Torznab across
    // many indexers, ~25-30 s) plus a small headroom under the TUI's
    // 60 s IPC timeout.
    const OVERALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(45);

    let start = std::time::Instant::now();
    let overall_deadline = start + OVERALL_BUDGET;
    let entry_id = r.entry_id.clone();
    let season   = r.season.unwrap_or(0);
    let episode  = r.episode.unwrap_or(0);
    let max_candidates = cfg.streaming.max_candidates.max(1);

    let mut futures = FuturesUnordered::new();
    for plugin_name in provider_names.iter() {
        let plugin_name = plugin_name.clone();
        let req = req.clone();
        let engine = engine.clone();
        futures.push(async move {
            let result = engine.supervisor_find_streams(&plugin_name, req, CallPriority::Foreground).await;
            (plugin_name, result)
        });
    }

    let mut had_any_results = false;
    let mut errors_text: Vec<String> = Vec::new();
    let mut pending: HashSet<String> = provider_names.iter().cloned().collect();

    while !futures.is_empty() {
        let now = std::time::Instant::now();
        let timeout_remaining = overall_deadline.saturating_duration_since(now);
        if timeout_remaining.is_zero() { break; }

        match tokio::time::timeout(timeout_remaining, futures.next()).await {
            Ok(Some((plugin_name, Ok(plugin_streams)))) => {
                pending.remove(&plugin_name);
                if plugin_streams.is_empty() { continue; }

                // Convert + rank within just this provider's batch.
                // Cross-provider re-ranking would require a final
                // pass — but the streaming UX shows results as they
                // arrive, and the per-provider rank is enough for
                // each batch to be meaningful on its own.
                let provider_streams: Vec<crate::providers::Stream> =
                    plugin_streams.into_iter().map(plugin_stream_to_provider).collect();

                let policy = crate::quality::RankingPolicy::default();
                let candidates = if !health_map.is_empty() {
                    crate::quality::rank_with_health(provider_streams, &policy, Some(&health_map))
                } else {
                    crate::quality::rank(provider_streams, &policy)
                };

                // Drop low-seeder dead torrents before they reach the
                // picker. Streams with unknown seeder counts (direct
                // HTTP, debrid CDN, magnet without DHT data) pass
                // through — the floor only applies when we have a
                // concrete seeder number to compare against.
                //
                // `require_seeders = true` flips the unknown-passes-
                // through default off: useful as a debug toggle when a
                // plugin's results don't surface seeders and you want
                // to see them disappear from the picker.
                let min_seeders        = cfg.streaming.min_seeders;
                let require_seeders    = cfg.streaming.require_seeders;
                let require_resolution = cfg.streaming.require_resolution;
                let allow_4k           = cfg.streaming.allow_4k;
                let allow_1080p        = cfg.streaming.allow_1080p;
                let allow_720p         = cfg.streaming.allow_720p;
                let allow_sd           = cfg.streaming.allow_sd;
                tracing::info!(
                    plugin = %plugin_name,
                    min_seeders, require_seeders, require_resolution,
                    allow_4k, allow_1080p, allow_720p, allow_sd,
                    candidate_count = candidates.len(),
                    "find_streams: filter inputs"
                );
                let wire: Vec<StreamInfoWire> = candidates
                    .iter()
                    .filter(|c| {
                        // Seeder gate
                        let seeders_ok = match c.stream.seeders {
                            Some(n) => min_seeders == 0 || n > min_seeders,
                            None    => !require_seeders,
                        };
                        if !seeders_ok { return false; }
                        // Per-tier resolution allowlist. Unknown is
                        // governed by `require_resolution` below.
                        use crate::providers::StreamQuality;
                        let tier_ok = match c.stream.quality {
                            StreamQuality::Uhd4k   => allow_4k,
                            StreamQuality::Hd1080  => allow_1080p,
                            StreamQuality::Hd720   => allow_720p,
                            StreamQuality::Sd      => allow_sd,
                            StreamQuality::Unknown => true,
                        };
                        if !tier_ok { return false; }
                        // Resolution gate — drop StreamQuality::Unknown
                        // when require_resolution is enabled. The
                        // ranker maps a missing/unparsed quality tag
                        // to Unknown upstream, so this filter catches
                        // both "no quality field" and "quality field
                        // present but unparseable".
                        if require_resolution
                            && matches!(c.stream.quality, crate::providers::StreamQuality::Unknown)
                        {
                            return false;
                        }
                        true
                    })
                    .take(max_candidates)
                    .map(|c| stream_to_wire(c.stream.clone(), c.score.total()))
                    .collect();

                if wire.is_empty() { continue; }

                had_any_results = true;
                let partial = StreamsPartialWire {
                    entry_id: entry_id.clone(),
                    season,
                    episode,
                    provider: plugin_name.clone(),
                    streams: wire,
                };
                if let Ok(line) = Response::StreamsPartial(partial).to_wire() {
                    if event_tx.send(line).await.is_err() {
                        // TUI hung up — no point continuing the fan-out.
                        return;
                    }
                }
            }
            Ok(Some((plugin_name, Err(e)))) => {
                pending.remove(&plugin_name);
                let sanitized = crate::ipc::sanitize_secrets(&e.to_string());
                tracing::warn!(plugin = %plugin_name, err = %sanitized, "find_streams: plugin returned error");
                errors_text.push(format!("{}: {}", plugin_name, sanitized));
            }
            Ok(None) => break, // all sources resolved
            Err(_) => {
                // Hard deadline reached. Whatever's still pending is
                // either in-flight on the supervisor lock or had its
                // outer future dropped — log and move on.
                for plugin_name in pending.iter() {
                    tracing::warn!(
                        plugin = %plugin_name,
                        elapsed_ms = (std::time::Instant::now() - start).as_millis() as u64,
                        "find_streams: hit overall deadline (still in flight)"
                    );
                    errors_text.push(format!("{}: timed out", plugin_name));
                }
                break;
            }
        }
    }

    // Final marker. Carry an error string ONLY when the user got
    // nothing at all — partial successes don't need a banner.
    let all_timed_out = !errors_text.is_empty()
        && errors_text.iter().all(|e| e.ends_with(": timed out"));
    let error = if had_any_results {
        None
    } else if all_timed_out {
        // Single tidy line instead of "jackett-provider: timed out;
        // prowlarr-provider: timed out; torrentio-provider: timed out;
        // …" — when everything timed out, the cause is almost always
        // the network or the deadline budget, not any one provider.
        Some("All stream providers timed out — check network or try again".to_string())
    } else if !errors_text.is_empty() {
        Some(errors_text.join("; "))
    } else {
        Some("No providers returned any streams".to_string())
    };
    if !had_any_results {
        trace.fallback("no streams after streaming fan-out");
    }
    let complete = StreamsCompleteWire {
        entry_id,
        season,
        episode,
        error,
    };
    if let Ok(line) = Response::StreamsComplete(complete).to_wire() {
        let _ = event_tx.send(line).await;
    }
}

/// Handle a `get_streams` IPC request.
///
/// Resolves streams via WASM plugin providers loaded through the Engine,
/// optionally benchmarking HTTP streams for speed if `benchmark_streams` is enabled.
/// Best-effort resolution of an IMDb id from whatever else the caller
/// provided (TMDB id, or just title+year). Used by `find_streams` when
/// the user pressed Enter before enrichment had populated `imdb_id`,
/// which would otherwise cause torrentio (the fastest movie source) to
/// skip the entry entirely. Returns `None` on timeout, plugin error, or
/// when no usable identifier is present.
async fn resolve_imdb_id(
    engine: &Arc<Engine>,
    r: &GetStreamsRequest,
    kind: stui_plugin_sdk::EntryKind,
) -> Option<String> {
    use crate::abi::types::{EnrichRequest, PluginEntry};

    // tmdb's enrich only handles movies / series / episodes — bail
    // gracefully for any unrelated kind that lands here.
    let kind_for_search = match kind {
        stui_plugin_sdk::EntryKind::Movie => stui_plugin_sdk::EntryKind::Movie,
        stui_plugin_sdk::EntryKind::Series | stui_plugin_sdk::EntryKind::Episode => {
            stui_plugin_sdk::EntryKind::Series
        }
        _ => return None,
    };

    let tmdb_id = r
        .tmdb_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| r.external_ids.get("tmdb").cloned().filter(|s| !s.is_empty()));

    // Without at least a TMDB id or a non-empty title there's nothing for
    // the plugin to anchor a lookup on.
    if tmdb_id.is_none() && r.title.is_empty() {
        return None;
    }

    let mut partial = PluginEntry {
        title: r.title.clone(),
        year: r.year,
        kind: kind_for_search,
        ..Default::default()
    };
    if let Some(t) = &tmdb_id {
        partial.id = t.clone();
        partial.source = "tmdb".to_string();
        partial.external_ids.insert("tmdb".to_string(), t.clone());
    }

    let enrich_req = EnrichRequest {
        partial,
        prefer_id_source: Some("tmdb".to_string()),
        force_refresh: false,
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        engine.supervisor_enrich("tmdb", enrich_req, CallPriority::Foreground),
    )
    .await;

    match result {
        Ok(Ok(entry)) => entry
            .imdb_id
            .filter(|s| !s.is_empty())
            .or_else(|| entry.external_ids.get("imdb").cloned().filter(|s| !s.is_empty())),
        Ok(Err(e)) => {
            tracing::debug!(err = %e, "find_streams: tmdb enrich for imdb_id failed");
            None
        }
        Err(_) => {
            tracing::debug!("find_streams: tmdb enrich for imdb_id timed out");
            None
        }
    }
}

pub async fn run_get_streams(
    engine: &Arc<Engine>,
    _catalog: &Arc<Catalog>,
    config: &Arc<ConfigManager>,
    health: &Arc<HealthRegistry>,
    bench: &StreamBenchmarker,
    trace: &Arc<TraceEmitter>,
    event_tx: tokio::sync::mpsc::Sender<String>,
    r: GetStreamsRequest,
) -> Response {
    let cfg = config.snapshot().await;
    let benchmark_enabled = cfg.streaming.benchmark_streams;
    let health_map = health.all_reliability_scores();

    let reg = engine.registry().read().await;
    let provider_names: Vec<String> = reg.find_stream_providers()
        .into_iter()
        .map(|p| p.manifest.plugin.name.clone())
        .collect();
    drop(reg);

    let mut all_streams: Vec<crate::providers::Stream> = vec![];
    let mut errors = vec![];

    // Decide between the new `find_streams` flow and the legacy
    // `resolve` flow based on whether the caller populated the new
    // request fields. New callers (Episodes tab streams column) supply
    // `title` (always) plus season/episode/external_ids; legacy callers
    // (the standalone stream picker) supply only `entry_id`.
    let use_find_streams = !r.title.is_empty();
    tracing::info!(
        title = %r.title,
        kind = ?r.kind,
        season = ?r.season,
        episode = ?r.episode,
        path = if use_find_streams { "find_streams" } else { "resolve_raw" },
        providers = ?provider_names,
        "get_streams: dispatching"
    );
    if use_find_streams {
        run_find_streams_streaming(
            engine, config, health, &event_tx, trace, &r,
        ).await;
        // Streaming path emits its own StreamsPartial / StreamsComplete
        // events via `event_tx`; the synchronous response is just an
        // ack so the TUI's request-id correlation channel unblocks.
        return Response::Ok;
    }

    // Legacy resolve_raw path — kept for the standalone stream picker
    // which still expects a single synchronous StreamsResult response.
    // Runs sequentially across providers because there's only one
    // legacy verb (`resolve_raw`) and the call shape is per-id.
    for plugin_name in &provider_names {
        match engine.resolve_raw(&r.entry_id, plugin_name).await {
            Ok(result) => {
                let quality_label = result.quality.clone().unwrap_or_else(|| "Unknown".to_string());
                let stream = crate::providers::Stream {
                    id: result.stream_url.clone(),
                    name: quality_label.clone(),
                    url: result.stream_url,
                    mime: None,
                    quality: crate::providers::StreamQuality::from_label(&quality_label),
                    provider: plugin_name.clone(),
                    protocol: Some("https".to_string()),
                    seeders: None,
                    bitrate_kbps: None,
                    codec: None,
                    resolution: None,
                    hdr: crate::providers::HdrFormat::None,
                    size_bytes: None,
                    latency_ms: None,
                    speed_mbps: None,
                    audio_channels: None,
                    language: None,
                };
                all_streams.push(stream);
            }
            Err(e) => {
                errors.push(format!("{}: {}", plugin_name, e));
            }
        }
    }

    // Emit per-provider errors; detect timeout errors separately
    for err in &errors {
        if let Some((name, msg)) = err.split_once(": ") {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("timeout") || msg_lower.contains("timed out") {
                trace.fallback("timeout");
            } else {
                trace.provider_error(name, msg);
            }
        }
    }

    if all_streams.is_empty() {
        trace.fallback("no streams after resolve");
        return Response::StreamsResult(StreamsResponse {
            id: r.id,
            entry_id: r.entry_id,
            streams: vec![],
        });
    }

    // Apply benchmarking if enabled
    if benchmark_enabled {
        all_streams = bench.probe_all(&all_streams).await;
        trace.bench(all_streams.len());
    }

    // Apply health-based re-ranking if health data available
    let candidates = if !health_map.is_empty() {
        use crate::quality::rank_with_health;
        rank_with_health(all_streams.clone(), &crate::quality::RankingPolicy::default(), Some(&health_map))
    } else {
        use crate::quality::rank;
        rank(all_streams.clone(), &crate::quality::RankingPolicy::default())
    };

    // Apply speed-based re-ranking if benchmarking enabled
    let candidates = if benchmark_enabled {
        use crate::quality::rank_with_health_and_speed;
        let mut speed_map: HashMap<String, f64> = HashMap::new();
        for stream in &all_streams {
            if let Some(speed) = stream.speed_mbps {
                speed_map.insert(stream.url.clone(), speed);
            }
        }
        if !speed_map.is_empty() {
            rank_with_health_and_speed(
                all_streams,
                &crate::quality::RankingPolicy::default(),
                if health_map.is_empty() { None } else { Some(&health_map) },
                Some(&speed_map),
            )
        } else {
            candidates
        }
    } else {
        candidates
    };

    // Cap at the user's configured limit AND diversify by resolution
    // before wire conversion. Two reasons:
    //   1. Indexer aggregators (Jackett over many trackers, Prowlarr)
    //      can return thousands of candidates; serialising all of them
    //      across the IPC boundary blows past safe message sizes and
    //      has been observed to take the runtime down with no panic
    //      logged.
    //   2. The ranker awards 4K +400 points (vs 1080p +300, 720p +200,
    //      SD +100) — so on a big aggregator response the top-N by
    //      score is always 100% 4K, even though most users want a
    //      bandwidth-friendlier mix. Round-robin across quality
    //      buckets gives "best 4K, best 1080p, best 720p, …, second
    //      4K, second 1080p, …" up to max_candidates.
    let max_candidates = cfg.streaming.max_candidates.max(1);
    let candidates = diversify_by_quality(candidates, max_candidates);
    let streams: Vec<StreamInfoWire> = candidates
        .iter()
        .map(|c| stream_to_wire(c.stream.clone(), c.score.total()))
        .collect();

    if streams.is_empty() {
        trace.fallback("no streams after bench");
    } else {
        let best_score = candidates.first()
            .map(|c| c.score.total() as f64 / 100.0)
            .unwrap_or(0.0);
        trace.rank(1, best_score);
    }

    Response::StreamsResult(StreamsResponse {
        id: r.id,
        entry_id: r.entry_id,
        streams,
    })
}
