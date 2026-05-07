//! Plugin subsystem: loader, state, dispatcher, supervisor.
//!
//! See `docs/superpowers/specs/2026-04-20-plugin-refactor-design.md` §2 for
//! architecture context. Split out of the monolithic `plugin.rs` in Task 1.7.

pub mod manifest;
pub mod loader;
pub mod state;
pub mod dispatcher;
pub mod supervisor;
pub mod rate_limit;

// ── Re-exports for external callers ───────────────────────────────────────────

pub use manifest::{
    ArtworkConfig, AuthorMeta, Capabilities, CatalogCapability, LookupConfig,
    ManifestValidationError, NetworkPermission, Permissions, PluginCapability, PluginConfigField,
    PluginManifest, PluginMeta, PluginMetaExt, PluginType, RateLimit, VerbConfig,
};
pub use loader::{load_from_dir, load_manifest, parse_manifest, resolve_entrypoint, ExecutionMode, LoadedPlugin, LoaderError};
pub use state::{resolve_config, PluginState, PluginStatus, StateStore};
pub use dispatcher::{Dispatcher, LoadedPluginSummary};
pub use supervisor::PluginSupervisor;
pub use rate_limit::TokenBucket;
