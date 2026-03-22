//! Stream-resolution pipeline — rank candidates and map to wire types.

use std::sync::Arc;
use std::collections::HashMap;

use crate::catalog::Catalog;
use crate::config::ConfigManager;
use crate::engine::{Engine, TraceEmitter};
use crate::ipc::{GetStreamsRequest, Response, StreamInfoWire, StreamsResponse};
use crate::providers::{HealthRegistry, StreamBenchmarker};

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
        speed_mbps: stream.speed_mbps,
        latency_ms: stream.latency_ms,
    }
}

/// Handle a `get_streams` IPC request.
///
/// Resolves streams via WASM plugin providers loaded through the Engine,
/// optionally benchmarking HTTP streams for speed if `benchmark_streams` is enabled.
pub async fn run_get_streams(
    engine: &Arc<Engine>,
    _catalog: &Arc<Catalog>,
    config: &Arc<ConfigManager>,
    health: &Arc<HealthRegistry>,
    bench: &StreamBenchmarker,
    trace: &Arc<TraceEmitter>,
    r: GetStreamsRequest,
) -> Response {
    let cfg = config.snapshot().await;
    let benchmark_enabled = cfg.streaming.benchmark_streams;
    let health_map = health.all_reliability_scores();

    let reg = engine.registry().read().await;
    let providers = reg.find_stream_providers();

    let mut all_streams: Vec<crate::providers::Stream> = vec![];
    let mut errors = vec![];

    for provider in providers {
        match engine.resolve_raw(&r.entry_id, &provider.manifest.plugin.name).await {
            Ok(result) => {
                let quality_label = result.quality.clone().unwrap_or_else(|| "Unknown".to_string());
                let stream = crate::providers::Stream {
                    id: result.stream_url.clone(),
                    name: quality_label.clone(),
                    url: result.stream_url,
                    mime: None,
                    quality: crate::providers::StreamQuality::from_label(&quality_label),
                    provider: provider.manifest.plugin.name.clone(),
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
                errors.push(format!("{}: {}", provider.manifest.plugin.name, e));
            }
        }
    }
    drop(reg);

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

    // Convert to wire format
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
