//! WASM host — executes plugin `.wasm` files through wasmtime.
//!
//! ## Architecture
//! Each plugin gets its own `wasmtime::Store` so state is fully isolated.
//! The store is created once at load time and reused across calls (the WASM
//! module is stateful — it holds its own allocator, HTTP cache, etc.).
//!
//! ## Feature flag
//! Full wasmtime execution is behind the `wasm-host` feature flag.
//! Without it, every call returns `AbiError::Execution("wasm-host feature
//! not enabled")` — clean and explicit, never a panic.
//!
//! To enable: add `--features wasm-host` to `cargo build`.

use std::path::Path;

use tracing::{debug, info};

use super::types::*;
use crate::sandbox::SandboxCtx;

// ── Public interface ──────────────────────────────────────────────────────────

/// A loaded, ready-to-call WASM plugin instance.
///
/// Created by `WasmHost::load()`, dropped when the plugin is unloaded.
pub struct WasmInstance {
    inner: WasmInner,
    pub plugin_name: String,
    pub abi_version: i32,
}

impl WasmInstance {
    /// Call the plugin's `stui_search` export.
    pub async fn search(&mut self, req: &SearchRequest) -> Result<SearchResponse, AbiError> {
        let json = serde_json::to_string(req)?;
        let raw = self.inner.call_export("stui_search", &json).await?;
        let result: PluginResult<SearchResponse> = serde_json::from_str(&raw)?;
        match result {
            PluginResult::Ok(r) => Ok(r),
            PluginResult::Err(e) => Err(AbiError::Execution(format!("{}: {}", e.code, e.message))),
        }
    }

    /// Call the plugin's `stui_resolve` export.
    pub async fn resolve(&mut self, req: &ResolveRequest) -> Result<ResolveResponse, AbiError> {
        let json = serde_json::to_string(req)?;
        let raw = self.inner.call_export("stui_resolve", &json).await?;
        let result: PluginResult<ResolveResponse> = serde_json::from_str(&raw)?;
        match result {
            PluginResult::Ok(r) => Ok(r),
            PluginResult::Err(e) => Err(AbiError::Execution(format!("{}: {}", e.code, e.message))),
        }
    }
}

// ── Host loader ───────────────────────────────────────────────────────────────

pub struct WasmHost;

impl WasmHost {
    /// Load a WASM plugin from disk, verify ABI version, wire host imports.
    ///
    /// `max_memory_mb` caps the plugin's linear memory via a `ResourceLimiter`.
    /// Exceeding the limit raises a wasmtime `Trap`, which `WasmSupervisor`
    /// catches and turns into a reload cycle.
    ///
    /// Returns a `WasmInstance` ready to call, or an `AbiError` explaining
    /// exactly why loading failed.
    pub async fn load(
        wasm_path:     &Path,
        plugin_name:   &str,
        ctx:           &SandboxCtx,
        max_memory_mb: u64,
    ) -> Result<WasmInstance, AbiError> {
        info!(plugin = %plugin_name, path = %wasm_path.display(), max_memory_mb, "loading WASM plugin");
        let inner = WasmInner::load(wasm_path, plugin_name, ctx, max_memory_mb).await?;
        let abi_version = inner.abi_version();
        if abi_version != STUI_ABI_VERSION {
            return Err(AbiError::VersionMismatch {
                plugin: abi_version,
                host: STUI_ABI_VERSION,
            });
        }
        debug!(plugin = %plugin_name, abi = abi_version, "WASM plugin loaded and ABI verified");
        Ok(WasmInstance {
            inner,
            plugin_name: plugin_name.to_string(),
            abi_version,
        })
    }
}

// ── Inner — feature-gated implementation ─────────────────────────────────────

#[cfg(feature = "wasm-host")]
mod inner_impl {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use tracing::{debug, warn};
    use wasmtime::*;
    use wasmtime_wasi::WasiCtxBuilder;
    use wasmtime_wasi::preview1::WasiP1Ctx;

    use super::*;
    use crate::sandbox::SandboxCtx;

    pub struct WasmInner {
        store: Mutex<Store<HostState>>,
        instance: Instance,
        engine: Engine,
    }

    /// Enforces a hard cap on the WASM module's linear memory.
    /// When `memory_growing` returns `false` wasmtime raises a `Trap`.
    struct MemoryLimiter {
        limit_bytes: usize,
    }

    impl wasmtime::ResourceLimiter for MemoryLimiter {
        fn memory_growing(
            &mut self,
            _current: usize,
            desired: usize,
            _maximum: Option<usize>,
        ) -> anyhow::Result<bool> {
            if desired > self.limit_bytes {
                warn!(
                    desired_mb = desired / (1024 * 1024),
                    limit_mb   = self.limit_bytes / (1024 * 1024),
                    "WASM memory limit exceeded — trapping",
                );
                Ok(false)
            } else {
                Ok(true)
            }
        }

        fn table_growing(
            &mut self,
            _current: u32,
            _desired: u32,
            _maximum: Option<u32>,
        ) -> anyhow::Result<bool> {
            Ok(true)
        }
    }

    struct HostState {
        wasi: WasiP1Ctx,
        ctx: SandboxCtx,
        /// Reusable buffer for HTTP responses written into plugin memory
        http_buf: Vec<u8>,
        /// Per-plugin KV cache — persists across calls within a session.
        /// Keys starting with "__env:" are pre-populated from plugin.toml [env].
        kv: std::collections::HashMap<String, String>,
        /// Memory limiter — enforces the max_memory_mb cap from SupervisorConfig.
        limiter: MemoryLimiter,
    }

    impl WasmInner {
        pub async fn load(
            wasm_path:     &Path,
            plugin_name:   &str,
            ctx:           &SandboxCtx,
            max_memory_mb: u64,
        ) -> Result<Self, AbiError> {
            let mut config = Config::new();
            config.async_support(true);
            config.wasm_memory64(false);
            let engine = Engine::new(&config)
                .map_err(|e| AbiError::Execution(e.to_string()))?;

            let wasm_bytes = std::fs::read(wasm_path)
                .map_err(|e| AbiError::Execution(format!("read wasm: {e}")))?;
            let module = Module::new(&engine, &wasm_bytes)
                .map_err(|e| AbiError::Execution(format!("compile wasm: {e}")))?;

            let wasi = WasiCtxBuilder::new()
                .inherit_stderr()
                .build_p1();

            // Pre-populate KV with env vars from the manifest [env] table.
            // Priority: actual process env > plugin.toml default value.
            let mut kv = std::collections::HashMap::new();
            for (var, default_val) in &ctx.env_defaults {
                let value = std::env::var(var).unwrap_or_else(|_| default_val.clone());
                kv.insert(format!("__env:{}", var), value);
            }

            let host_state = HostState {
                wasi,
                ctx: ctx.clone(),
                http_buf: vec![],
                kv,
                limiter: MemoryLimiter {
                    limit_bytes: (max_memory_mb as usize) * 1024 * 1024,
                },
            };

            let mut store = Store::new(&engine, host_state);
            // Wire the resource limiter so memory allocations are checked.
            store.limiter(|state| &mut state.limiter as &mut dyn wasmtime::ResourceLimiter);

            // ── Wire host imports ──────────────────────────────────────────
            let mut linker: Linker<HostState> = Linker::new(&engine);
            wasmtime_wasi::preview1::add_to_linker_async(&mut linker, |s| &mut s.wasi)
                .map_err(|e| AbiError::Execution(e.to_string()))?;

            // stui_log(level: i32, ptr: i32, len: i32)
            linker.func_wrap("stui", "stui_log", |mut caller: Caller<HostState>, level: i32, ptr: i32, len: i32| {
                if let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let data = mem.data(&caller);
                    if let Some(slice) = data.get(ptr as usize..(ptr + len) as usize) {
                        if let Ok(msg) = std::str::from_utf8(slice) {
                            match LogLevel::from_i32(level) {
                                LogLevel::Error => tracing::error!(plugin = "wasm", "{msg}"),
                                LogLevel::Warn  => tracing::warn!(plugin = "wasm", "{msg}"),
                                LogLevel::Debug => tracing::debug!(plugin = "wasm", "{msg}"),
                                LogLevel::Trace => tracing::trace!(plugin = "wasm", "{msg}"),
                                LogLevel::Info  => tracing::info!(plugin = "wasm", "{msg}"),
                            }
                        }
                    }
                }
            }).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_http_get(url_ptr, url_len) -> i64 ────────────────────
            // Makes a sandboxed GET; the host checks network_hosts from manifest.
            linker.func_wrap_async("stui", "stui_http_get",
                |mut caller: Caller<HostState>, (url_ptr, url_len): (i32, i32)| {
                    Box::new(async move {
                        let url = read_str_from_memory(&mut caller, url_ptr, url_len)?;
                        let allowed = {
                            let p = &caller.data().ctx.permissions;
                            let host = extract_host(&url);
                            p.allows_host(&host)
                        };
                        if !allowed {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked: host not in network_hosts");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }
                        let result = reqwest::get(&url).await;
                        let (status, body) = match result {
                            Ok(r)  => (r.status().as_u16(), r.text().await.unwrap_or_default()),
                            Err(e) => (0, e.to_string()),
                        };
                        write_response_to_memory(&mut caller, status, &body).await
                    })
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_http_post(payload_ptr, payload_len) -> i64 ───────────
            // Payload JSON: {"url":"…","body":"…","__stui_headers":{"X-Foo":"bar"}}
            // The host strips __stui_headers, applies them as real HTTP headers.
            linker.func_wrap_async("stui", "stui_http_post",
                |mut caller: Caller<HostState>, (ptr, len): (i32, i32)| {
                    Box::new(async move {
                        let raw = read_str_from_memory(&mut caller, ptr, len)?;
                        let mut val: serde_json::Value = serde_json::from_str(&raw)
                            .map_err(|e| wasmtime::Error::msg(e.to_string()))?;

                        let url = val["url"].as_str().unwrap_or("").to_string();
                        let body = val["body"].as_str().unwrap_or("").to_string();

                        // Permission check
                        let allowed = {
                            let p = &caller.data().ctx.permissions;
                            let host = extract_host(&url);
                            p.allows_host(&host)
                        };
                        if !allowed {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked POST: host not in network_hosts");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }

                        // Extract __stui_headers and remove from body
                        let headers_val = val.as_object_mut()
                            .and_then(|m| m.remove("__stui_headers"))
                            .unwrap_or_default();

                        let client = reqwest::Client::new();
                        let mut req = client.post(&url)
                            .header("Content-Type", "application/json")
                            .body(body);

                        // Apply plugin-declared headers
                        if let Some(h_map) = headers_val.as_object() {
                            for (k, v) in h_map {
                                if let Some(v_str) = v.as_str() {
                                    req = req.header(k.as_str(), v_str);
                                }
                            }
                        }

                        let (status, resp_body) = match req.send().await {
                            Ok(r)  => (r.status().as_u16(), r.text().await.unwrap_or_default()),
                            Err(e) => (0, e.to_string()),
                        };
                        write_response_to_memory(&mut caller, status, &resp_body).await
                    })
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_cache_get(key_ptr, key_len) -> i64 ───────────────────
            linker.func_wrap_async("stui", "stui_cache_get",
                |mut caller: Caller<HostState>, (key_ptr, key_len): (i32, i32)| {
                    Box::new(async move {
                        let key = read_str_from_memory(&mut caller, key_ptr, key_len)?;
                        let value = caller.data().kv.get(&key).cloned().unwrap_or_default();
                        if value.is_empty() {
                            return Ok::<i64, wasmtime::Error>(0);
                        }
                        write_bytes_to_memory(&mut caller, value.as_bytes()).await
                    })
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_cache_set(key_ptr, key_len, val_ptr, val_len) ────────
            linker.func_wrap("stui", "stui_cache_set",
                |mut caller: Caller<HostState>, kp: i32, kl: i32, vp: i32, vl: i32| {
                    let key = read_str_from_memory(&mut caller, kp, kl)
                        .unwrap_or_default()
                        .to_string();
                    let val = read_str_from_memory(&mut caller, vp, vl)
                        .unwrap_or_default()
                        .to_string();
                    if !key.is_empty() {
                        caller.data_mut().kv.insert(key, val);
                    }
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            let instance = linker.instantiate_async(&mut store, &module).await
                .map_err(|e| AbiError::Execution(format!("instantiate: {e}")))?;

            Ok(WasmInner {
                store: Mutex::new(store),
                instance,
                engine,
            })
        }

        pub fn abi_version(&self) -> i32 {
            let mut store = self.store.lock().unwrap();
            self.instance
                .get_typed_func::<(), i32>(&mut *store, "stui_abi_version")
                .ok()
                .and_then(|f| f.call(&mut *store, ()).ok())
                .unwrap_or(-1)
        }

        pub async fn call_export(&self, fn_name: &str, json_input: &str) -> Result<String, AbiError> {
            let input_bytes = json_input.as_bytes();
            let input_len = input_bytes.len() as i32;
            let mut store = self.store.lock().unwrap();

            // Allocate input buffer in plugin memory
            let alloc = self.instance
                .get_typed_func::<i32, i32>(&mut *store, "stui_alloc")
                .map_err(|_| AbiError::MissingExport("stui_alloc".into()))?;
            let input_ptr = alloc.call(&mut *store, input_len)
                .map_err(|e| AbiError::Execution(e.to_string()))?;

            // Write JSON into plugin memory
            let memory = self.instance
                .get_memory(&mut *store, "memory")
                .ok_or_else(|| AbiError::MissingExport("memory".into()))?;
            memory.write(&mut *store, input_ptr as usize, input_bytes)
                .map_err(|e| AbiError::Memory(e.to_string()))?;

            // Call the exported function — returns packed (ptr << 32) | len
            let func = self.instance
                .get_typed_func::<(i32, i32), i64>(&mut *store, fn_name)
                .map_err(|_| AbiError::MissingExport(fn_name.to_string()))?;
            let packed = func.call(&mut *store, (input_ptr, input_len))
                .map_err(|e| AbiError::Execution(e.to_string()))?;

            let out_ptr = ((packed >> 32) & 0xFFFFFFFF) as usize;
            let out_len = (packed & 0xFFFFFFFF) as usize;

            // Read result JSON from plugin memory
            let data = memory.data(&*store);
            let slice = data.get(out_ptr..out_ptr + out_len)
                .ok_or_else(|| AbiError::Memory("result ptr out of bounds".into()))?;
            let result = std::str::from_utf8(slice)
                .map_err(|e| AbiError::Memory(e.to_string()))?
                .to_string();

            // Free the input buffer
            let free = self.instance
                .get_typed_func::<(i32, i32), ()>(&mut *store, "stui_free")
                .map_err(|_| AbiError::MissingExport("stui_free".into()))?;
            let _ = free.call(&mut *store, (input_ptr, input_len));

            Ok(result)
        }
    }

    // ── Shared helper functions ───────────────────────────────────────────────

    /// Read a UTF-8 string from plugin linear memory.
    fn read_str_from_memory(
        caller: &mut Caller<HostState>,
        ptr: i32,
        len: i32,
    ) -> wasmtime::Result<String> {
        let mem = caller.get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| wasmtime::Error::msg("no memory export"))?;
        let data = mem.data(&*caller);
        let slice = data.get(ptr as usize..(ptr + len) as usize)
            .ok_or_else(|| wasmtime::Error::msg("ptr out of bounds"))?;
        std::str::from_utf8(slice)
            .map(|s| s.to_string())
            .map_err(|e| wasmtime::Error::msg(e.to_string()))
    }

    /// Write raw bytes into plugin memory (via stui_alloc), return packed i64.
    async fn write_bytes_to_memory(
        caller: &mut Caller<'_, HostState>,
        bytes: &[u8],
    ) -> wasmtime::Result<i64> {
        let len = bytes.len() as i32;
        let alloc = caller.get_export("stui_alloc")
            .and_then(|e| e.into_func())
            .ok_or_else(|| wasmtime::Error::msg("missing stui_alloc"))?;
        let mut results = vec![Val::I32(0)];
        alloc.call_async(caller.as_context_mut(), &[Val::I32(len)], &mut results).await?;
        let ptr = results[0].unwrap_i32();
        let mem = caller.get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| wasmtime::Error::msg("no memory export"))?;
        mem.write(caller.as_context_mut(), ptr as usize, bytes)
            .map_err(|e| wasmtime::Error::msg(e.to_string()))?;
        Ok(((ptr as i64) << 32) | (len as i64))
    }

    /// Serialise an HttpResponse and write it into plugin memory.
    async fn write_response_to_memory(
        caller: &mut Caller<'_, HostState>,
        status: u16,
        body: &str,
    ) -> wasmtime::Result<i64> {
        let json = serde_json::to_string(&HttpResponse {
            status,
            body: body.to_string(),
        })
        .unwrap_or_default();
        write_bytes_to_memory(caller, json.as_bytes()).await
    }

    /// Extract the bare hostname from a URL (no port, no scheme).
    fn extract_host(url: &str) -> String {
        // Strip scheme
        let after_scheme = url.find("://").map(|i| &url[i + 3..]).unwrap_or(url);
        // Strip path
        let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
        // Strip port
        host_port.split(':').next().unwrap_or(host_port).to_string()
    }
}

// ── Stub implementation — used when `wasm-host` feature is not enabled ────────

#[cfg(not(feature = "wasm-host"))]
mod stub_impl {
    use std::path::Path;

    use super::*;
    use crate::sandbox::SandboxCtx;

    pub struct WasmInner {
        pub plugin_name: String,
    }

    impl WasmInner {
        pub async fn load(
            _wasm_path:     &Path,
            plugin_name:    &str,
            _ctx:           &SandboxCtx,
            _max_memory_mb: u64,
        ) -> Result<Self, AbiError> {
            // Inform the caller clearly that the feature is not compiled in.
            // This is not a panic — it's a clean, expected state.
            tracing::warn!(
                plugin = %plugin_name,
                "WASM host not compiled in — rebuild with `--features wasm-host`"
            );
            Ok(WasmInner { plugin_name: plugin_name.to_string() })
        }

        pub fn abi_version(&self) -> i32 {
            // Return the host version so the version check passes in stub mode.
            // Real plugins need the real host.
            STUI_ABI_VERSION
        }

        pub async fn call_export(&self, fn_name: &str, _json: &str) -> Result<String, AbiError> {
            Err(AbiError::Execution(format!(
                "plugin '{}': WASM host not compiled in (fn: {}). \
                 Rebuild runtime with `--features wasm-host`.",
                self.plugin_name, fn_name
            )))
        }
    }
}

#[cfg(feature = "wasm-host")]
use inner_impl::WasmInner;

#[cfg(not(feature = "wasm-host"))]
use stub_impl::WasmInner;
