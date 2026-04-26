//! `stui-runtime` library crate.
//!
//! Exposes all runtime modules so they can be imported by:
//!   - Integration tests in `runtime/tests/`
//!   - Future clients (REST gateway, remote control, etc.)
//!   - The `stui-sdk` crate (for type-sharing)
//!
//! The binary entry point (`src/main.rs`) uses `stui_runtime::*` just like
//! any external consumer would, keeping `main.rs` thin.
//!
//! # Feature flags
//!
//! | Flag | Description |
//! |------|-------------|
//! | `wasm-host` | Enable full WASM plugin execution via wasmtime |

pub mod abi;
pub mod aria2_bridge;
pub mod cache;
pub mod catalog;
pub mod catalog_engine;
pub mod config;
pub mod discovery;
pub mod engine;
pub mod error;
pub mod events;
pub mod ipc;
pub mod ipc_batcher;
pub mod logging;
pub mod media;
pub mod mpd_bridge;
pub mod player;
pub mod plugin;
pub mod providers;
pub mod quality;
pub mod resolver;
pub mod sandbox;
pub mod scraper;
pub mod stremio;
pub mod streamer;
pub mod tvdb;
pub mod anime_bridge;
pub mod storage;
pub mod watchhistory;
pub mod mediacache;
pub mod dsp;
pub mod roon;

// `pipeline` ties the stages together into a single orchestrated flow.
pub mod pipeline;
pub mod plugin_rpc;
pub mod registry;
pub mod skipper;
pub mod auth;


// Re-export the most commonly used types at crate root for convenience.
pub use config::RuntimeConfig;
pub use engine::Engine;
pub use error::StuidError;
pub use engine::Pipeline;
pub use events::{EventBus, RuntimeEvent};
pub use config::ConfigManager;
#[allow(unused_imports)]
pub use providers::{HealthRegistry, ProviderThrottle};
