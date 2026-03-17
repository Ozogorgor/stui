//! Language-agnostic external plugin system using JSON-RPC over stdin/stdout.
//!
//! # Why RPC plugins?
//!
//! The WASM plugin system (via `plugin/` + `abi/`) requires plugins to be
//! compiled to WebAssembly, which means Rust-only authorship today.  The RPC
//! plugin system removes that constraint: **any language** can provide a
//! plugin by implementing a simple line-oriented JSON protocol.
//!
//! # Plugin authoring in any language
//!
//! A minimal Python stream provider:
//!
//! ```python
//! #!/usr/bin/env python3
//! import sys, json
//!
//! for line in sys.stdin:
//!     req = json.loads(line.strip())
//!     rid = req["id"]
//!
//!     if req["method"] == "handshake":
//!         print(json.dumps({"id": rid, "result": {
//!             "name": "my-provider", "version": "1.0",
//!             "capabilities": ["streams"]
//!         }}))
//!
//!     elif req["method"] == "streams.resolve":
//!         print(json.dumps({"id": rid, "result": [
//!             {"url": "magnet:?xt=urn:btih:abc123",
//!              "name": "1080p BluRay HEVC"}
//!         ]}))
//!
//!     elif req["method"] == "shutdown":
//!         print(json.dumps({"id": rid, "result": {}}))
//!         break
//!
//!     sys.stdout.flush()
//! ```
//!
//! # Coexistence with WASM plugins
//!
//! Both systems run side-by-side. The `Pipeline` collects results from built-in
//! providers, WASM plugins, Stremio addons, **and** RPC plugins before ranking.
//!
//! # Module structure
//!
//! ```
//! plugin_rpc/
//!   mod.rs        ← this file
//!   protocol.rs   ← wire types (RpcRequest, RpcResponse, PluginHandshake, …)
//!   process.rs    ← PluginProcess: spawn, handshake, call()
//!   supervisor.rs ← PluginSupervisor: restart, backoff, crash-loop, memory limit
//!   manager.rs    ← PluginRpcManager: discovery, routing, fan-out, shutdown
//! ```

pub mod manager;
pub mod process;
pub mod protocol;
pub mod supervisor;

pub use manager::PluginRpcManager;
pub use process::PluginProcess;
pub use protocol::{PluginHandshake, RpcRequest, RpcResponse};
pub use supervisor::{PluginSupervisor, SupervisorConfig, SupervisorStats};
