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
use stui_plugin_sdk::{
    EntryKind,
    TrailersRequest, TrailersResponse,
    ReleaseInfoRequest, ReleaseInfoResponse,
    KeywordsRequest, KeywordsResponse, Keyword,
    BoxOfficeRequest, BoxOfficeResponse,
    AlternativeTitlesRequest, AlternativeTitlesResponse,
    BulkEnrichRequest, BulkEnrichResponse,
};
use crate::cache::metadata::{MetadataCache, MetadataPayload};
use crate::cache::metadata_key::{IdSource, MetadataCacheKey, MetadataVerb};

pub use dispatch::{EngineMetadataDispatch, ManifestCapabilityProbe};
pub use sources::{SourceCapabilityProbe, SourceResolver};

mod dispatch;

// ── Request / Partial types ──────────────────────────────────────────────────

/// Single top-level request to enrich a detail view for `entry_id`.
///
/// `title`/`year`/`external_ids` ride along so the orchestrator's enrich
/// stage can title-search a foreign provider (e.g. resolve a `kitsu-…`
/// entry's AniList id) and downstream verbs can dispatch each plugin
/// using its native id from `external_ids` rather than blindly forwarding
/// the entry's primary `(id_source, entry_id)` to every source.
#[derive(Debug, Clone)]
pub struct DetailMetadataRequest {
    pub entry_id: String,
    pub id_source: IdSource,
    /// Lowercase TUI-tab label: `"movies" | "series" | "anime" | "music"`.
    pub kind: String,
    pub per_verb_timeout: Duration,
    pub title: String,
    pub year: Option<u16>,
    pub external_ids: std::collections::BTreeMap<String, String>,
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

    async fn call_get_trailers(
        &self,
        plugin: &str,
        req: TrailersRequest,
    ) -> Result<TrailersResponse, String>;
    async fn call_get_release_info(
        &self,
        plugin: &str,
        req: ReleaseInfoRequest,
    ) -> Result<ReleaseInfoResponse, String>;
    async fn call_get_keywords(
        &self,
        plugin: &str,
        req: KeywordsRequest,
    ) -> Result<KeywordsResponse, String>;
    async fn call_get_box_office(
        &self,
        plugin: &str,
        req: BoxOfficeRequest,
    ) -> Result<BoxOfficeResponse, String>;
    async fn call_get_alternative_titles(
        &self,
        plugin: &str,
        req: AlternativeTitlesRequest,
    ) -> Result<AlternativeTitlesResponse, String>;

    /// Single-source fetch of the elfhosted rating-aggregator addon.
    /// Returns the addon's pre-formatted description block + external URL.
    /// `None` means the addon has no entry for this id; `Err` means
    /// network / decode failure (caller logs and falls through to Empty).
    /// Implementations that don't have a client wired (e.g. tests) return
    /// `Ok(None)`.
    async fn fetch_ratings_aggregator(
        &self,
        imdb_id: &str,
        kind: &str,
    ) -> Result<Option<crate::ipc::v1::RatingsAggregatorData>, String>;

    async fn call_bulk_enrich(
        &self,
        plugin: &str,
        req: BulkEnrichRequest,
    ) -> Result<BulkEnrichResponse, String>;
}

// ── Orchestrator ─────────────────────────────────────────────────────────────

/// Fan out all four verbs for a detail view in parallel; stream each
/// merged per-verb payload back on `tx` as soon as it's ready.
///
/// Takes the engine by value (require `Clone`) so each verb task can own
/// its own handle — sidesteps the `'static` lifetime constraint of
/// `tokio::spawn`. The real caller will pass an `Arc`-wrapped handle.
//
// New in ABI v2: fan_out_trailers / fan_out_release_info /
// fan_out_keywords / fan_out_box_office / fan_out_alternative_titles
// helpers exist below but are NOT invoked here yet — TUI consumption
// is the next plan; that plan extends `MetadataVerb` + the IPC frames
// + this orchestrator together.
pub async fn fetch_detail_metadata<E>(
    engine: E,
    mut req: DetailMetadataRequest,
    tx: mpsc::Sender<DetailMetadataPartial>,
) where
    E: MetadataDispatch + Clone + 'static,
{
    // ── Phase 1 — Enrich ─────────────────────────────────────────────────
    // Run enrich first (sequentially) so any external_ids it discovers
    // are available to the credits/artwork/related fan-outs in phase 2.
    // This is what bridges entries that arrived from one provider (e.g.
    // kitsu, which doesn't implement credits) over to a richer source
    // like AniList.
    let enrich_payload = run_verb(&engine, MetadataVerb::Enrich, &req).await;
    if let MetadataPayload::Enrich(ref e) = enrich_payload {
        for (k, v) in &e.external_ids {
            // Don't clobber pre-existing ids passed in from the catalog —
            // those are authoritative; enrich's hits are best-effort.
            req.external_ids.entry(k.clone()).or_insert(v.clone());
        }
    }
    // Stream the enrich partial first so the TUI can populate Studio /
    // Networks / etc. while phase 2 is still running.
    let _ = tx
        .send(DetailMetadataPartial {
            entry_id: req.entry_id.clone(),
            verb: MetadataVerb::Enrich,
            payload: enrich_payload,
        })
        .await;

    // ── Phase 2 — Credits / Artwork / Related (parallel) ─────────────────
    let verbs = [
        MetadataVerb::Credits,
        MetadataVerb::Artwork,
        MetadataVerb::Related,
    ];
    let mut handles = Vec::with_capacity(verbs.len() + 1);
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

    // Rating-aggregator runs in phase 2 too — independent single-source
    // HTTP, no plugin fan-out. Skip when we don't have an IMDb id (the
    // addon only accepts `tt…` lookups) or when the kind isn't movie /
    // series (the manifest declares only those two types).
    if let Some(imdb) = imdb_id_from_request(&req) {
        let kind_wire = match req.kind.as_str() {
            "movies" => Some("movie"),
            "series" | "anime" => Some("series"),
            _ => None,
        };
        if let Some(kind) = kind_wire {
            let eng = engine.clone();
            let req_c = req.clone();
            let tx_c = tx.clone();
            let imdb = imdb.clone();
            let kind = kind.to_string();
            handles.push(tokio::spawn(async move {
                let payload = run_ratings_aggregator(&eng, &req_c, &imdb, &kind).await;
                let partial = DetailMetadataPartial {
                    entry_id: req_c.entry_id.clone(),
                    verb: MetadataVerb::RatingsAggregator,
                    payload,
                };
                let _ = tx_c.send(partial).await;
            }));
        }
    }

    for h in handles {
        let _ = h.await;
    }
}

/// Resolve the IMDb id to use for the rating-aggregator call. The
/// orchestrator may have arrived with an IMDb id either as the primary
/// `entry_id` (id_source = imdb) or as a cross-provider id picked up
/// from the catalog merge / enrich phase (`external_ids["imdb"]`).
fn imdb_id_from_request(req: &DetailMetadataRequest) -> Option<String> {
    if let IdSource::Imdb = req.id_source {
        if !req.entry_id.is_empty() {
            return Some(req.entry_id.clone());
        }
    }
    req.external_ids
        .get("imdb")
        .filter(|s| !s.is_empty())
        .cloned()
}

/// Cache-aware fetch of the rating-aggregator block. Mirrors `run_verb`
/// for the four plugin-driven verbs but bypasses the source resolver
/// since this is a single fixed source.
async fn run_ratings_aggregator<E: MetadataDispatch>(
    engine: &E,
    req: &DetailMetadataRequest,
    imdb_id: &str,
    kind: &str,
) -> MetadataPayload {
    let cache_key = MetadataCacheKey {
        verb: MetadataVerb::RatingsAggregator,
        id_source: IdSource::Imdb,
        id: imdb_id.to_string(),
    };
    if let Some(p) = engine.cache().get(&cache_key).await {
        return p;
    }
    let payload = match engine.fetch_ratings_aggregator(imdb_id, kind).await {
        Ok(Some(data)) => MetadataPayload::RatingsAggregator(data),
        Ok(None) => MetadataPayload::Empty,
        Err(e) => {
            tracing::debug!(
                err = %e,
                title = %req.title,
                imdb = %imdb_id,
                "rating_aggregator: fetch failed"
            );
            MetadataPayload::Empty
        }
    };
    if matches!(payload, MetadataPayload::Empty) {
        engine.cache().insert_negative(cache_key, payload.clone()).await;
    } else {
        engine.cache().insert(cache_key, payload.clone()).await;
    }
    payload
}

/// Resolve which `(id, id_source)` to send to a specific plugin for `verb`,
/// driven by the manifest-declared `id_sources` list rather than a hardcoded
/// plugin-name → id-source table.
///
/// Routing order:
/// 1. **Manifest path** — if the plugin declares `id_sources` for `verb`
///    (e.g. xmdb's `enrich = { id_sources = ["imdb"] }`), walk the list and
///    return the first id we can serve: either the request's primary
///    `entry_id` (when its `id_source` matches) or `req.external_ids[src]`.
///    Returns `None` if the plugin declares constraints but none can be met
///    — caller skips this plugin instead of feeding it a mismatched id.
/// 2. **Runtime-native special case** — `fanart` is kind-conditional
///    (movies → tmdb id, series → tvdb id) and isn't a wasm plugin, so it
///    keeps its hardcoded routing here until the manifest schema gains
///    kind-conditional id_sources.
/// 3. **Legacy fallback** — bool-form verbs (`enrich = true`, no
///    declared id_sources) preserve the old plugin-name-as-id-source
///    default: prefer `external_ids[plugin]`, else the entry's primary id.
fn resolve_id_for_plugin(
    req: &DetailMetadataRequest,
    plugin: &str,
    id_sources: &[String],
) -> Option<(String, String)> {
    // (2) fanart: runtime-native, kind-conditional. Skipped by the manifest
    // path because fanart isn't a wasm plugin in the registry.
    if plugin == "fanart" {
        let preferred = match req.kind.as_str() {
            "movies" => "tmdb",
            "series" | "anime" | "episode" => "tvdb",
            _ => "tmdb",
        };
        if let Some(id) = req.external_ids.get(preferred).filter(|s| !s.is_empty()) {
            return Some((id.clone(), preferred.to_string()));
        }
        if id_source_as_str(&req.id_source) == preferred && !req.entry_id.is_empty() {
            return Some((req.entry_id.clone(), preferred.to_string()));
        }
        return None;
    }

    // (1) manifest-driven routing.
    if !id_sources.is_empty() {
        let primary_src = id_source_as_str(&req.id_source);
        for src in id_sources {
            if src.as_str() == primary_src && !req.entry_id.is_empty() {
                return Some((req.entry_id.clone(), src.clone()));
            }
            if let Some(id) = req.external_ids.get(src).filter(|s| !s.is_empty()) {
                return Some((id.clone(), src.clone()));
            }
        }
        // Plugin declared constraints but none are available → skip.
        return None;
    }

    // (3) legacy fallback for bool-form `verb = true` declarations.
    if let Some(id) = req.external_ids.get(plugin).filter(|s| !s.is_empty()) {
        return Some((id.clone(), plugin.to_string()));
    }
    Some((req.entry_id.clone(), id_source_as_str(&req.id_source)))
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

    // Anime-shaped entries (id_source = anilist/kitsu) get filed under
    // the "series" tab by the catalog merge, so the wire `kind` arriving
    // from the TUI is "series" — but the series source list (tvdb/tmdb/
    // omdb) won't resolve any anime-aware plugin, leaving every verb
    // empty. Promote to the "anime" source list when the id_source
    // signals anime origin.
    let effective_kind: &str = match (req.kind.as_str(), &req.id_source) {
        ("series", IdSource::Anilist | IdSource::Kitsu) => "anime",
        _ => req.kind.as_str(),
    };

    // Empty source list → no plugins can serve this (verb, kind). Bail.
    let sources = engine.sources().resolve(verb, effective_kind);
    debug!(
        ?verb,
        wire_kind = %req.kind,
        effective_kind,
        id_source = %id_source_as_str(&req.id_source),
        entry_id = %req.entry_id,
        sources = ?sources,
        "metadata verb dispatch"
    );
    if sources.is_empty() {
        debug!(
            ?verb,
            id_source = %id_source_as_str(&req.id_source),
            kind = %effective_kind,
            entry_id = %req.entry_id,
            "no sources for verb — entry will get empty payload"
        );
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
            // fan_out_credits is now deadline-aware internally — it
            // returns whatever results landed before per_verb_timeout
            // elapsed instead of a wholesale empty Vec. No outer
            // timeout wrapper needed.
            let results = fan_out_credits(engine, &sources, req).await;
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::credits_to_payload(merge::merge_credits(None, results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
        MetadataVerb::Artwork => {
            let results = fan_out_artwork(engine, &sources, req).await;
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::artwork_to_payload(merge::merge_artwork(results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
        MetadataVerb::Related => {
            let results = fan_out_related(engine, &sources, req).await;
            let had_results = !results.is_empty();
            let payload = if had_results {
                wire::related_to_payload(merge::merge_related(results))
            } else {
                MetadataPayload::Empty
            };
            (payload, had_results)
        }
        // RatingsAggregator runs via the dedicated `run_ratings_aggregator`
        // path (single fixed source, no plugin fan-out), so this verb
        // never reaches the plugin-driven `run_verb` dispatch.
        MetadataVerb::RatingsAggregator => {
            debug_assert!(false, "run_verb called with RatingsAggregator");
            (MetadataPayload::Empty, false)
        }
    };

    if had_results {
        engine.cache().insert(key, payload.clone()).await;
    } else {
        // Cache the empty result with a short TTL so a flaky / throttled
        // upstream (TMDB quota error, TVDB transient 5xx) doesn't get
        // re-hammered on every detail re-open. The user sees no credits
        // for 60s, then the next open retries the fan-out.
        debug!(
            ?verb,
            id = %req.entry_id,
            "fan-out empty (timeout or all errored) — caching negative result"
        );
        engine.cache().insert_negative(key, payload.clone()).await;
    }
    debug!(
        ?verb,
        id_source = %id_source_as_str(&req.id_source),
        entry_id = %req.entry_id,
        had_results,
        "metadata verb result"
    );
    payload
}

// ── Per-verb fan-out helpers ─────────────────────────────────────────────────

/// Map the tab-flavoured `kind` string coming from the TUI
/// (`"movies" | "series" | "anime" | "music"`) to the `EntryKind` the
/// plugins expect on their verb requests.
///
/// Anime cards hit the same `/tv/{id}/credits` endpoint as series on
/// TMDB, so they map to `Series`. Music maps to `Track` as a fallback —
/// music providers (discogs/musicbrainz) route on scope internally and
/// won't use kind for detail-level verbs today, but we pass something
/// so the wire form isn't ambiguous.
fn entry_kind_from_hint(hint: &str) -> EntryKind {
    match hint {
        "movies" => EntryKind::Movie,
        "series" | "anime" => EntryKind::Series,
        "music" => EntryKind::Track,
        _ => EntryKind::Movie, // safest default — most verbs error cleanly on wrong-kind
    }
}

async fn fan_out_enrich<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<EnrichResponse> {
    let kind = entry_kind_from_hint(&req.kind);
    let calls = sources.iter().filter_map(|plugin| {
        // Each plugin's enrich gets the entry id it natively recognises
        // (per its manifest `id_sources`) plus title/year so plugins that
        // don't yet have a native id can title-search to discover one.
        // Skip the plugin entirely when its declared id_sources can't be
        // satisfied — feeding a strict plugin a mismatched id just wastes
        // a round-trip on a guaranteed UNKNOWN_ID rejection.
        let id_sources = engine.sources().id_sources_for(plugin, MetadataVerb::Enrich);
        let (id, _id_src) = resolve_id_for_plugin(req, plugin, &id_sources)?;
        let er = EnrichRequest {
            partial: PluginEntry {
                id,
                kind,
                title: req.title.clone(),
                year: req.year.map(u32::from),
                source: plugin.clone(),
                external_ids: req
                    .external_ids
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                ..Default::default()
            },
            prefer_id_source: Some(id_source_as_str(&req.id_source)),
            force_refresh: false,
        };
        Some(engine.call_enrich(plugin, er))
    });
    join_all(calls).await.into_iter().filter_map(Result::ok).collect()
}

async fn fan_out_credits<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<CreditsResponse> {
    let kind = entry_kind_from_hint(&req.kind);
    let calls = sources.iter().filter_map(|plugin| {
        let id_sources = engine.sources().id_sources_for(plugin, MetadataVerb::Credits);
        let (id, id_source) = resolve_id_for_plugin(req, plugin, &id_sources)?;
        let cr = CreditsRequest { id, id_source, kind, force_refresh: false };
        Some(engine.call_credits(plugin, cr))
    });
    drain_with_deadline(calls.collect(), req.per_verb_timeout).await
}

async fn fan_out_artwork<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<ArtworkResponse> {
    let kind = entry_kind_from_hint(&req.kind);
    let calls = sources.iter().filter_map(|plugin| {
        let id_sources = engine.sources().id_sources_for(plugin, MetadataVerb::Artwork);
        let (id, id_source) = resolve_id_for_plugin(req, plugin, &id_sources)?;
        let ar = ArtworkRequest {
            id,
            id_source,
            kind,
            size: crate::abi::types::ArtworkSize::Any,
            force_refresh: false,
        };
        Some(engine.call_artwork(plugin, ar))
    });
    drain_with_deadline(calls.collect(), req.per_verb_timeout).await
}

async fn fan_out_related<E: MetadataDispatch>(
    engine: &E,
    sources: &[String],
    req: &DetailMetadataRequest,
) -> Vec<Vec<PluginEntry>> {
    let kind = entry_kind_from_hint(&req.kind);
    let calls = sources.iter().filter_map(|plugin| {
        let id_sources = engine.sources().id_sources_for(plugin, MetadataVerb::Related);
        let (id, id_source) = resolve_id_for_plugin(req, plugin, &id_sources)?;
        let rr = RelatedRequest {
            id,
            id_source,
            kind,
            relation: crate::abi::types::RelationKind::Any,
            limit: 20,
            force_refresh: false,
        };
        Some(engine.call_related(plugin, rr))
    });
    drain_with_deadline(calls.collect(), req.per_verb_timeout).await
}

// ── ABI v2 fan-out helpers (not yet wired into the orchestrator) ─────────────

/// Returns the first `Ok` response from the plugin list, or the last `Err`
/// if all fail. First-non-error-wins semantics mirror `fan_out_credits`.
async fn fan_out_trailers<E: MetadataDispatch>(
    engine: &E,
    plugins: &[String],
    req: TrailersRequest,
) -> Result<TrailersResponse, String> {
    let mut last_err = String::from("no trailers providers configured");
    for plugin in plugins {
        match engine.call_get_trailers(plugin, req.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!(plugin = %plugin, error = %e, "get_trailers call failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// Returns the first `Ok` response from the plugin list, or the last `Err`
/// if all fail.
async fn fan_out_release_info<E: MetadataDispatch>(
    engine: &E,
    plugins: &[String],
    req: ReleaseInfoRequest,
) -> Result<ReleaseInfoResponse, String> {
    let mut last_err = String::from("no release_info providers configured");
    for plugin in plugins {
        match engine.call_get_release_info(plugin, req.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!(plugin = %plugin, error = %e, "get_release_info call failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// Returns the first `Ok` response from the plugin list, or the last `Err`
/// if all fail.
async fn fan_out_box_office<E: MetadataDispatch>(
    engine: &E,
    plugins: &[String],
    req: BoxOfficeRequest,
) -> Result<BoxOfficeResponse, String> {
    let mut last_err = String::from("no box_office providers configured");
    for plugin in plugins {
        match engine.call_get_box_office(plugin, req.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!(plugin = %plugin, error = %e, "get_box_office call failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// Returns the first `Ok` response from the plugin list, or the last `Err`
/// if all fail.
async fn fan_out_alternative_titles<E: MetadataDispatch>(
    engine: &E,
    plugins: &[String],
    req: AlternativeTitlesRequest,
) -> Result<AlternativeTitlesResponse, String> {
    let mut last_err = String::from("no alternative_titles providers configured");
    for plugin in plugins {
        match engine.call_get_alternative_titles(plugin, req.clone()).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!(plugin = %plugin, error = %e, "get_alternative_titles call failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// Maximum number of merged keywords across all providers.
const MAX_MERGED_KEYWORDS: usize = 200;

/// Round-robin fan-out for keywords across multiple providers.
///
/// Each provider is called concurrently upfront; then their keyword
/// iterators are interleaved round-robin so that a provider with many
/// keywords doesn't crowd out a late-joining provider with fewer.
/// Keywords are deduplicated case-insensitively; the first occurrence
/// wins. Each kept `Keyword.provider` is stamped with the originating
/// plugin name. Capped at [`MAX_MERGED_KEYWORDS`] total.
///
/// Partial success: if some providers error and some succeed, the
/// succeeded keywords are returned as `Ok`. Only returns `Err` when
/// all providers failed.
async fn fan_out_keywords<E: MetadataDispatch>(
    engine: &E,
    plugins: Vec<String>,
    req: KeywordsRequest,
) -> Result<KeywordsResponse, String> {
    use std::collections::HashSet;

    let total_plugins = plugins.len();
    let mut iters: Vec<(String, std::vec::IntoIter<Keyword>)> = Vec::new();
    let mut error_count = 0usize;

    for p in &plugins {
        let req_p = req.clone();
        match engine.call_get_keywords(p, req_p).await {
            Ok(resp) => iters.push((p.clone(), resp.keywords.into_iter())),
            Err(e) => {
                error_count += 1;
                tracing::warn!(plugin = %p, error = %e, "keywords call failed");
            }
        }
    }

    if iters.is_empty() && total_plugins > 0 && error_count > 0 {
        return Err(format!("all {error_count} keyword providers failed"));
    }

    let mut merged: Vec<Keyword> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    'outer: while !iters.is_empty() {
        let mut progressed = false;
        let mut i = 0;
        while i < iters.len() {
            let (provider_name, it) = &mut iters[i];
            match it.next() {
                Some(mut kw) => {
                    progressed = true;
                    let key = kw.name.trim().to_lowercase();
                    if seen.insert(key) {
                        kw.provider = Some(provider_name.clone());
                        merged.push(kw);
                        if merged.len() >= MAX_MERGED_KEYWORDS {
                            break 'outer;
                        }
                    }
                    i += 1;
                }
                None => {
                    iters.remove(i);
                }
            }
        }
        if !progressed {
            break;
        }
    }

    Ok(KeywordsResponse { keywords: merged })
}

/// Stream-collect results from a parallel fan-out, returning any
/// `Ok` outputs received before `budget` elapses. Errored sources are
/// dropped silently. Crucially, this is NOT `join_all` — that would
/// block until every future completed, so a single hung source (TMDB
/// rate-limited at 8 s) would shadow a fast one (TVDB at 1 s) when
/// the outer deadline fired and dropped everything wholesale. Using
/// `FuturesUnordered` lets fast responses land in `results` even if
/// later ones miss the deadline.
async fn drain_with_deadline<F, T, E>(
    mut futures: futures::stream::FuturesUnordered<F>,
    budget: std::time::Duration,
) -> Vec<T>
where
    F: futures::Future<Output = Result<T, E>>,
{
    use futures::stream::StreamExt;
    let mut results = Vec::new();
    let deadline = std::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() { break; }
        match tokio::time::timeout(remaining, futures.next()).await {
            Ok(Some(Ok(r))) => results.push(r),
            Ok(Some(Err(_))) => continue, // one source errored — skip, others may yet succeed
            Ok(None) => break,            // all sources resolved
            Err(_) => break,              // deadline reached; return what we have
        }
    }
    results
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
            title: r.title,
            year: r.year,
            external_ids: r.external_ids,
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
            MetadataVerb::RatingsAggregator => "ratings_aggregator",
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
        pub ids: Vec<String>,
    }
    impl SourceCapabilityProbe for FakeProbe {
        fn supports(&self, plugin: &str, _verb: MetadataVerb, _kind_hint: &str) -> bool {
            self.ids.iter().any(|p| p == plugin)
        }
        fn discover(&self, _verb: MetadataVerb, _kind_hint: &str) -> Vec<String> {
            // Test fake doesn't model auto-discovery — the resolver
            // tests cover that explicitly with their own probe.
            Vec::new()
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

        async fn call_get_trailers(
            &self,
            _plugin: &str,
            _req: TrailersRequest,
        ) -> Result<TrailersResponse, String> {
            Ok(TrailersResponse { trailers: vec![] })
        }
        async fn call_get_release_info(
            &self,
            _plugin: &str,
            _req: ReleaseInfoRequest,
        ) -> Result<ReleaseInfoResponse, String> {
            Ok(ReleaseInfoResponse { releases: vec![] })
        }
        async fn call_get_keywords(
            &self,
            _plugin: &str,
            _req: KeywordsRequest,
        ) -> Result<KeywordsResponse, String> {
            Ok(KeywordsResponse { keywords: vec![] })
        }
        async fn call_get_box_office(
            &self,
            _plugin: &str,
            _req: BoxOfficeRequest,
        ) -> Result<BoxOfficeResponse, String> {
            Ok(BoxOfficeResponse {
                budget: None,
                opening_weekend: None,
                gross_domestic: None,
                gross_worldwide: None,
            })
        }
        async fn call_get_alternative_titles(
            &self,
            _plugin: &str,
            _req: AlternativeTitlesRequest,
        ) -> Result<AlternativeTitlesResponse, String> {
            Ok(AlternativeTitlesResponse { titles: vec![] })
        }

        async fn fetch_ratings_aggregator(
            &self,
            _imdb_id: &str,
            _kind: &str,
        ) -> Result<Option<crate::ipc::v1::RatingsAggregatorData>, String> {
            self.apply_behavior(MetadataVerb::RatingsAggregator).await?;
            Ok(None)
        }

        async fn call_bulk_enrich(
            &self,
            _plugin: &str,
            _req: BulkEnrichRequest,
        ) -> Result<BulkEnrichResponse, String> {
            Ok(BulkEnrichResponse { entries: vec![] })
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
            title: String::new(),
            year: None,
            external_ids: Default::default(),
        }
    }

    pub fn req_with_timeout_ms(ms: u64) -> DetailMetadataRequest {
        DetailMetadataRequest {
            entry_id: "tt1".into(),
            id_source: IdSource::Imdb,
            kind: "movies".into(),
            per_verb_timeout: Duration::from_millis(ms),
            title: String::new(),
            year: None,
            external_ids: Default::default(),
        }
    }

    pub async fn collect_all(
        rx: &mut mpsc::Receiver<DetailMetadataPartial>,
    ) -> Vec<DetailMetadataPartial> {
        // Drain until the orchestrator drops `tx` (channel closes) or a
        // 5 s lull goes by — whichever happens first. The fixed `0..4`
        // loop that lived here before silently dropped the slowest
        // partial once the orchestrator started emitting 5 messages
        // (Enrich + Credits + Artwork + Related + RatingsAggregator
        // for IMDb-keyed movie/series requests). Iterate until close
        // so the count stays self-correcting.
        let mut out = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(p)) => out.push(p),
                Ok(None) => break, // channel closed — orchestrator finished
                Err(_) => break,   // 5 s without a partial — give up
            }
        }
        out
    }
}

// ── TestEngine wrappers for new verb fan-out tests ───────────────────────────

/// A minimal `MetadataDispatch` for the new verb fan-out tests.
///
/// Allows wiring per-plugin, per-verb canned responses or errors — more
/// flexible than `TestEngine` (which is verb-level only) for the round-robin
/// keywords tests that need per-plugin keyword lists.
#[cfg(test)]
pub(crate) mod test_dispatch {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    pub type TrailersMap    = HashMap<String, Result<TrailersResponse, String>>;
    pub type BoxOfficeMap   = HashMap<String, Result<BoxOfficeResponse, String>>;
    pub type KeywordsMap    = HashMap<String, Result<KeywordsResponse, String>>;
    pub type BulkEnrichMap  = HashMap<String, Result<BulkEnrichResponse, String>>;

    #[derive(Clone)]
    pub struct PluginDispatch {
        cache: MetadataCache,
        sources: Arc<SourceResolver>,
        pub trailers:     Arc<TrailersMap>,
        pub box_office:   Arc<BoxOfficeMap>,
        pub keywords:     Arc<KeywordsMap>,
        pub bulk_enrich:  Arc<BulkEnrichMap>,
    }

    impl PluginDispatch {
        pub fn new(
            trailers:    TrailersMap,
            box_office:  BoxOfficeMap,
            keywords:    KeywordsMap,
            bulk_enrich: BulkEnrichMap,
        ) -> Self {
            let mut cfg = crate::config::types::MetadataSources::default();
            cfg.movies = vec!["fake-p1".into(), "fake-p2".into()];
            cfg.series = vec!["fake-p1".into(), "fake-p2".into()];
            let probe = super::test_engine::FakeProbe { ids: vec!["fake-p1".into(), "fake-p2".into()] };
            let sources = SourceResolver::new(cfg, Box::new(probe));
            PluginDispatch {
                cache:       MetadataCache::with_custom_ttl(Duration::from_secs(60)),
                sources:     Arc::new(sources),
                trailers:    Arc::new(trailers),
                box_office:  Arc::new(box_office),
                keywords:    Arc::new(keywords),
                bulk_enrich: Arc::new(bulk_enrich),
            }
        }
    }

    #[async_trait]
    impl MetadataDispatch for PluginDispatch {
        fn cache(&self) -> &MetadataCache { &self.cache }
        fn sources(&self) -> &SourceResolver { &self.sources }

        async fn call_enrich(&self, _: &str, _: EnrichRequest) -> Result<EnrichResponse, String> {
            Ok(EnrichResponse { entry: PluginEntry::default(), confidence: 0.0 })
        }
        async fn call_credits(&self, _: &str, _: CreditsRequest) -> Result<CreditsResponse, String> {
            Ok(CreditsResponse { cast: vec![], crew: vec![] })
        }
        async fn call_artwork(&self, _: &str, _: ArtworkRequest) -> Result<ArtworkResponse, String> {
            Ok(ArtworkResponse { variants: vec![] })
        }
        async fn call_related(&self, _: &str, _: RelatedRequest) -> Result<Vec<PluginEntry>, String> {
            Ok(vec![])
        }
        async fn call_get_trailers(&self, plugin: &str, _: TrailersRequest) -> Result<TrailersResponse, String> {
            self.trailers.get(plugin).cloned().unwrap_or(Err("no fixture".into()))
        }
        async fn call_get_release_info(&self, _: &str, _: ReleaseInfoRequest) -> Result<ReleaseInfoResponse, String> {
            Ok(ReleaseInfoResponse { releases: vec![] })
        }
        async fn call_get_keywords(&self, plugin: &str, _: KeywordsRequest) -> Result<KeywordsResponse, String> {
            self.keywords.get(plugin).cloned().unwrap_or(Err("no fixture".into()))
        }
        async fn call_get_box_office(&self, plugin: &str, _: BoxOfficeRequest) -> Result<BoxOfficeResponse, String> {
            self.box_office.get(plugin).cloned().unwrap_or(Err("no fixture".into()))
        }
        async fn call_get_alternative_titles(&self, _: &str, _: AlternativeTitlesRequest) -> Result<AlternativeTitlesResponse, String> {
            Ok(AlternativeTitlesResponse { titles: vec![] })
        }
        async fn fetch_ratings_aggregator(&self, _: &str, _: &str) -> Result<Option<crate::ipc::v1::RatingsAggregatorData>, String> {
            Ok(None)
        }
        async fn call_bulk_enrich(&self, plugin: &str, _req: BulkEnrichRequest) -> Result<BulkEnrichResponse, String> {
            match self.bulk_enrich.get(plugin) {
                Some(Ok(resp)) => Ok(resp.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(format!("no canned bulk_enrich response for plugin {plugin}")),
            }
        }
    }

    fn kw(name: &str) -> Keyword {
        Keyword { name: name.to_string(), source_id: None, provider: None }
    }

    pub fn keywords_resp(names: &[&str]) -> Result<KeywordsResponse, String> {
        Ok(KeywordsResponse { keywords: names.iter().map(|n| kw(n)).collect() })
    }

    pub fn trailers_resp(url: &str) -> Result<TrailersResponse, String> {
        Ok(TrailersResponse {
            trailers: vec![stui_plugin_sdk::Trailer {
                url: url.to_string(),
                thumbnail_url: None,
                title: None,
                kind: stui_plugin_sdk::TrailerKind::Trailer,
                language: None,
                duration_secs: None,
            }],
        })
    }

    pub fn box_office_resp(gross: u64) -> Result<BoxOfficeResponse, String> {
        Ok(BoxOfficeResponse {
            budget: None,
            opening_weekend: None,
            gross_domestic: None,
            gross_worldwide: Some(stui_plugin_sdk::MoneyAmount { amount: gross, currency: "USD".into() }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: &str, id_source: IdSource, entry_id: &str) -> DetailMetadataRequest {
        DetailMetadataRequest {
            entry_id: entry_id.to_string(),
            id_source,
            kind: kind.to_string(),
            per_verb_timeout: Duration::from_secs(1),
            title: "T".into(),
            year: None,
            external_ids: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn resolve_manifest_routes_to_external_id_when_primary_does_not_match() {
        // TMDB-keyed entry on movies tab. xmdb declares id_sources = ["imdb"].
        // The catalog merge populated external_ids["imdb"] earlier; resolver
        // must hand xmdb the imdb id rather than the unrelated tmdb entry_id.
        let mut r = req("movies", IdSource::Tmdb, "12345");
        r.external_ids.insert("imdb".into(), "tt0111161".into());
        let got = resolve_id_for_plugin(&r, "xmdb", &["imdb".to_string()]);
        assert_eq!(got, Some(("tt0111161".into(), "imdb".into())));
    }

    #[test]
    fn resolve_manifest_skips_plugin_when_no_compatible_id_available() {
        // Imdb-required plugin but the entry is TMDB-only with no imdb hint.
        // Resolver returns None so the fan-out drops the plugin instead of
        // wasting a round-trip on a guaranteed UNKNOWN_ID.
        let r = req("movies", IdSource::Tmdb, "12345");
        let got = resolve_id_for_plugin(&r, "xmdb", &["imdb".to_string()]);
        assert_eq!(got, None);
    }

    #[test]
    fn resolve_manifest_uses_primary_when_id_source_matches_first_declared() {
        // Imdb-keyed entry hitting an imdb-only plugin: primary entry_id is
        // already the imdb id, no external_ids hop needed.
        let r = req("movies", IdSource::Imdb, "tt0111161");
        let got = resolve_id_for_plugin(&r, "omdb", &["imdb".to_string()]);
        assert_eq!(got, Some(("tt0111161".into(), "imdb".into())));
    }

    #[test]
    fn resolve_legacy_bool_form_uses_plugin_name_as_id_source() {
        // tmdb declares enrich = true (no id_sources) — legacy fallback path:
        // prefer external_ids["tmdb"], else the entry's primary id.
        let mut r = req("movies", IdSource::Imdb, "tt0111161");
        r.external_ids.insert("tmdb".into(), "999".into());
        let got = resolve_id_for_plugin(&r, "tmdb", &[]);
        assert_eq!(got, Some(("999".into(), "tmdb".into())));
    }

    #[test]
    fn resolve_fanart_routes_kind_conditionally() {
        let mut r = req("series", IdSource::Tmdb, "tmdbid");
        r.external_ids.insert("tvdb".into(), "12345".into());
        // Series → tvdb endpoint, even though primary id_source is tmdb.
        let got = resolve_id_for_plugin(&r, "fanart", &[]);
        assert_eq!(got, Some(("12345".into(), "tvdb".into())));
    }

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
    async fn timeout_caches_empty_with_short_ttl() {
        // A transient timeout (TMDB throttling, network blip) is now
        // cached as Empty under a short TTL (`NEGATIVE_TTL`, see
        // cache::metadata). That stops the runtime from re-hammering
        // the throttled provider on every detail re-open while keeping
        // the cache from being poisoned for the full 30-day positive
        // TTL — the negative entry expires in ~60 s and the next open
        // retries the fan-out.
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
            matches!(
                engine.cache().get(&key).await,
                Some(MetadataPayload::Empty)
            ),
            "timeout should cache Empty under the negative-TTL window"
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

    // ── fan_out_trailers / fan_out_box_office smoke tests ─────────────────────

    #[tokio::test]
    async fn fan_out_trailers_returns_first_success() {
        use test_dispatch::{PluginDispatch, trailers_resp};
        use std::collections::HashMap;

        let mut trailers = HashMap::new();
        trailers.insert("fake-p1".to_string(), trailers_resp("https://youtube.com/trailer1"));
        trailers.insert("fake-p2".to_string(), trailers_resp("https://youtube.com/trailer2"));

        let engine = PluginDispatch::new(trailers, HashMap::new(), HashMap::new(), HashMap::new());
        let req = TrailersRequest {
            id: "tt1".into(),
            id_source: "imdb".into(),
            kind: EntryKind::Movie,
            locale: None,
            force_refresh: false,
        };
        let result = fan_out_trailers(&engine, &["fake-p1".to_string(), "fake-p2".to_string()], req).await;
        assert!(result.is_ok());
        // First-wins: should be fake-p1's URL
        assert_eq!(result.unwrap().trailers[0].url, "https://youtube.com/trailer1");
    }

    #[tokio::test]
    async fn fan_out_box_office_all_fail_returns_err() {
        use test_dispatch::PluginDispatch;
        use std::collections::HashMap;

        let mut box_office = HashMap::new();
        box_office.insert("fake-p1".to_string(), Err::<BoxOfficeResponse, String>("upstream error".into()));
        box_office.insert("fake-p2".to_string(), Err("another error".into()));

        let engine = PluginDispatch::new(HashMap::new(), box_office, HashMap::new(), HashMap::new());
        let req = BoxOfficeRequest {
            id: "tt1".into(),
            id_source: "imdb".into(),
            kind: EntryKind::Movie,
            force_refresh: false,
        };
        let result = fan_out_box_office(&engine, &["fake-p1".to_string(), "fake-p2".to_string()], req).await;
        assert!(result.is_err());
    }

    // ── fan_out_keywords tests ────────────────────────────────────────────────

    fn kw_req() -> KeywordsRequest {
        KeywordsRequest { id: "tt1".into(), id_source: "imdb".into(), kind: EntryKind::Movie, force_refresh: false }
    }

    #[tokio::test]
    async fn keywords_merge_dedups_case_insensitively() {
        use test_dispatch::{PluginDispatch, keywords_resp};
        use std::collections::HashMap;

        let mut kw = HashMap::new();
        kw.insert("fake-p1".to_string(), keywords_resp(&["Thriller", "Drama"]));
        kw.insert("fake-p2".to_string(), keywords_resp(&["thriller", "Horror"])); // "thriller" is dup

        let engine = PluginDispatch::new(HashMap::new(), HashMap::new(), kw, HashMap::new());
        let result = fan_out_keywords(&engine, vec!["fake-p1".to_string(), "fake-p2".to_string()], kw_req()).await.unwrap();
        let names: Vec<_> = result.keywords.iter().map(|k| k.name.to_lowercase()).collect();
        // Should have thriller, drama, horror — not two "thriller"s
        assert_eq!(result.keywords.len(), 3);
        assert!(names.contains(&"thriller".to_string()));
        assert!(names.contains(&"drama".to_string()));
        assert!(names.contains(&"horror".to_string()));
    }

    #[tokio::test]
    async fn keywords_merge_round_robin_lets_late_providers_contribute() {
        // Plugin A returns 250 keywords (more than cap alone); Plugin B returns ["independent film"].
        // Round-robin ensures "independent film" gets a slot before cap is hit.
        use test_dispatch::{PluginDispatch, keywords_resp};
        use std::collections::HashMap;

        let many: Vec<String> = (0..250).map(|i| format!("keyword_{i}")).collect();
        let many_refs: Vec<&str> = many.iter().map(|s| s.as_str()).collect();

        let mut kw = HashMap::new();
        kw.insert("fake-p1".to_string(), keywords_resp(&many_refs));
        kw.insert("fake-p2".to_string(), keywords_resp(&["independent film"]));

        let engine = PluginDispatch::new(HashMap::new(), HashMap::new(), kw, HashMap::new());
        let result = fan_out_keywords(&engine, vec!["fake-p1".to_string(), "fake-p2".to_string()], kw_req()).await.unwrap();
        assert_eq!(result.keywords.len(), MAX_MERGED_KEYWORDS);
        let names: Vec<_> = result.keywords.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"independent film"), "independent film should be present due to round-robin");
    }

    #[tokio::test]
    async fn keywords_merge_caps_at_max() {
        use test_dispatch::{PluginDispatch, keywords_resp};
        use std::collections::HashMap;

        let many: Vec<String> = (0..300).map(|i| format!("kw_{i}")).collect();
        let many_refs: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
        let many2: Vec<String> = (300..600).map(|i| format!("kw_{i}")).collect();
        let many2_refs: Vec<&str> = many2.iter().map(|s| s.as_str()).collect();

        let mut kw = HashMap::new();
        kw.insert("fake-p1".to_string(), keywords_resp(&many_refs));
        kw.insert("fake-p2".to_string(), keywords_resp(&many2_refs));

        let engine = PluginDispatch::new(HashMap::new(), HashMap::new(), kw, HashMap::new());
        let result = fan_out_keywords(&engine, vec!["fake-p1".to_string(), "fake-p2".to_string()], kw_req()).await.unwrap();
        assert_eq!(result.keywords.len(), MAX_MERGED_KEYWORDS);
    }

    #[tokio::test]
    async fn keywords_merge_partial_failure_returns_partial() {
        // Plugin A errors, plugin B succeeds → Ok with B's keywords.
        use test_dispatch::{PluginDispatch, keywords_resp};
        use std::collections::HashMap;

        let mut kw = HashMap::new();
        kw.insert("fake-p1".to_string(), Err("upstream failed".into()));
        kw.insert("fake-p2".to_string(), keywords_resp(&["sci-fi", "space"]));

        let engine = PluginDispatch::new(HashMap::new(), HashMap::new(), kw, HashMap::new());
        let result = fan_out_keywords(&engine, vec!["fake-p1".to_string(), "fake-p2".to_string()], kw_req()).await;
        assert!(result.is_ok());
        let keywords = result.unwrap().keywords;
        let names: Vec<_> = keywords.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"sci-fi"));
        assert!(names.contains(&"space"));
    }

    #[tokio::test]
    async fn keywords_merge_all_failed_returns_err() {
        use test_dispatch::PluginDispatch;
        use std::collections::HashMap;

        let mut kw = HashMap::new();
        kw.insert("fake-p1".to_string(), Err::<KeywordsResponse, String>("fail1".into()));
        kw.insert("fake-p2".to_string(), Err("fail2".into()));

        let engine = PluginDispatch::new(HashMap::new(), HashMap::new(), kw, HashMap::new());
        let result = fan_out_keywords(&engine, vec!["fake-p1".to_string(), "fake-p2".to_string()], kw_req()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("all 2 keyword providers failed"));
    }

    #[tokio::test]
    async fn keywords_merge_provider_field_stamped() {
        use test_dispatch::{PluginDispatch, keywords_resp};
        use std::collections::HashMap;

        let mut kw = HashMap::new();
        kw.insert("fake-p1".to_string(), keywords_resp(&["action"]));
        kw.insert("fake-p2".to_string(), keywords_resp(&["comedy"]));

        let engine = PluginDispatch::new(HashMap::new(), HashMap::new(), kw, HashMap::new());
        let result = fan_out_keywords(&engine, vec!["fake-p1".to_string(), "fake-p2".to_string()], kw_req()).await.unwrap();
        for kw in &result.keywords {
            assert!(kw.provider.is_some(), "every keyword must have provider stamped; name={}", kw.name);
        }
        let action = result.keywords.iter().find(|k| k.name == "action").unwrap();
        assert_eq!(action.provider.as_deref(), Some("fake-p1"));
        let comedy = result.keywords.iter().find(|k| k.name == "comedy").unwrap();
        assert_eq!(comedy.provider.as_deref(), Some("fake-p2"));
    }
}
