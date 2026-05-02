//! Request/response types for `CatalogPlugin` verbs, plus lifecycle + helpers.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::kinds::EntryKind;
use crate::manifest::{ManifestValidationError, PluginManifest};
use crate::{PluginEntry, PluginResult};

// ── InitContext ───────────────────────────────────────────────────────────────

/// Context passed to `Plugin::init`. Carries resolved env, config, cache dir,
/// and a logger handle.
///
/// The `logger` field is NOT serializable — it is attached on the plugin side
/// after deserializing the wire-format [`InitRequest`] via
/// [`InitContext::from_request`].
///
/// `config` is a `HashMap<String, serde_json::Value>` so plugins can read
/// values via the same serde-json helpers they already use for HTTP
/// response parsing (`.as_str()`, `.as_i64()`, `.as_bool()`), without
/// pulling the `toml` crate just to touch their own config.
pub struct InitContext<'a> {
    pub env: &'a HashMap<String, String>,
    pub config: &'a HashMap<String, serde_json::Value>,
    pub cache_dir: &'a PathBuf,
    pub logger: &'a dyn PluginLogger,
}

impl<'a> InitContext<'a> {
    /// Build an `InitContext` from a deserialized [`InitRequest`] plus a
    /// logger handle. The plugin side reassembles the context this way because
    /// the `logger` trait-object cannot cross the ABI boundary.
    pub fn from_request(req: &'a InitRequest, logger: &'a dyn PluginLogger) -> Self {
        Self {
            env: &req.env,
            config: &req.config,
            cache_dir: &req.cache_dir,
            logger,
        }
    }
}

/// Wire-format payload for `stui_init`. This is the serializable subset of
/// [`InitContext`] — the `logger` is attached after deserialization via
/// [`InitContext::from_request`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InitRequest {
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub cache_dir: PathBuf,
}

/// Default `PluginLogger` used on the plugin side after deserializing an
/// [`InitRequest`]. Routes to `host_log` when running under WASM, falls back
/// to `eprintln!` on the host (tests).
pub struct DefaultPluginLogger;

impl PluginLogger for DefaultPluginLogger {
    fn debug(&self, msg: &str) { crate::host_log(1, msg); }
    fn info(&self, msg: &str)  { crate::host_log(2, msg); }
    fn warn(&self, msg: &str)  { crate::host_log(3, msg); }
    fn error(&self, msg: &str) { crate::host_log(4, msg); }
}

/// Logging surface exposed to plugins (backed by `stui_log` host import at runtime,
/// no-op or stdout in test harness).
pub trait PluginLogger {
    fn debug(&self, msg: &str);
    fn info(&self, msg: &str);
    fn warn(&self, msg: &str);
    fn error(&self, msg: &str);
}

/// Result of `Plugin::init`. `MissingConfig` is soft — user-fixable via TUI;
/// `Fatal` is hard — code bug or trap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginInitError {
    MissingConfig {
        fields: Vec<String>,
        hint: Option<String>,
    },
    Fatal(String),
}

/// Wire-format envelope for the plugin-side response from `stui_init`.
///
/// Mirrors the shape of [`crate::PluginResult`] but with a fixed success
/// type of `()` — `init` never carries a success payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InitResultEnvelope {
    Ok,
    Err(PluginInitError),
}

impl From<Result<(), PluginInitError>> for InitResultEnvelope {
    fn from(r: Result<(), PluginInitError>) -> Self {
        match r {
            Ok(())  => Self::Ok,
            Err(e)  => Self::Err(e),
        }
    }
}

impl From<InitResultEnvelope> for Result<(), PluginInitError> {
    fn from(e: InitResultEnvelope) -> Self {
        match e {
            InitResultEnvelope::Ok     => Ok(()),
            InitResultEnvelope::Err(e) => Err(e),
        }
    }
}

// ── Lookup ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub locale: Option<String>,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupResponse {
    pub entry: PluginEntry,
}

// ── Enrich ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichRequest {
    pub partial: PluginEntry,
    pub prefer_id_source: Option<String>,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichResponse {
    pub entry: PluginEntry,
    /// 0.0..=1.0 — plugin's own match-confidence score.
    pub confidence: f32,
}

// ── Artwork ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkSize {
    Thumbnail,
    Standard,
    HiRes,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub size: ArtworkSize,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkVariant {
    pub size: ArtworkSize,
    pub url: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkResponse {
    pub variants: Vec<ArtworkVariant>,
}

// ── Credits ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastRole {
    Actor,
    Vocalist,
    FeaturedArtist,
    GuestAppearance,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    pub role: CastRole,
    pub character: Option<String>,
    pub instrument: Option<String>,
    pub billing_order: Option<u32>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_ids: HashMap<String, String>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewMember {
    pub name: String,
    pub role: CrewRole,
    pub department: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_ids: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsResponse {
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
}

/// Normalize upstream crew-role strings into canonical `CrewRole` variants.
/// Unrecognized strings map to `CrewRole::Other(s)`.
pub fn normalize_crew_role(s: &str) -> CrewRole {
    match s.to_lowercase().as_str() {
        "director" => CrewRole::Director,
        "writer" | "screenplay" | "screenwriter" => CrewRole::Writer,
        "producer" => CrewRole::Producer,
        "executive producer" => CrewRole::ExecutiveProducer,
        "cinematographer" | "director of photography" | "dp" | "dop" => CrewRole::Cinematographer,
        "editor" => CrewRole::Editor,
        "composer" | "original music composer" => CrewRole::Composer,
        "songwriter" => CrewRole::Songwriter,
        "lyricist" => CrewRole::Lyricist,
        "arranger" => CrewRole::Arranger,
        "instrumentalist" | "session musician" => CrewRole::Instrumentalist,
        "production designer" => CrewRole::ProductionDesigner,
        "art director" => CrewRole::ArtDirector,
        "costume designer" => CrewRole::CostumeDesigner,
        "sound designer" => CrewRole::SoundDesigner,
        "vfx supervisor" | "visual effects supervisor" => CrewRole::VfxSupervisor,
        "animation director" | "anime director" => CrewRole::AnimationDirector,
        "lead animator" | "chief animation director" | "sakuga director" => CrewRole::LeadAnimator,
        _ => CrewRole::Other(s.to_string()),
    }
}

// ── Related ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    pub relation: RelationKind,
    pub limit: u32,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedResponse {
    pub items: Vec<PluginEntry>,
}

// ── Episodes verb ─────────────────────────────────────────────────────────────

/// Request for one season's episode list.
///
/// `season` is the natural-numbered season (e.g. 1, 2, 3 — never 0 for
/// "Specials" since most providers shape that differently and stui's
/// EpisodeScreen treats it as out-of-band).  `id_source` is included so
/// providers that key by foreign ids (e.g. OMDb on imdb) can refuse a
/// request meant for a different namespace cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodesRequest {
    pub series_id: String,
    pub id_source: String,
    pub season: u32,
}

/// Single episode descriptor. Mirrors the TUI's `ipc.EpisodeEntry` shape;
/// the runtime forwards each item straight through to the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeWire {
    pub season: u32,
    pub episode: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mins: Option<u32>,
    pub provider: String,
    /// Provider-native id for the individual episode (used later to
    /// resolve streams).  When the provider doesn't expose a per-episode
    /// id, plugins should synthesise `<series_id>:s<season>e<episode>`.
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodesResponse {
    pub episodes: Vec<EpisodeWire>,
}

// ── Trailers ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailersRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrailerKind {
    Trailer,
    Teaser,
    Clip,
    Featurette,
    BehindTheScenes,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trailer {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub kind: TrailerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailersResponse {
    pub trailers: Vec<Trailer>,
}

// ── Release info ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfoRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseKind {
    Premiere,
    Theatrical,
    Limited,
    Streaming,
    Digital,
    Physical,
    Tv,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseEntry {
    pub country: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_kind: Option<ReleaseKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfoResponse {
    pub releases: Vec<ReleaseEntry>,
}

// ── Keywords ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordsRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Populated by the engine post-merge. Plugins leave None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordsResponse {
    pub keywords: Vec<Keyword>,
}

// ── Box office ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxOfficeRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoneyAmount {
    /// Whole units of `currency` (not cents/decimals). Box-office
    /// figures are typically rounded; if a provider returns
    /// fractional amounts, round before constructing.
    pub amount: u64,
    /// ISO 4217 code (e.g. "USD", "EUR", "JPY").
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxOfficeResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<MoneyAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_weekend: Option<MoneyAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gross_domestic: Option<MoneyAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gross_worldwide: Option<MoneyAmount>,
}

// ── Alternative titles ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeTitlesRequest {
    pub id: String,
    pub id_source: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeTitle {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Free-form provider label (e.g. `"AKA"`, `"working title"`,
    /// `"international"`, `"original title"`). Distinct from the
    /// request's `EntryKind` — this is a per-row classification of
    /// the alternative title itself, not the work's kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeTitlesResponse {
    pub titles: Vec<AlternativeTitle>,
}

// ── Bulk enrich ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkEnrichRequest {
    /// Partial entries to enrich, one per call. Each entry should
    /// carry at least the fields the plugin needs to identify a row
    /// (typically `imdb_id` or `external_ids["imdb"]`).
    pub partials: Vec<PluginEntry>,
    /// Single `prefer_id_source` shared across the batch. Same
    /// semantics as `EnrichRequest.prefer_id_source` — advisory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer_id_source: Option<String>,
    /// Single `force_refresh` flag shared across the batch.
    /// Forwarded into the underlying enrichment path's cache layer.
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkEnrichEntry {
    /// Stable correlator that ties this output entry back to a
    /// specific input partial. Convention: take from
    /// `partial.imdb_id` if present, otherwise `partial.id`.
    /// Plugins may reorder or omit entries; callers reconcile by
    /// matching `id`.
    pub id: String,
    /// Per-entry result. Failures are reported here, NOT at the
    /// outer `BulkEnrichResponse` level — partial success is the
    /// expected mode.
    pub result: PluginResult<EnrichResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkEnrichResponse {
    pub entries: Vec<BulkEnrichEntry>,
}

// ── err_not_implemented helper ────────────────────────────────────────────────

/// Canonical helper for default-method bodies on optional `CatalogPlugin`
/// verbs. Returns a `PluginResult::Err` with the `NOT_IMPLEMENTED` code.
pub fn err_not_implemented<T>() -> PluginResult<T> {
    PluginResult::err(
        crate::error_codes::NOT_IMPLEMENTED,
        "verb not implemented by this plugin",
    )
}

// ── Manifest validator (used by CLI lint/build) ───────────────────────────────

/// Validate a freshly-parsed manifest against the canonical schema.
///
/// Thin delegator to [`crate::manifest::validate`] — the authoritative
/// validator lives alongside the manifest types. This name is kept here as a
/// stable entry point for the CLI (`stui plugin lint` / `stui plugin build`)
/// so call sites like `stui_plugin_sdk::capabilities::validate_manifest(&m)`
/// continue to compile.
pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), ManifestValidationError> {
    crate::manifest::validate(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_crew_role_common_aliases() {
        assert!(matches!(normalize_crew_role("Director"), CrewRole::Director));
        assert!(matches!(normalize_crew_role("director of photography"), CrewRole::Cinematographer));
        assert!(matches!(normalize_crew_role("DOP"), CrewRole::Cinematographer));
        assert!(matches!(normalize_crew_role("Original Music Composer"), CrewRole::Composer));
    }

    #[test]
    fn normalize_crew_role_unknown_is_other() {
        match normalize_crew_role("Foley Artist") {
            CrewRole::Other(s) => assert_eq!(s, "Foley Artist"),
            _ => panic!("expected Other variant"),
        }
    }

    #[test]
    fn plugin_init_error_serde_tagged() {
        let e = PluginInitError::MissingConfig {
            fields: vec!["api_key".into()],
            hint: Some("Get a key at example.com".into()),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"kind\":\"missing_config\""));
        assert!(s.contains("api_key"));
    }

    #[test]
    fn init_request_round_trips_through_json() {
        let mut env = HashMap::new();
        env.insert("TMDB_API_KEY".into(), "secret".into());

        let mut config: HashMap<String, serde_json::Value> = HashMap::new();
        config.insert("api_key".into(), serde_json::Value::String("secret".into()));

        let req = InitRequest {
            env,
            config,
            cache_dir: std::path::PathBuf::from("/tmp/cache/tmdb"),
        };

        let json = serde_json::to_string(&req).unwrap();
        let back: InitRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.env.get("TMDB_API_KEY").map(String::as_str), Some("secret"));
        assert_eq!(
            back.config.get("api_key").and_then(|v| v.as_str()),
            Some("secret"),
        );
        assert_eq!(back.cache_dir, std::path::PathBuf::from("/tmp/cache/tmdb"));
    }

    #[test]
    fn init_result_envelope_round_trips_ok() {
        let e: InitResultEnvelope = Ok::<(), PluginInitError>(()).into();
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"status\":\"ok\""), "got {s}");
        let back: InitResultEnvelope = serde_json::from_str(&s).unwrap();
        let r: Result<(), PluginInitError> = back.into();
        assert!(r.is_ok());
    }

    #[test]
    fn init_result_envelope_round_trips_missing_config() {
        let e: InitResultEnvelope = Err::<(), _>(PluginInitError::MissingConfig {
            fields: vec!["api_key".into()],
            hint: None,
        }).into();
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"status\":\"err\""));
        let back: InitResultEnvelope = serde_json::from_str(&s).unwrap();
        let r: Result<(), PluginInitError> = back.into();
        match r {
            Err(PluginInitError::MissingConfig { fields, .. }) => {
                assert_eq!(fields, vec!["api_key".to_string()]);
            }
            _ => panic!("expected MissingConfig"),
        }
    }

    #[test]
    fn init_context_from_request_attaches_logger() {
        // Non-WASM host path: DefaultPluginLogger is a ZST that routes to
        // eprintln! outside of WASM, so this test just verifies the shape
        // and that the borrow-through fields match.
        let req = InitRequest {
            env: HashMap::from([("K".to_string(), "V".to_string())]),
            config: HashMap::new(),
            cache_dir: std::path::PathBuf::from("/tmp"),
        };
        let logger = DefaultPluginLogger;
        let ctx = InitContext::from_request(&req, &logger);
        assert_eq!(ctx.env.get("K").map(String::as_str), Some("V"));
        assert_eq!(ctx.cache_dir, &std::path::PathBuf::from("/tmp"));
    }

    #[test]
    fn err_not_implemented_returns_error() {
        let r: PluginResult<i32> = err_not_implemented();
        match r {
            PluginResult::Err(e) => {
                assert_eq!(e.code, crate::error_codes::NOT_IMPLEMENTED);
            }
            _ => panic!("expected Err"),
        }
    }

    #[test]
    fn artwork_size_serializes_snake_case() {
        let s = serde_json::to_string(&ArtworkSize::HiRes).unwrap();
        assert_eq!(s, "\"hi_res\"");
    }

    #[test]
    fn cast_role_other_variant_preserves_string() {
        let r = CastRole::Other("Extra".to_string());
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("Extra"));
        let back: CastRole = serde_json::from_str(&s).unwrap();
        if let CastRole::Other(x) = back {
            assert_eq!(x, "Extra");
        } else {
            panic!("round-trip lost Other variant");
        }
    }

    #[test]
    fn crew_role_animation_director_round_trips() {
        let v = CrewRole::AnimationDirector;
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, "\"animation_director\"");
        let back: CrewRole = serde_json::from_str(&s).unwrap();
        assert_eq!(back, CrewRole::AnimationDirector);
    }

    #[test]
    fn crew_role_lead_animator_round_trips() {
        let v = CrewRole::LeadAnimator;
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, "\"lead_animator\"");
        let back: CrewRole = serde_json::from_str(&s).unwrap();
        assert_eq!(back, CrewRole::LeadAnimator);
    }

    #[test]
    fn normalize_anime_director_variants() {
        assert_eq!(normalize_crew_role("animation director"), CrewRole::AnimationDirector);
        assert_eq!(normalize_crew_role("Animation Director"), CrewRole::AnimationDirector);
        assert_eq!(normalize_crew_role("anime director"),     CrewRole::AnimationDirector);
    }

    #[test]
    fn normalize_lead_animator_variants() {
        assert_eq!(normalize_crew_role("lead animator"),            CrewRole::LeadAnimator);
        assert_eq!(normalize_crew_role("chief animation director"), CrewRole::LeadAnimator);
        assert_eq!(normalize_crew_role("sakuga director"),          CrewRole::LeadAnimator);
    }

    #[test]
    fn normalize_preserves_other_fallthrough() {
        assert_eq!(normalize_crew_role("key animator"), CrewRole::Other("key animator".into()));
    }

    #[test]
    fn trailer_kind_serializes_snake_case() {
        let v = TrailerKind::BehindTheScenes;
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, "\"behind_the_scenes\"");
    }

    #[test]
    fn trailer_kind_other_round_trips() {
        let v = TrailerKind::Other("FanEdit".to_string());
        let s = serde_json::to_string(&v).unwrap();
        let back: TrailerKind = serde_json::from_str(&s).unwrap();
        if let TrailerKind::Other(x) = back { assert_eq!(x, "FanEdit"); }
        else { panic!("lost Other variant"); }
    }

    #[test]
    fn trailers_request_round_trips() {
        let req = TrailersRequest {
            id: "tt0111161".into(),
            id_source: "imdb".into(),
            kind: EntryKind::Movie,
            locale: Some("en-US".into()),
            force_refresh: false,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: TrailersRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "tt0111161");
        assert_eq!(back.force_refresh, false);
    }

    #[test]
    fn trailers_request_force_refresh_defaults_to_false() {
        let json = r#"{"id":"tt1","id_source":"imdb","kind":"movie"}"#;
        let req: TrailersRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
        assert!(req.locale.is_none());
    }

    #[test]
    fn release_kind_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&ReleaseKind::Theatrical).unwrap(),
                   "\"theatrical\"");
        assert_eq!(serde_json::to_string(&ReleaseKind::Tv).unwrap(),
                   "\"tv\"");
    }

    #[test]
    fn release_info_response_round_trips() {
        let resp = ReleaseInfoResponse {
            releases: vec![ReleaseEntry {
                country: "US".into(),
                date: Some("1994-09-23".into()),
                release_kind: Some(ReleaseKind::Theatrical),
                certificate: Some("R".into()),
                note: None,
            }],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: ReleaseInfoResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.releases.len(), 1);
        assert_eq!(back.releases[0].country, "US");
    }

    #[test]
    fn keyword_provider_field_round_trips() {
        let kw = Keyword {
            name: "indie".into(),
            source_id: Some("xmdb-kw-42".into()),
            provider: Some("xmdb".into()),
        };
        let s = serde_json::to_string(&kw).unwrap();
        let back: Keyword = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "indie");
        assert_eq!(back.provider.as_deref(), Some("xmdb"));
    }

    #[test]
    fn keyword_provider_field_omitted_when_none() {
        let kw = Keyword {
            name: "indie".into(),
            source_id: None,
            provider: None,
        };
        let s = serde_json::to_string(&kw).unwrap();
        assert!(!s.contains("provider"));
        assert!(!s.contains("source_id"));
    }

    #[test]
    fn money_amount_round_trips() {
        let m = MoneyAmount { amount: 25_000_000, currency: "USD".into() };
        let s = serde_json::to_string(&m).unwrap();
        let back: MoneyAmount = serde_json::from_str(&s).unwrap();
        assert_eq!(back.amount, 25_000_000);
        assert_eq!(back.currency, "USD");
    }

    #[test]
    fn box_office_response_with_partial_fields() {
        let resp = BoxOfficeResponse {
            budget: Some(MoneyAmount { amount: 25_000_000, currency: "USD".into() }),
            opening_weekend: None,
            gross_domestic: None,
            gross_worldwide: Some(MoneyAmount { amount: 73_341_414, currency: "USD".into() }),
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("budget"));
        assert!(!s.contains("opening_weekend"));
        let back: BoxOfficeResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.budget.unwrap().amount, 25_000_000);
    }

    #[test]
    fn alternative_titles_response_round_trips() {
        let resp = AlternativeTitlesResponse {
            titles: vec![AlternativeTitle {
                title: "Les Évadés".into(),
                locale: Some("fr-FR".into()),
                country: Some("FR".into()),
                kind: Some("AKA".into()),
            }],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: AlternativeTitlesResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.titles[0].title, "Les Évadés");
    }

    #[test]
    fn trailer_kind_partial_eq() {
        assert_eq!(TrailerKind::Trailer, TrailerKind::Trailer);
        assert_ne!(TrailerKind::Trailer, TrailerKind::Teaser);
        assert_eq!(
            TrailerKind::Other("FanEdit".into()),
            TrailerKind::Other("FanEdit".into()),
        );
    }

    #[test]
    fn release_kind_partial_eq() {
        assert_eq!(ReleaseKind::Theatrical, ReleaseKind::Theatrical);
        assert_ne!(ReleaseKind::Theatrical, ReleaseKind::Streaming);
    }

    #[test]
    fn enrich_request_force_refresh_defaults_false() {
        let json = r#"{"partial":{"id":"x","kind":"movie","title":"T","source":"s"}}"#;
        let req: EnrichRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
    }

    #[test]
    fn artwork_request_force_refresh_defaults_false() {
        let json = r#"{"id":"x","id_source":"imdb","kind":"movie","size":"standard"}"#;
        let req: ArtworkRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
    }

    #[test]
    fn credits_request_force_refresh_defaults_false() {
        let json = r#"{"id":"x","id_source":"imdb","kind":"movie"}"#;
        let req: CreditsRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
    }

    #[test]
    fn related_request_force_refresh_defaults_false() {
        let json = r#"{"id":"x","id_source":"imdb","kind":"movie","relation":"similar","limit":10}"#;
        let req: RelatedRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
    }

    #[test]
    fn lookup_request_force_refresh_defaults_false() {
        let json = r#"{"id":"x","id_source":"imdb","kind":"movie"}"#;
        let req: LookupRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
    }

    #[test]
    fn bulk_enrich_request_round_trips_serde() {
        let req = BulkEnrichRequest {
            partials: vec![PluginEntry {
                id: "tt0111161".into(),
                kind: EntryKind::Movie,
                title: "Shawshank".into(),
                source: "test".into(),
                imdb_id: Some("tt0111161".into()),
                ..Default::default()
            }],
            prefer_id_source: Some("imdb".into()),
            force_refresh: true,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: BulkEnrichRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.partials.len(), 1);
        assert_eq!(back.partials[0].imdb_id.as_deref(), Some("tt0111161"));
        assert_eq!(back.prefer_id_source.as_deref(), Some("imdb"));
        assert!(back.force_refresh);
    }

    #[test]
    fn bulk_enrich_request_force_refresh_defaults_false() {
        let json = r#"{"partials":[]}"#;
        let req: BulkEnrichRequest = serde_json::from_str(json).unwrap();
        assert!(!req.force_refresh);
        assert!(req.prefer_id_source.is_none());
    }

    #[test]
    fn bulk_enrich_response_round_trips_with_mixed_results() {
        let resp = BulkEnrichResponse {
            entries: vec![
                BulkEnrichEntry {
                    id: "tt0111161".into(),
                    result: PluginResult::ok(EnrichResponse {
                        entry: PluginEntry {
                            id: "tt0111161".into(),
                            kind: EntryKind::Movie,
                            title: "Shawshank".into(),
                            source: "test".into(),
                            ..Default::default()
                        },
                        confidence: 1.0,
                    }),
                },
                BulkEnrichEntry {
                    id: "tt9999999".into(),
                    result: PluginResult::err(
                        crate::error_codes::UNKNOWN_ID, "no such id"),
                },
            ],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: BulkEnrichResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].id, "tt0111161");
        match &back.entries[0].result {
            PluginResult::Ok(r) => assert!((r.confidence - 1.0).abs() < f32::EPSILON),
            _ => panic!("entry 0 should be Ok"),
        }
        match &back.entries[1].result {
            PluginResult::Err(e) => assert_eq!(e.code, crate::error_codes::UNKNOWN_ID),
            _ => panic!("entry 1 should be Err"),
        }
    }
}
