//! Centralised logging / tracing initialisation for the stui runtime.
//!
//! Call `logging::init(&cfg.logging)` once at the top of `main()` before
//! any async work begins.  The subscriber writes to stderr so it doesn't
//! pollute the NDJSON IPC stream on stdout.
//!
//! # Log levels
//!
//! Controlled by `LoggingConfig::level` (from `stui.toml`) or the
//! `STUI_LOG` environment variable, which takes precedence.
//! Uses the standard `tracing` filter syntax, e.g.:
//!
//! ```
//! STUI_LOG=stui_runtime=debug,warn   # debug for this crate, warn elsewhere
//! STUI_LOG=debug                     # everything at debug
//! ```
//!
//! # File logging
//!
//! If `LoggingConfig::log_file` is set, a second layer appends structured
//! JSON logs to that file.  Useful for post-mortem debugging of daemon mode.

use std::path::Path;
use tracing::info;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

use crate::config::LoggingConfig;

/// Initialise the global tracing subscriber.
///
/// Must be called exactly once, before any `tracing::*` macros fire.
/// Subsequent calls are silently ignored (guard against double-init in tests).
pub fn init(cfg: &LoggingConfig) {
    // STUI_LOG env var takes precedence over config file level
    let filter_str = std::env::var("STUI_LOG")
        .unwrap_or_else(|_| cfg.level.clone());

    let filter = EnvFilter::try_new(&filter_str)
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_span_events(FmtSpan::NONE)
        .compact();

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer);

    // Silently ignore the error — double-init in tests is fine
    let _ = registry.try_init();

    info!("logging initialised (level={})", filter_str);
}

/// Convenience wrapper: init with just a level string (useful in tests).
pub fn init_with_level(level: &str) {
    let cfg = LoggingConfig {
        level:    level.to_string(),
        log_file: None,
    };
    init(&cfg);
}
