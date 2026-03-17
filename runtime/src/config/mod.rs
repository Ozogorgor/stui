//! Runtime configuration — loading, types, and env-var overrides.
//!
//! # Quick start
//!
//! ```rust
//! use stui_runtime::config;
//!
//! let cfg = config::load();           // reads ~/.stui/config/stui.toml + env vars
//! println!("{}", cfg.plugin_dir.display());
//! ```
//!
//! # Structure
//!
//! ```
//! config/
//!   mod.rs      — this file; re-exports load() and RuntimeConfig
//!   types.rs    — RuntimeConfig + nested structs (LoggingConfig, PlaybackConfig)
//!   loader.rs   — TOML file parsing + STUI_* env-var overrides
//! ```

pub mod loader;
pub mod types;
pub mod manager;

pub use loader::load;
pub use types::{LoggingConfig, PlaybackConfig, ProvidersConfig, StreamingConfig, SubtitlesConfig, RuntimeConfig};
pub use manager::ConfigManager;
