//! Request-handling pipelines — one module per concern.
//!
//! Each submodule exposes a single async entry-point function that
//! takes the relevant shared state and returns an IPC `Response`.
//! `main.rs` delegates directly to these; no business logic lives
//! in the dispatch loop itself.
//!
//! | Module      | Responsibility                                      |
//! |-------------|-----------------------------------------------------|
//! | `search`    | Fan-out to plugins, fallback to catalog filter      |
//! | `resolve`   | Rank stream candidates, map to wire types           |
//! | `playback`  | Fire player + skipper tasks; typed player commands  |
//! | `config`    | Live config updates, provider settings, plugin repos|
//! | `registry`  | Browse registry index + install plugins             |

pub mod config;
pub mod playback;
pub mod registry;
pub mod resolve;
pub mod search;
