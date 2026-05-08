//! Plugin subsystem: loader, state, dispatcher, supervisor.
//!
//! See `docs/superpowers/specs/2026-04-20-plugin-refactor-design.md` §2 for
//! architecture context. Split out of the monolithic `plugin.rs` in Task 1.7.

pub mod dispatcher;
pub mod loader;
pub mod manifest;
pub mod rate_limit;
pub mod state;
pub mod supervisor;

// ── Re-exports for external callers ───────────────────────────────────────────

pub use dispatcher::{Dispatcher, LoadedPluginSummary};
pub use loader::{
    load_from_dir, load_manifest, parse_manifest, resolve_entrypoint, ExecutionMode, LoadedPlugin,
    LoaderError,
};
pub use manifest::{
    ArtworkConfig, AuthorMeta, Capabilities, CatalogCapability, LookupConfig,
    ManifestValidationError, NetworkPermission, Permissions, PluginCapability, PluginConfigField,
    PluginManifest, PluginMeta, PluginMetaExt, PluginType, RateLimit, VerbConfig,
};
pub use rate_limit::TokenBucket;
pub use state::{resolve_config, PluginState, PluginStatus, StateStore};
pub use supervisor::PluginSupervisor;
