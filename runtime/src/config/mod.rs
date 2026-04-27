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
//! # Secrets Management
//!
//! API keys and passwords are loaded from:
//! 1. Environment variables (highest priority)
//! 2. `~/.stui/secrets.env` file
//!
//! See [`secrets`] module for details.
//!
//! # Structure
//!
//! ```text
//! config/
//!   mod.rs      - this file; re-exports load() and RuntimeConfig
//!   types.rs    - RuntimeConfig + nested structs (LoggingConfig, PlaybackConfig)
//!   loader.rs   - TOML file parsing + STUI_* env-var overrides
//!   secrets.rs  - Secure secrets loading from .env file
//!   manager.rs  - Live config hot-reload
//! ```

pub mod loader;
pub mod migrate;
pub mod types;
pub mod manager;
pub mod secrets;
pub mod secrets_enc;

pub use loader::load;
#[allow(unused_imports)]
pub use types::{PlaybackConfig, ProvidersConfig, StreamingConfig, SubtitlesConfig};
pub use types::{LoggingConfig, RuntimeConfig};
pub use manager::ConfigManager;
