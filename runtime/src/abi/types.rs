//! Stable ABI types — versioned JSON contract between the stui host and plugins.
//!
//! ## Versioning
//! `STUI_ABI_VERSION` is the host's maximum accepted ABI version.
//! Plugins with `abi_version <= STUI_ABI_VERSION` load cleanly (backward-compat
//! for v1 plugins under a v2+ host). Plugins with `abi_version > STUI_ABI_VERSION`
//! are rejected — the plugin was built against a newer host than is installed.
//!
//! ## Memory model
//! All data crosses the WASM boundary as UTF-8 JSON written into WASM linear
//! memory. The plugin owns its memory; the host reads through a shared view.
//!
//!   host → plugin:  host calls stui_alloc(len), writes JSON, calls fn(ptr,len)
//!   plugin → host:  fn returns (ptr, len) pointing into plugin memory;
//!                   host reads, then calls stui_free(ptr, len)
//!
//! ## Function exports (plugin must provide)
//! ```text
//! stui_abi_version() -> i32          version guard — must equal STUI_ABI_VERSION
//! stui_alloc(len: i32) -> i32        allocate len bytes, return ptr
//! stui_free(ptr: i32, len: i32)      free previously allocated region
//! stui_search(ptr: i32, len: i32) -> i64   packed (ptr<<32)|len of result JSON
//! stui_resolve(ptr: i32, len: i32) -> i64  packed (ptr<<32)|len of result JSON
//! ```
//!
//! ## Host imports (host provides, plugin may call)
//! ```text
//! stui_log(level: i32, ptr: i32, len: i32)
//! stui_http_get(url_ptr: i32, url_len: i32) -> i64   packed result ptr/len
//! stui_cache_get(key_ptr: i32, key_len: i32) -> i64
//! stui_cache_set(kp: i32, kl: i32, vp: i32, vl: i32)
//! stui_now_unix() -> i64                             seconds since UNIX epoch (v2+)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::{EntryKind, SearchScope};

/// Current ABI version. Bump this when making breaking changes.
/// v2 adds: `stui_now_unix`, trailers/release_info/keywords/box_office/alternative_titles verbs.
pub const STUI_ABI_VERSION: i32 = 2;

// ── Requests (host → plugin, serialized to JSON in WASM memory) ──────────────

/// Payload passed to `stui_search`. Mirrors sdk::SearchRequest exactly so the
/// host and plugin deserialize the same wire shape after plugin migration
/// (Task 7.1).
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub scope: SearchScope,
    pub page: u32,
    pub limit: u32,
    #[serde(default)]
    pub per_scope_limit: Option<u32>,
    #[serde(default)]
    pub locale: Option<String>,
}

/// Payload passed to `stui_resolve`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub entry_id: String,
}

// ── Responses (plugin → host, serialized to JSON in WASM memory) ─────────────

/// Returned by `stui_search`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub items: Vec<PluginEntry>,
    pub total: u32,
}

/// A single media entry returned by a plugin search. Mirrors sdk::PluginEntry
/// exactly — typed numeric fields, kind, source, and all per-kind optional
/// fields — so the JSON written by the host and the JSON read by the plugin
/// after Task 7.1 migration share the same wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PluginEntry {
    /// Provider-scoped unique id (used for resolve calls).
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    pub source: String,

    #[serde(default, skip_serializing_if = "Option::is_none")] pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub rating: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub poster_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub duration: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")] pub artist_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub album_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub episode: Option<u32>,
    /// Total seasons for series entries (populated by lookup/enrich on
    /// providers that have it; absent otherwise). Mirrors
    /// `stui_plugin_sdk::PluginEntry::season_count`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub season_count: Option<u32>,
    /// Per-season provider-native ids, parallel to seasons 1..=N. Used
    /// by providers (e.g. AniList) where each season is a separate
    /// catalog entry. Mirrors
    /// `stui_plugin_sdk::PluginEntry::season_ids`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub season_ids: Vec<String>,
    /// ISO 639-1 original language. Mirrors `stui_plugin_sdk::PluginEntry`.
    /// Used by the engine's anime-mix classifier alongside genre.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub original_language: Option<String>,
    /// Cross-provider native ids (e.g. `{"anilist": "5114", "kitsu": "1376"}`).
    /// Mirrors `stui_plugin_sdk::PluginEntry::external_ids` so plugins can
    /// fast-path enrich when their own native id is already known on a
    /// foreign-provider entry.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")] pub external_ids: HashMap<String, String>,
    /// Per-source rating breakdown — mirrors
    /// `stui_plugin_sdk::PluginEntry::ratings`. Plugins like OMDb that
    /// aggregate multiple providers in one response (IMDb + Rotten
    /// Tomatoes + Metacritic) populate this so the runtime's catalog
    /// aggregator can compose a weighted composite with full
    /// provenance. The keys must match the aggregator's RatingWeight
    /// names (e.g. `imdb`, `tomatometer`, `metacritic`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")] pub ratings: HashMap<String, f32>,

    /// Per-source vote count — mirror of
    /// `stui_plugin_sdk::PluginEntry::rating_votes`. When present the
    /// aggregator applies Bayesian shrinkage to the matching source's
    /// rating before composing the weighted median, suppressing tiny-
    /// sample 10.0s. Absent for sources that don't expose vote
    /// counts (e.g. RT critic / Metacritic critic).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")] pub rating_votes: HashMap<String, u32>,
}

/// Returned by `stui_resolve`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub stream_url: String,
    pub quality: Option<String>,
    pub subtitles: Vec<SubtitleTrack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub language: String,
    pub url: String,
    pub format: String, // "srt" | "vtt" | "ass"
}

/// Generic error envelope — plugins return this on failure.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

/// A result type that plugins return — either success payload or an error.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PluginResult<T> {
    Ok(T),
    Err(PluginError),
}

// ── Lookup ────────────────────────────────────────────────────────────────────

/// Payload passed to `stui_lookup`. Mirrors sdk::LookupRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub locale: Option<String>,
    #[serde(default)]
    pub force_refresh: bool,
}

/// Returned by `stui_lookup`. Mirrors sdk::LookupResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupResponse {
    pub entry: PluginEntry,
}

// ── Enrich ────────────────────────────────────────────────────────────────────

/// Payload passed to `stui_enrich`. Mirrors sdk::EnrichRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichRequest {
    pub partial: PluginEntry,
    pub prefer_id_source: Option<String>,
    #[serde(default)]
    pub force_refresh: bool,
}

/// Returned by `stui_enrich`. Mirrors sdk::EnrichResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichResponse {
    pub entry: PluginEntry,
    /// 0.0..=1.0 — plugin's own match-confidence score.
    pub confidence: f32,
}

// ── Artwork ───────────────────────────────────────────────────────────────────

/// Requested artwork resolution. Mirrors sdk::ArtworkSize exactly.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkSize {
    Thumbnail,
    Standard,
    HiRes,
    Any,
}

/// Payload passed to `stui_artwork`. Mirrors sdk::ArtworkRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtworkRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub size: ArtworkSize,
    #[serde(default)]
    pub force_refresh: bool,
}

/// One resolved artwork URL with its metadata. Mirrors sdk::ArtworkVariant exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtworkVariant {
    pub size: ArtworkSize,
    pub url: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Returned by `stui_artwork`. Mirrors sdk::ArtworkResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtworkResponse {
    pub variants: Vec<ArtworkVariant>,
}

// ── Credits ───────────────────────────────────────────────────────────────────

/// Payload passed to `stui_credits`. Mirrors sdk::CreditsRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditsRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub force_refresh: bool,
}

/// On-screen role for a cast member. Mirrors sdk::CastRole exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastRole {
    Actor,
    Vocalist,
    FeaturedArtist,
    GuestAppearance,
    Other(String),
}

/// A single cast credit. Mirrors sdk::CastMember exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    pub role: CastRole,
    pub character: Option<String>,
    pub instrument: Option<String>,
    pub billing_order: Option<u32>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_ids: HashMap<String, String>,
}

/// Behind-the-camera role. Mirrors sdk::CrewRole exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrewRole {
    Director,
    Writer,
    Producer,
    ExecutiveProducer,
    Cinematographer,
    Editor,
    Composer,
    Songwriter,
    Lyricist,
    Arranger,
    Instrumentalist,
    ProductionDesigner,
    ArtDirector,
    CostumeDesigner,
    SoundDesigner,
    VfxSupervisor,
    AnimationDirector,
    LeadAnimator,
    Other(String),
}

/// A single crew credit. Mirrors sdk::CrewMember exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewMember {
    pub name: String,
    pub role: CrewRole,
    pub department: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_ids: HashMap<String, String>,
}

/// Returned by `stui_credits`. Mirrors sdk::CreditsResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditsResponse {
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
}

// ── Related ───────────────────────────────────────────────────────────────────

/// Relationship kind requested. Mirrors sdk::RelationKind exactly.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    SameArtist,
    SameDirector,
    SameStudio,
    Similar,
    Sequel,
    Compilation,
    Any,
}

/// Payload passed to `stui_related`. Mirrors sdk::RelatedRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub relation: RelationKind,
    pub limit: u32,
    #[serde(default)]
    pub force_refresh: bool,
}

/// Returned by `stui_related`. Mirrors sdk::RelatedResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedResponse {
    pub items: Vec<PluginEntry>,
}

// ── Episodes ──────────────────────────────────────────────────────────────────

/// Payload passed to `stui_episodes`. Mirrors sdk::EpisodesRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodesRequest {
    pub series_id: String,
    pub id_source: String,
    pub season: u32,
}

/// One episode descriptor. Mirrors sdk::EpisodeWire exactly so what plugins
/// emit goes straight onto the IPC wire to the TUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeWire {
    pub season: u32,
    pub episode: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mins: Option<u32>,
    pub provider: String,
    pub entry_id: String,
}

/// Returned by `stui_episodes`. Mirrors sdk::EpisodesResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodesResponse {
    pub episodes: Vec<EpisodeWire>,
}

// ── FindStreams ───────────────────────────────────────────────────────────────

/// Payload passed to `stui_find_streams`. Mirrors sdk::FindStreamsRequest exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FindStreamsRequest {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(default)]
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub external_ids: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<String>,
}

/// Returned by `stui_find_streams`. Mirrors sdk::FindStreamsResponse exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindStreamsResponse {
    pub streams: Vec<Stream>,
}

/// One stream candidate from a StreamProvider. Mirrors sdk::Stream
/// exactly so the JSON wire shape across the WASM boundary stays
/// identical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Stream {
    pub url: String,
    pub title: String,
    pub provider: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub hdr: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubtitleTrack>,
}

// ── Host import payloads ──────────────────────────────────────────────────────

/// HTTP response returned by the `stui_http_get` host import.
#[derive(Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Log levels for the `stui_log` host import.
#[repr(i32)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info  = 2,
    Warn  = 3,
    Error = 4,
}

impl LogLevel {
    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Trace,
            1 => Self::Debug,
            3 => Self::Warn,
            4 => Self::Error,
            _ => Self::Info,
        }
    }
}

// ── ABI version check ─────────────────────────────────────────────────────────

/// Error returned when a plugin's ABI version doesn't match the host.
#[derive(Debug, thiserror::Error)]
pub enum AbiError {
    #[error("ABI version mismatch: plugin={plugin}, host={host}")]
    VersionMismatch { plugin: i32, host: i32 },

    #[error("plugin is missing required export: {0}")]
    MissingExport(String),

    #[error("WASM execution error: {0}")]
    Execution(String),

    #[error("JSON serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("memory error: {0}")]
    Memory(String),
}

// ── Init ──────────────────────────────────────────────────────────────────────

// Re-export the wire types from the SDK so callers don't double-import.
pub use stui_plugin_sdk::{InitRequest, InitResultEnvelope, PluginInitError};

/// Error returned from `WasmHost::init` / `WasmSupervisor::init`.
///
/// Split from [`AbiError`] because init has a distinct failure mode:
/// the plugin itself can report `MissingConfig` / `Fatal` via
/// [`PluginInitError`] without a traptime failure. Plumbing / ABI errors
/// (memory, missing export, serde failure, timeout) remain in the
/// `Abi` variant.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error(transparent)]
    Abi(#[from] AbiError),

    #[error("plugin init reported: {0:?}")]
    Plugin(PluginInitError),
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use stui_plugin_sdk::EntryKind;

    fn sample_entry() -> PluginEntry {
        PluginEntry {
            id: "tt1234567".into(),
            kind: EntryKind::Movie,
            title: "Test Movie".into(),
            source: "test_source".into(),
            ..Default::default()
        }
    }

    // ── LookupRequest / LookupResponse ────────────────────────────────────

    #[test]
    fn lookup_request_round_trip() {
        let req = LookupRequest {
            id: "tt0000001".into(),
            id_source: "imdb".into(),
            kind: EntryKind::Movie,
            locale: Some("en-US".into()),
            force_refresh: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: LookupRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn lookup_response_round_trip() {
        let resp = LookupResponse { entry: sample_entry() };
        let json = serde_json::to_string(&resp).unwrap();
        let back: LookupResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
    }

    // ── EnrichRequest / EnrichResponse ────────────────────────────────────

    #[test]
    fn enrich_request_round_trip() {
        let req = EnrichRequest {
            partial: sample_entry(),
            prefer_id_source: Some("tmdb".into()),
            force_refresh: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: EnrichRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn enrich_response_round_trip() {
        let resp = EnrichResponse {
            entry: sample_entry(),
            confidence: 0.95,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: EnrichResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
    }

    // ── ArtworkRequest / ArtworkResponse ──────────────────────────────────

    #[test]
    fn artwork_size_snake_case() {
        assert_eq!(serde_json::to_string(&ArtworkSize::HiRes).unwrap(), "\"hi_res\"");
        assert_eq!(serde_json::to_string(&ArtworkSize::Thumbnail).unwrap(), "\"thumbnail\"");
    }

    #[test]
    fn artwork_request_round_trip() {
        let req = ArtworkRequest {
            id: "tt0000002".into(),
            id_source: "tmdb".into(),
            kind: EntryKind::Movie,
            size: ArtworkSize::Standard,
            force_refresh: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ArtworkRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn artwork_response_round_trip() {
        let resp = ArtworkResponse {
            variants: vec![ArtworkVariant {
                size: ArtworkSize::Standard,
                url: "https://example.com/art.jpg".into(),
                mime: "image/jpeg".into(),
                width: Some(500),
                height: Some(750),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ArtworkResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
    }

    // ── CreditsRequest / CreditsResponse ──────────────────────────────────

    /// Verify `CastRole::Other("Narrator")` serializes as `{"other":"Narrator"}`
    /// (serde externally-tagged tuple variant with rename_all = "snake_case").
    /// Both the ABI and SDK produce this shape — it is the canonical wire format.
    #[test]
    fn cast_role_other_round_trip() {
        let role = CastRole::Other("Narrator".into());
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, r#"{"other":"Narrator"}"#);
        let back: CastRole = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);
    }

    #[test]
    fn crew_role_snake_case() {
        let s = serde_json::to_string(&CrewRole::VfxSupervisor).unwrap();
        assert_eq!(s, "\"vfx_supervisor\"");
    }

    #[test]
    fn crew_role_other_round_trip() {
        let role = CrewRole::Other("Foley Artist".into());
        let json = serde_json::to_string(&role).unwrap();
        let back: CrewRole = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);
    }

    #[test]
    fn credits_request_round_trip() {
        let req = CreditsRequest {
            id: "tt0000003".into(),
            id_source: "tmdb".into(),
            kind: EntryKind::Movie,
            force_refresh: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: CreditsRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn credits_response_round_trip() {
        let resp = CreditsResponse {
            cast: vec![CastMember {
                name: "Jane Doe".into(),
                role: CastRole::Actor,
                character: Some("Hero".into()),
                instrument: None,
                billing_order: Some(1),
                external_ids: HashMap::new(),
            }],
            crew: vec![CrewMember {
                name: "John Smith".into(),
                role: CrewRole::Director,
                department: Some("Directing".into()),
                external_ids: HashMap::new(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: CreditsResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
        // external_ids must be omitted from JSON when empty
        assert!(!json.contains("external_ids"));
    }

    // ── RelatedRequest / RelatedResponse ──────────────────────────────────

    #[test]
    fn relation_kind_snake_case() {
        assert_eq!(serde_json::to_string(&RelationKind::SameArtist).unwrap(), "\"same_artist\"");
        assert_eq!(serde_json::to_string(&RelationKind::SameDirector).unwrap(), "\"same_director\"");
    }

    #[test]
    fn related_request_round_trip() {
        let req = RelatedRequest {
            id: "tt0000004".into(),
            id_source: "tmdb".into(),
            kind: EntryKind::Movie,
            relation: RelationKind::Sequel,
            limit: 10,
            force_refresh: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: RelatedRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn related_response_round_trip() {
        let resp = RelatedResponse { items: vec![sample_entry()] };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RelatedResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
    }
}

#[cfg(test)]
mod crew_role_tests {
    use super::*;

    #[test]
    fn animation_director_deserializes_from_sdk_wire_format() {
        let wire = "\"animation_director\"";
        let parsed: CrewRole = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed, CrewRole::AnimationDirector);
    }

    #[test]
    fn lead_animator_deserializes_from_sdk_wire_format() {
        let wire = "\"lead_animator\"";
        let parsed: CrewRole = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed, CrewRole::LeadAnimator);
    }
}
