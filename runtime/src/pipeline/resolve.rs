//! Stream-resolution pipeline — rank candidates and map to wire types.

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::engine::Engine;
use crate::ipc::{self, GetStreamsRequest, Response, StreamInfoWire, StreamsResponse};
use crate::quality::RankingPolicy;

/// Handle a `get_streams` IPC request.
///
/// Resolves streams via all matching providers, scores them with the
/// default ranking policy, and returns them sorted by quality score.
pub async fn run_get_streams(engine: &Arc<Engine>, catalog: &Arc<Catalog>, r: GetStreamsRequest) -> Response {
    let policy     = RankingPolicy::default();
    let candidates = engine.ranked_streams(&r.entry_id, &policy, catalog.providers()).await;

    let streams: Vec<StreamInfoWire> = candidates.into_iter().map(|c| {
        let name_up = c.stream.name.to_uppercase();

        // Prefer explicit codec field; fall back to name-string parse.
        let codec = c.stream.codec.clone().or_else(|| {
            if name_up.contains("AV1") { Some("AV1".to_string()) }
            else if name_up.contains("HEVC") || name_up.contains("H265") || name_up.contains("X265") { Some("HEVC".to_string()) }
            else if name_up.contains("H264") || name_up.contains("X264") || name_up.contains("AVC")  { Some("H264".to_string()) }
            else { None }
        });

        let source = if name_up.contains("BLURAY") || name_up.contains("BLU-RAY") { Some("BluRay".to_string()) }
            else if name_up.contains("WEB-DL") || name_up.contains("WEBDL") { Some("WEB-DL".to_string()) }
            else if name_up.contains("WEBRIP") || name_up.contains("WEB-RIP") { Some("WEBRip".to_string()) }
            else if name_up.contains("HDTV") { Some("HDTV".to_string()) }
            else if name_up.contains("DVDRIP") || name_up.contains("DVD-RIP") { Some("DVDRip".to_string()) }
            else if name_up.contains("CAM") { Some("CAM".to_string()) }
            else { None };

        // Prefer explicit hdr field; fall back to name-string check.
        let hdr = c.stream.hdr != crate::providers::HdrFormat::None
            || name_up.contains("HDR")
            || name_up.contains("DOLBY");

        StreamInfoWire {
            url:      c.stream.url.clone(),
            name:     c.stream.name.clone(),
            quality:  c.stream.quality.label().to_string(),
            provider: c.stream.provider.clone(),
            score:    c.score.total(),
            badge:    c.badge(),
            codec,
            source,
            hdr,
            seeders:  c.stream.seeders,
        }
    }).collect();

    Response::StreamsResult(StreamsResponse {
        id:       r.id,
        entry_id: r.entry_id,
        streams,
    })
}
