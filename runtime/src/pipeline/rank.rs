//! Policy-based stream ranking with human-readable explanations.

use crate::ipc::{Response, StreamPreferencesWire};
use crate::quality::{rank_streams, RankingPolicy, StreamPreferences};

use super::super::ipc::v1::{RankStreamsRequest, RankStreamsResponse, RankedStreamWire};

impl From<StreamPreferencesWire> for StreamPreferences {
    fn from(wire: StreamPreferencesWire) -> Self {
        StreamPreferences {
            prefer_protocol: wire.prefer_protocol,
            max_resolution: wire.max_resolution,
            max_size_mb: wire.max_size_mb,
            min_seeders: wire.min_seeders,
            avoid_labels: wire.avoid_labels,
            prefer_hdr: wire.prefer_hdr,
            prefer_codecs: wire.prefer_codecs,
        }
    }
}

pub async fn run_rank_streams(req: RankStreamsRequest) -> Response {
    let mut policy = RankingPolicy::default();
    policy.preferences = req.preferences.into();

    let ranked = rank_streams(req.streams, &policy);

    let ranked_wire: Vec<RankedStreamWire> = ranked
        .into_iter()
        .map(|scored| RankedStreamWire {
            stream: scored.stream,
            score: scored.score,
            reasons: scored.reasons,
        })
        .collect();

    Response::RankStreams(RankStreamsResponse { ranked: ranked_wire })
}
