//! Metadata enrichment pipeline.
//!
//! The orchestrator ([`fetch_detail_metadata`]) fans out the four metadata
//! verbs (enrich, credits, artwork, related) in parallel, each with its
//! own source-list fan-out, cache, timeout, and merge step. Partials are
//! emitted on an `mpsc::Sender<DetailMetadataPartial>` as soon as each
//! verb finishes — the TUI can paint whichever panel arrives first
//! without blocking on the slowest verb.
//!
//! ## Error handling (all inherited from lower layers)
//!
//! * Single-plugin error → `filter_map(Result::ok)` silently drops it;
//!   the fan-out continues with whatever other sources respond.
//! * Per-verb timeout → the fan-out's `tokio::time::timeout` returns `Err`
//!   and we substitute an empty result vector, which merges into
//!   [`MetadataPayload::Empty`].
//! * Stale cache → [`MetadataCache::get`] already filters expired entries
//!   (returns `None`), which drives us to the live fan-out path.
//! * Circuit-breaker → inherited from `Supervisor` via the `call_*` trait
//!   methods. The orchestrator itself never sees a tripped breaker — the
//!   call returns `Err` and we drop it.
//!
//! ## Sub-modules
//!
//! * [`sources`] — ordered source-list resolution.
//! * [`merge`] — pure per-verb merge functions.

pub mod sources;
pub mod merge;
mod wire;

use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use tokio::sync::mpsc;
use tracing::debug;

use crate::abi::types::{
    ArtworkRequest, ArtworkResponse, CreditsRequest, CreditsResponse, EnrichRequest,
    EnrichResponse, PluginEntry, RelatedRequest,
};
use crate::cache::metadata::{MetadataCache, MetadataPayload};
use crate::cache::metadata_key::{IdSource, MetadataCacheKey, MetadataVerb};

pub use dispatch::{EngineMetadataDispatch, ManifestCapabilityProbe};
pub use sources::{SourceCapabilityProbe, SourceResolver};

mod dispatch;

// ── Request / Partial types ──────────────────────────────────────────────────

/// Single top-level request to enrich a detail view for `entry_id`.
#[derive(Debug, Clone)]
pub struct DetailMetadataRequest {
    pub entry_id: String,
    pub id_source: IdSource,
    /// Lowercase TUI-tab label: `"movies" | "series" | "anime" | "music"`.
    pub kind: String,
    pub per_verb_timeout: Duration,
}

/// One merged per-verb payload streamed back to the TUI as soon as its
/// fan-out + merge finishes.
#[derive(Debug, Clone)]
pub struct DetailMetadataPartial {
    pub entry_id: String,
    pub verb: MetadataVerb,
    pub payload: MetadataPayload,
}

// ── Dispatch abstraction ─────────────────────────────────────────────────────

/// Abstraction over "call verb X on plugin Y". Implemented by the real
/// `Engine` in Chunk 5; mocked by `TestEngine` below for this chunk's
/// orchestrator tests.
///
/// Keeping this as a trait isolates the orchestrator from the full
/// `Engine` type (which carries plugin registry, sandbox, etc.) so tests
/// don't need to spin one up.
#[async_trait]
pub trait MetadataDispatch: Send + Sync {
    fn cache(&self) -> &MetadataCache;
    fn sources(&self) -> &SourceResolver;

    async fn call_enrich(
        &self,
        plugin: &str,
        req: EnrichRequest,
    ) -> Result<EnrichResponse, String>;
    async fn call_credits(
        &self,
        plugin: &str,
        req: CreditsRequest,
    ) -> Result<CreditsResponse, String>;
    async fn call_artwork(
        &self,
        plugin: &str,
        req: ArtworkRequest,
    ) -> Result<ArtworkResponse, String>;
    async fn call_related(
        &self,
        plugin: &str,
        req: RelatedRequest,
    ) -> Result<Vec<PluginEntry>, String>;
}

// ── Orchestrator ─────────────────────────────────────────────────────────────

/// Fan out all four verbs for a detail view in parallel; stream each
/// merged per-verb payload back on `tx` as soon as it's ready.
///
/// Takes the engine by value (require `Clone`) so each verb task can own
/// its own handle — sidesteps the `'static` lifetime constraint of
/// `tokio::spawn`. The real caller will pass an `Arc`-wrapped handle.
pub async fn fetch_detail_metadata<E>(
    engine: E,
    req: DetailMetadataRequest,
    tx: mpsc::Sender<DetailMetadataPartial>,
) where
    E: MetadataDispatch + Clone + 'static,
{
    let verbs = [
        MetadataVerb::Enrich,
        MetadataVerb::Credits,
        MetadataVerb::Artwork,
        MetadataVerb::Related,
    ];
    let mut handles = Vec::with_capacity(verbs.len());
    for verb in verbs {
        let eng = engine.clone();
        let req_c = req.clone();
        let tx_c = tx.clone();
        handles.push(tokio::spawn(async move {
            let payload = run_verb(&eng, verb, &req_c).await;
            let partial = DetailMetadataPartial {
                entry_id: req_c.entry_id.clone(),
                verb,
                payload,
            };
            // Silent drop if receiver has hung up — the detail panel
            // closed and nobody's listening.
            let _ = tx_c.send(partial).await;
        }));
    }
    // Wait for every verb task to finish (or the spawn to panic).
    // The orchestrator returns AFTER all partials are sent — callers
    // who want fire-and-forget should wrap this in `tokio::spawn`.
    for h in handles {
        let _ = h.await;
    }
}

/// Single-verb pipeline: cache lookup → source resolution → fan-out →
/// timeout wrap → merge → cache-write.
async fn run_verb<E: MetadataDispatch>(
    engine: &E,
    verb: MetadataVerb,
    req: &DetailMetadataRequest,
) -> MetadataPayload {
    let key = MetadataCacheKey {
        verb,
        id_source: req.id_source.clone(),
        id: req.entry_id.clone(),
    };

    // Cache HIT short-circuits the fan-out.
    if let Some(cached) = engine.cache().get(&key).await {
        debug!(?verb, id = %req.entry_id, "metadata verb cache HIT");
        return cached;
    }

    // Empty source list → no plugins can serve this (verb, kind). Bail.
    let sources = engine.sources().resolve(verb, &req.kind);
    if sources.is_empty() {
        debug!(?verb, kind = %req.kind, "no sources for verb");
        return MetadataPayload::Empty;
    }

    // Fan out + timeout + merge. On timeout we substitute empty results
    // so the merge still runs (returns a sensible "nothing" payload).
    //
    // `had_results` distinguishes an *authoritative* empty (plugins
    // responded, nothing found — safe to cache) from a *transient*
    // empty (timeout or all plugins errored — must NOT cache, or a
    // single blip poisons the 30-day TTL entry).
    //
    // When `had_results == false` we emit `MetadataPayload::Empty` rather
    // than an empty per-verb variant so the TUI can cleanly distinguish
    // "verb ran but found nothing" from "verb didn't run at all" — the
    // UI draws a `(none)` placeholder for Empty and nothing for the
    // empty-variant path.
    let (payload, had_results) = match verb {
        MetadataVerb::Enrich => {
            let fan = fan_out_enrich(engine, &sources, req);
            let results =
                tokio::time::timeout(req.per_verb_timeout, fan)
                    .await
                    .unwrap_or_default();
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::enrich_to_payload(merge::merge_enrich(None, results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
        MetadataVerb::Credits => {
            let fan = fan_out_credits(engine, &sources, req);
            let results =
                tokio::time::timeout(req.per_verb_timeout, fan)
                    .await
                    .unwrap_or_default();
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::credits_to_payload(merge::merge_credits(None, results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
        MetadataVerb::Artwork => {
            let fan = fan_out_artwork(engine, &sources, req);
            let results =
                tokio::time::timeout(req.per_verb_timeout, fan)
                    .await
                    .unwrap_or_default();
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::artwork_to_payload(merge::merge_artwork(results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
        MetadataVerb::Related => {
            let fan = fan_out_related(engine, &sources, req);
            let results =
                tokio::time::timeout(req.per_verb_timeout, fan)
                    .await
                    .unwrap_or_default();
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::related_to_payload(merge::merge_related(results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
    };

    if had_results {
        engine.cache().insert(key, payload.clone()).await;
    } else {
        debug!(
            ?verb,
            id = %req.entry_id,
            "skipping cache write: fan-out produced no results (timeout or all errored)"
        );
    }
    payload
}

// ── Per-verb fan-out helpers ─────────────────────────────────────────────────

async fn fan_out_enrich<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<EnrichResponse> {
    let calls = sources.iter().map(|plugin| {
        let er = EnrichRequest {
            partial: PluginEntry {
                id: req.entry_id.clone(),
                kind: Default::default(),
                title: String::new(),
                source: plugin.clone(),
                ..Default::default()
            },
            prefer_id_source: Some(id_source_as_str(&req.id_source)),
        };
        engine.call_enrich(plugin, er)
    });
    join_all(calls).await.into_iter().filter_map(Result::ok).collect()
}

async fn fan_out_credits<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<CreditsResponse> {
    let calls = sources.iter().map(|plugin| {
        let cr = CreditsRequest {
            id: req.entry_id.clone(),
            id_source: id_source_as_str(&req.id_source),
            kind: Default::default(),
        };
        engine.call_credits(plugin, cr)
    });
    join_all(calls).await.into_iter().filter_map(Result::ok).collect()
}

async fn fan_out_artwork<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<ArtworkResponse> {
    let calls = sources.iter().map(|plugin| {
        let ar = ArtworkRequest {
            id: req.entry_id.clone(),
            id_source: id_source_as_str(&req.id_source),
            kind: Default::default(),
            size: crate::abi::types::ArtworkSize::Any,
        };
        engine.call_artwork(plugin, ar)
    });
    join_all(calls).await.into_iter().filter_map(Result::ok).collect()
}

async fn fan_out_related<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<Vec<PluginEntry>> {
    let calls = sources.iter().map(|plugin| {
        let rr = RelatedRequest {
            id: req.entry_id.clone(),
            id_source: id_source_as_str(&req.id_source),
            kind: Default::default(),
            relation: crate::abi::types::RelationKind::Any,
            limit: 20,
        };
        engine.call_related(plugin, rr)
    });
    join_all(calls).await.into_iter().filter_map(Result::ok).collect()
}

fn id_source_as_str(s: &IdSource) -> String {
    match s {
        IdSource::Imdb => "imdb".into(),
        IdSource::Tmdb => "tmdb".into(),
        IdSource::Tvdb => "tvdb".into(),
        IdSource::Anilist => "anilist".into(),
        IdSource::Kitsu => "kitsu".into(),
        IdSource::Musicbrainz => "musicbrainz".into(),
        IdSource::Discogs => "discogs".into(),
        IdSource::Other(s) => s.clone(),
    }
}

/// Parse a wire-form id-source string back into [`IdSource`].
/// Unknown sources round-trip through [`IdSource::Other`].
fn parse_id_source(s: &str) -> IdSource {
    match s {
        "imdb" => IdSource::Imdb,
        "tmdb" => IdSource::Tmdb,
        "tvdb" => IdSource::Tvdb,
        "anilist" => IdSource::Anilist,
        "kitsu" => IdSource::Kitsu,
        "musicbrainz" => IdSource::Musicbrainz,
        "discogs" => IdSource::Discogs,
        other => IdSource::Other(other.to_string()),
    }
}

// ── Wire conversion ──────────────────────────────────────────────────────────

/// Default per-verb timeout applied to a wire [`GetDetailMetadataRequest`].
///
/// 8 s is long enough that individual plugin calls can stretch on a slow
/// network, but short enough that the user doesn't wait silently —
/// the orchestrator substitutes `MetadataPayload::Empty` and the TUI
/// still paints a useful card from whatever verbs finished in time.
pub const DEFAULT_PER_VERB_TIMEOUT: Duration = Duration::from_millis(8_000);

impl DetailMetadataRequest {
    /// Convert a wire request to the engine-internal form.
    pub fn from_wire(r: crate::ipc::v1::GetDetailMetadataRequest) -> Self {
        DetailMetadataRequest {
            entry_id: r.entry_id,
            id_source: parse_id_source(&r.id_source),
            kind: r.kind,
            per_verb_timeout: DEFAULT_PER_VERB_TIMEOUT,
        }
    }
}

impl DetailMetadataPartial {
    /// Convert an engine-internal partial to the IPC wire form.
    ///
    /// The payload field is already a `MetadataPayload` in both types
    /// (since `cache::metadata` re-exports `ipc::v1::MetadataPayload`),
    /// so only the `verb` enum needs to be serialised to its wire string.
    pub fn into_wire(self) -> crate::ipc::v1::DetailMetadataPartial {
        let verb = match self.verb {
            MetadataVerb::Enrich => "enrich",
            MetadataVerb::Credits => "credits",
            MetadataVerb::Artwork => "artwork",
            MetadataVerb::Related => "related",
        }
        .to_string();
        crate::ipc::v1::DetailMetadataPartial {
            entry_id: self.entry_id,
            verb,
            payload: self.payload,
        }
    }
}

// ── Test harness + tests ─────────────────────────────────────────────────────

#[cfg(test)]
pub mod test_engine {
    //! In-memory `MetadataDispatch` for exercising `fetch_detail_metadata`
    //! without the full `Engine`. Factory functions shape the per-verb
    //! behaviour (empty / latency / error / stuck) to drive each test.

    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    pub enum VerbBehavior {
        Empty,
        Delay(Duration),
        /// Sleep effectively forever — simulates a hung plugin so the
        /// orchestrator's timeout path trips.
        Stuck,
        Error,
    }

    pub struct FakeProbe {
        ids: Vec<String>,
    }
    impl SourceCapabilityProbe for FakeProbe {
        fn supports(&self, plugin: &str, _verb: MetadataVerb, _kind_hint: &str) -> bool {
            self.ids.iter().any(|p| p == plugin)
        }
    }

    #[derive(Clone)]
    pub struct TestEngine {
        cache: MetadataCache,
        sources: Arc<SourceResolver>,
        // verb -> behavior
        behavior: Arc<HashMap<MetadataVerb, VerbBehavior>>,
    }

    impl TestEngine {
        fn new(behavior: HashMap<MetadataVerb, VerbBehavior>) -> Self {
            // Use a fake source list wired to a single fake plugin so
            // the fan-out path is actually exercised (instead of the
            // empty-source-list short circuit).
            let mut cfg = crate::config::types::MetadataSources::default();
            cfg.movies = vec!["fake-tmdb".into()];
            cfg.series = vec!["fake-tmdb".into()];
            cfg.anime = vec!["fake-tmdb".into()];
            cfg.music = vec!["fake-tmdb".into()];
            let probe = FakeProbe { ids: vec!["fake-tmdb".into()] };
            let sources = SourceResolver::new(cfg, Box::new(probe));
            TestEngine {
                cache: MetadataCache::with_custom_ttl(Duration::from_secs(60)),
                sources: Arc::new(sources),
                behavior: Arc::new(behavior),
            }
        }

        fn behavior_for(&self, verb: MetadataVerb) -> VerbBehavior {
            self.behavior
                .get(&verb)
                .cloned()
                .unwrap_or(VerbBehavior::Empty)
        }

        async fn apply_behavior(&self, verb: MetadataVerb) -> Result<(), String> {
            match self.behavior_for(verb) {
                VerbBehavior::Empty => Ok(()),
                VerbBehavior::Delay(d) => {
                    tokio::time::sleep(d).await;
                    Ok(())
                }
                VerbBehavior::Stuck => {
                    // Long enough that any reasonable test timeout fires first.
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(())
                }
                VerbBehavior::Error => Err("simulated plugin error".into()),
            }
        }
    }

    #[async_trait]
    impl MetadataDispatch for TestEngine {
        fn cache(&self) -> &MetadataCache {
            &self.cache
        }
        fn sources(&self) -> &SourceResolver {
            &self.sources
        }

        async fn call_enrich(
            &self,
            _plugin: &str,
            _req: EnrichRequest,
        ) -> Result<EnrichResponse, String> {
            self.apply_behavior(MetadataVerb::Enrich).await?;
            Ok(EnrichResponse {
                entry: PluginEntry::default(),
                confidence: 0.0,
            })
        }
        async fn call_credits(
            &self,
            _plugin: &str,
            _req: CreditsRequest,
        ) -> Result<CreditsResponse, String> {
            self.apply_behavior(MetadataVerb::Credits).await?;
            Ok(CreditsResponse {
                cast: vec![],
                crew: vec![],
            })
        }
        async fn call_artwork(
            &self,
            _plugin: &str,
            _req: ArtworkRequest,
        ) -> Result<ArtworkResponse, String> {
            self.apply_behavior(MetadataVerb::Artwork).await?;
            Ok(ArtworkResponse { variants: vec![] })
        }
        async fn call_related(
            &self,
            _plugin: &str,
            _req: RelatedRequest,
        ) -> Result<Vec<PluginEntry>, String> {
            self.apply_behavior(MetadataVerb::Related).await?;
            Ok(vec![])
        }
    }

    // ── Factories ──────────────────────────────────────────────────────

    pub fn always_empty() -> TestEngine {
        TestEngine::new(HashMap::new())
    }

    pub fn with_latencies(spec: &[(MetadataVerb, Duration)]) -> TestEngine {
        let mut m = HashMap::new();
        for (v, d) in spec {
            m.insert(*v, VerbBehavior::Delay(*d));
        }
        TestEngine::new(m)
    }

    pub fn stuck(verb: MetadataVerb) -> TestEngine {
        let mut m = HashMap::new();
        m.insert(verb, VerbBehavior::Stuck);
        TestEngine::new(m)
    }

    pub fn make_request(id: &str) -> DetailMetadataRequest {
        DetailMetadataRequest {
            entry_id: id.into(),
            id_source: IdSource::Imdb,
            kind: "movies".into(),
            per_verb_timeout: Duration::from_secs(8),
        }
    }

    pub fn req_with_timeout_ms(ms: u64) -> DetailMetadataRequest {
        DetailMetadataRequest {
            entry_id: "tt1".into(),
            id_source: IdSource::Imdb,
            kind: "movies".into(),
            per_verb_timeout: Duration::from_millis(ms),
        }
    }

    pub async fn collect_all(
        rx: &mut mpsc::Receiver<DetailMetadataPartial>,
    ) -> Vec<DetailMetadataPartial> {
        let mut out = Vec::new();
        for _ in 0..4 {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(p)) => out.push(p),
                _ => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emits_four_partials_one_per_verb() {
        let engine = test_engine::always_empty();
        let (tx, mut rx) = mpsc::channel(16);
        fetch_detail_metadata(engine, test_engine::make_request("tt1"), tx).await;
        let mut verbs = std::collections::HashSet::new();
        while let Ok(Some(p)) =
            tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
        {
            verbs.insert(p.verb);
            if verbs.len() == 4 {
                break;
            }
        }
        assert_eq!(verbs.len(), 4);
    }

    #[tokio::test]
    async fn slow_verb_does_not_block_fast_verb() {
        let engine = test_engine::with_latencies(&[
            (MetadataVerb::Credits, Duration::from_millis(100)),
            (MetadataVerb::Artwork, Duration::from_secs(4)),
        ]);
        let (tx, mut rx) = mpsc::channel(16);
        let start = std::time::Instant::now();
        tokio::spawn(async move {
            fetch_detail_metadata(engine, test_engine::make_request("tt1"), tx).await;
        });
        let first = rx.recv().await.unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
        // Enrich / Related (both Empty, near-instant) tend to arrive
        // first. The contract we assert: it is NOT the slow Artwork verb.
        assert_ne!(first.verb, MetadataVerb::Artwork);
    }

    #[tokio::test]
    async fn timeout_emits_empty_for_stuck_verb() {
        let engine = test_engine::stuck(MetadataVerb::Related);
        let (tx, mut rx) = mpsc::channel(16);
        fetch_detail_metadata(engine, test_engine::req_with_timeout_ms(500), tx).await;
        let partials = test_engine::collect_all(&mut rx).await;
        let related = partials
            .into_iter()
            .find(|p| p.verb == MetadataVerb::Related)
            .unwrap();
        assert!(matches!(related.payload, MetadataPayload::Empty));
    }

    #[tokio::test]
    async fn timeout_does_not_cache_empty() {
        // A transient timeout (e.g. network blip) must NOT poison the
        // cache — otherwise a single hiccup short-circuits all future
        // opens for the 30-day TTL.
        let engine = test_engine::stuck(MetadataVerb::Credits);
        let (tx, mut rx) = mpsc::channel(16);
        fetch_detail_metadata(
            engine.clone(),
            test_engine::req_with_timeout_ms(300),
            tx,
        )
        .await;
        let _ = test_engine::collect_all(&mut rx).await;
        let key = MetadataCacheKey {
            verb: MetadataVerb::Credits,
            id_source: IdSource::Imdb,
            id: "tt1".into(),
        };
        assert!(
            engine.cache().get(&key).await.is_none(),
            "timeout must not cache Empty"
        );
    }

    #[tokio::test]
    async fn authoritative_empty_is_cached() {
        // Plugin responded with an empty CreditsResponse — this IS
        // ground truth ("we know there's no credits for this title"),
        // so it should be cached to avoid re-fanning on every open.
        //
        // `always_empty()` wires one fake plugin (`fake-tmdb`) whose
        // `call_*` methods return empty-but-Ok responses, so the
        // fan-out yields a non-empty Vec of empty responses — exactly
        // the authoritative-empty case.
        let engine = test_engine::always_empty();
        let (tx, mut rx) = mpsc::channel(16);
        fetch_detail_metadata(
            engine.clone(),
            test_engine::make_request("tt1"),
            tx,
        )
        .await;
        let _ = test_engine::collect_all(&mut rx).await;
        let key = MetadataCacheKey {
            verb: MetadataVerb::Credits,
            id_source: IdSource::Imdb,
            id: "tt1".into(),
        };
        assert!(
            engine.cache().get(&key).await.is_some(),
            "authoritative empty must be cached"
        );
    }
}
