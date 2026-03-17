//! Plugin ABI — versioned contract between host and WASM plugins.
//!
//! ## Modules
//! - `types`      — shared request/response types, ABI version constant
//! - `host`       — wasmtime host: loads .wasm, wires imports, calls exports
//! - `supervisor` — wraps WasmInstance with timeout, crash detection, reload

pub mod host;
pub mod supervisor;
pub mod types;

pub use host::{WasmHost, WasmInstance};
pub use supervisor::{WasmSupervisor, WasmSupervisorConfig, WasmSupervisorStats};
pub use types::*;
