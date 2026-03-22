//! Stream-resolution pipeline — rank candidates and map to wire types.

use std::sync::Arc;
use std::collections::HashMap;

use crate::catalog::Catalog;
use crate::config::ConfigManager;
use crate::engine::Engine;
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
    r: GetStreamsRequest,
) -> Response {
    let cfg = config.snapshot().await;
    let benchmark_enabled = cfg.streaming.benchmark_streams;
    let health_map = health.all_reliability_scores();

    // Get all stream providers from the registry
    let reg = engine.registry().read().await;
    let providers = reg.find_stream_providers();
    
    // For now, stream resolution is provider-specific
    // We collect streams from all providers that support this entry
    let mut all_streams: Vec<crate::providers::Stream> = vec![];
    let mut errors = vec![];
    
    for provider in providers {
        // Try to resolve streams via WASM plugin
        match engine.resolve_raw(&r.entry_id, &provider.manifest.plugin.name).await {
            Ok(result) => {
                // Convert resolve result to stream
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

    // If no streams found from providers, return empty
    if all_streams.is_empty() {
        return Response::StreamsResult(StreamsResponse {
            id: r.id,
            entry_id: r.entry_id,
            streams: vec![],
        });
    }

    // Apply benchmarking if enabled
    if benchmark_enabled {
        all_streams = bench.probe_all(&all_streams).await;
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
        .into_iter()
        .map(|c| stream_to_wire(c.stream, c.score.total()))
        .collect();

    Response::StreamsResult(StreamsResponse {
        id: r.id,
        entry_id: r.entry_id,
        streams,
    })
}
