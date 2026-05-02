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

#![allow(dead_code)]

use std::path::Path;

use tracing::{debug, info};

use super::types::*;
use crate::sandbox::SandboxCtx;
use stui_plugin_sdk::{
    TrailersRequest, TrailersResponse,
    ReleaseInfoRequest, ReleaseInfoResponse,
    KeywordsRequest, KeywordsResponse,
    BoxOfficeRequest, BoxOfficeResponse,
    AlternativeTitlesRequest, AlternativeTitlesResponse,
    BulkEnrichRequest, BulkEnrichResponse,
};

// Plugin HTTP fetches go through reqwest with this ceiling. Set high
// enough to cover Jackett/Prowlarr Torznab fan-outs across many
// indexers — those legitimately take 20-30s end-to-end. Going lower
// (we used 15) silently failed real searches; going higher than the
// runtime's overall stream-resolution budget (~20s) would just be
// theatre because resolve::run_get_streams already gives up earlier.
const WASM_HTTP_TIMEOUT_SECS: u64 = 45;

/// User-Agent advertised on every plugin HTTP request. Several upstream
/// APIs (MusicBrainz, Discogs) hard-403 requests with reqwest's default
/// blank UA, citing "your application has not identified itself". This
/// default keeps every plugin honest by default; plugins that need a
/// per-service UA can still override via __stui_headers on stui_http_post.
const PLUGIN_USER_AGENT: &str =
    "stui-runtime/0.1.0 ( https://github.com/stui/stui )";

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
    /// Generic helper: serialize `req` to JSON, call the named export, and
    /// deserialize the `PluginResult<Resp>` returned by the plugin.
    async fn call_verb<Req, Resp>(&mut self, fn_name: &str, req: &Req) -> Result<Resp, AbiError>
    where
        Req: serde::Serialize,
        Resp: for<'de> serde::Deserialize<'de>,
    {
        let json = serde_json::to_string(req)?;
        let raw = self.inner.call_export(fn_name, &json).await?;
        let result: PluginResult<Resp> = serde_json::from_str(&raw)?;
        match result {
            PluginResult::Ok(r)  => Ok(r),
            PluginResult::Err(e) => Err(AbiError::Execution(format!("{}: {}", e.code, e.message))),
        }
    }

    /// Low-level entry point: call a named export with pre-serialized JSON and
    /// deserialize the `PluginResult<Resp>` envelope from the raw response.
    ///
    /// Used by `WasmSupervisor::call_verb` so it can pre-serialize the request
    /// before acquiring the instance lock, avoiding cross-await borrow issues.
    pub(super) async fn call_export_typed<Resp>(
        &mut self,
        fn_name: &str,
        json: &str,
    ) -> Result<Resp, AbiError>
    where
        Resp: for<'de> serde::Deserialize<'de>,
    {
        match self.call_export_envelope::<Resp>(fn_name, json).await? {
            PluginResult::Ok(r)  => Ok(r),
            PluginResult::Err(e) => Err(AbiError::Execution(format!("{}: {}", e.code, e.message))),
        }
    }

    /// Variant of [`call_export_typed`] that returns the full
    /// `PluginResult<Resp>` envelope without collapsing the error variant
    /// into an [`AbiError`].
    ///
    /// The supervisor needs this so it can distinguish a plumbing failure
    /// (trap, serde mismatch, missing export — `Err(AbiError)`) from a
    /// plugin-reported application error (`Ok(PluginResult::Err)`). Only the
    /// former should count toward the crash window and trigger a reload.
    pub(super) async fn call_export_envelope<Resp>(
        &mut self,
        fn_name: &str,
        json: &str,
    ) -> Result<PluginResult<Resp>, AbiError>
    where
        Resp: for<'de> serde::Deserialize<'de>,
    {
        let raw = self.inner.call_export(fn_name, json).await?;
        let result: PluginResult<Resp> = serde_json::from_str(&raw)?;
        Ok(result)
    }

    /// Call the plugin's `stui_init` export.
    ///
    /// Init has a different response shape than verb calls —
    /// [`InitResultEnvelope`] has no success payload and its error side
    /// carries a [`PluginInitError`] (not a [`PluginError`]). The result is
    /// surfaced as [`InitError`] so the caller can distinguish plumbing
    /// failures (network, memory) from plugin-reported init problems
    /// (missing config).
    pub async fn init(&mut self, req: &InitRequest) -> Result<(), InitError> {
        let json = serde_json::to_string(req).map_err(AbiError::Serde)?;
        let raw = self.inner.call_export("stui_init", &json).await
            .map_err(InitError::Abi)?;
        let env: InitResultEnvelope = serde_json::from_str(&raw)
            .map_err(|e| InitError::Abi(AbiError::Serde(e)))?;
        match Result::<(), PluginInitError>::from(env) {
            Ok(()) => Ok(()),
            Err(e) => Err(InitError::Plugin(e)),
        }
    }

    /// Low-level init caller — used by `WasmSupervisor::init` so the
    /// supervisor can pre-serialize the request before acquiring the
    /// instance lock, avoiding cross-await borrow issues. Callers should
    /// prefer [`WasmInstance::init`].
    pub(super) async fn call_init_with_json(
        &mut self,
        json: &str,
    ) -> Result<(), InitError> {
        let raw = self.inner.call_export("stui_init", json).await
            .map_err(InitError::Abi)?;
        let env: InitResultEnvelope = serde_json::from_str(&raw)
            .map_err(|e| InitError::Abi(AbiError::Serde(e)))?;
        match Result::<(), PluginInitError>::from(env) {
            Ok(()) => Ok(()),
            Err(e) => Err(InitError::Plugin(e)),
        }
    }

    /// Call the plugin's `stui_search` export.
    pub async fn search(&mut self, req: &SearchRequest) -> Result<SearchResponse, AbiError> {
        self.call_verb("stui_search", req).await
    }

    /// Call the plugin's `stui_resolve` export.
    pub async fn resolve(&mut self, req: &ResolveRequest) -> Result<ResolveResponse, AbiError> {
        self.call_verb("stui_resolve", req).await
    }

    /// Call the plugin's `stui_lookup` export.
    pub async fn lookup(&mut self, req: &LookupRequest) -> Result<LookupResponse, AbiError> {
        self.call_verb("stui_lookup", req).await
    }

    /// Call the plugin's `stui_enrich` export.
    pub async fn enrich(&mut self, req: &EnrichRequest) -> Result<EnrichResponse, AbiError> {
        self.call_verb("stui_enrich", req).await
    }

    /// Call the plugin's `stui_get_artwork` export.
    pub async fn get_artwork(&mut self, req: &ArtworkRequest) -> Result<ArtworkResponse, AbiError> {
        self.call_verb("stui_get_artwork", req).await
    }

    /// Call the plugin's `stui_get_credits` export.
    pub async fn get_credits(&mut self, req: &CreditsRequest) -> Result<CreditsResponse, AbiError> {
        self.call_verb("stui_get_credits", req).await
    }

    /// Call the plugin's `stui_related` export.
    pub async fn related(&mut self, req: &RelatedRequest) -> Result<RelatedResponse, AbiError> {
        self.call_verb("stui_related", req).await
    }

    /// Call the plugin's `stui_get_trailers` export.
    pub async fn get_trailers(&mut self, req: &TrailersRequest) -> Result<TrailersResponse, AbiError> {
        self.call_verb("stui_get_trailers", req).await
    }

    /// Call the plugin's `stui_get_release_info` export.
    pub async fn get_release_info(&mut self, req: &ReleaseInfoRequest) -> Result<ReleaseInfoResponse, AbiError> {
        self.call_verb("stui_get_release_info", req).await
    }

    /// Call the plugin's `stui_get_keywords` export.
    pub async fn get_keywords(&mut self, req: &KeywordsRequest) -> Result<KeywordsResponse, AbiError> {
        self.call_verb("stui_get_keywords", req).await
    }

    /// Call the plugin's `stui_get_box_office` export.
    pub async fn get_box_office(&mut self, req: &BoxOfficeRequest) -> Result<BoxOfficeResponse, AbiError> {
        self.call_verb("stui_get_box_office", req).await
    }

    /// Call the plugin's `stui_get_alternative_titles` export.
    pub async fn get_alternative_titles(&mut self, req: &AlternativeTitlesRequest) -> Result<AlternativeTitlesResponse, AbiError> {
        self.call_verb("stui_get_alternative_titles", req).await
    }

    /// Call the plugin's `stui_find_streams` export.
    pub async fn find_streams(&mut self, req: &FindStreamsRequest) -> Result<FindStreamsResponse, AbiError> {
        self.call_verb("stui_find_streams", req).await
    }

    /// Call the plugin's `stui_bulk_enrich` export.
    pub async fn bulk_enrich(&mut self, req: &BulkEnrichRequest) -> Result<BulkEnrichResponse, AbiError> {
        self.call_verb("stui_bulk_enrich", req).await
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
        let abi_version = inner.abi_version().await;
        if abi_version > STUI_ABI_VERSION {
            return Err(AbiError::VersionMismatch {
                plugin: abi_version,
                host: STUI_ABI_VERSION,
            });
        }
        // For v2 plugins, warn about any missing new-verb exports at load time
        // so operators notice gaps before the first user request triggers them.
        // v1 plugins are exempt — they pre-date these exports by design.
        if abi_version >= 2 {
            inner.probe_v2_exports(plugin_name).await;
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
    use tokio::sync::Mutex;
    use wasmtime::*;
    use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};
    use wasmtime_wasi::preview1::WasiP1Ctx;

    use super::*;
    use crate::auth;
    use crate::sandbox::SandboxCtx;
    use tracing::warn;

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
        /// Per-plugin KV cache — persists across calls within a session.
        /// Keys starting with "__env:" are pre-populated from plugin.toml [env].
        kv: std::collections::HashMap<String, String>,
        /// Memory limiter — enforces the max_memory_mb cap from SupervisorConfig.
        limiter: MemoryLimiter,
        /// Holds the auth callback receiver between stui_auth_allocate_port
        /// and stui_auth_open_and_wait calls. Taken (not cloned) before .await.
        pub auth_receiver: Option<crate::auth::OAuthReceiver>,
    }

    impl WasmInner {
        pub async fn load(
            wasm_path:     &Path,
            _plugin_name:   &str,
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

            let mut wasi_builder = WasiCtxBuilder::new();
            wasi_builder.inherit_stderr();
            // Scope filesystem access to directories declared in plugin.toml [permissions].
            // A plugin with an empty filesystem list gets no preopens — no FS access at all.
            for root in ctx.allowed_fs_roots() {
                if root.exists() {
                    let guest = root.to_string_lossy();
                    if let Err(e) = wasi_builder.preopened_dir(
                        &root,
                        guest.as_ref(),
                        DirPerms::all(),
                        FilePerms::all(),
                    ) {
                        warn!(plugin=%ctx.plugin_name, path=%root.display(), err=%e, "failed to preopen fs root — skipping");
                    }
                }
            }
            let wasi = wasi_builder.build_p1();

            // Pre-populate KV with env vars exposed as `__env:<VAR>` keys.
            // Priority (high → low):
            //   1. user TUI settings (runtime.toml `[plugins.<name>]`) — passed
            //      in via `ctx.user_env_overrides`
            //   2. `secrets.env` / process env (via `secrets::env_lookup`)
            //   3. manifest `[env]` default
            // The Settings UI is the canonical source of truth; secrets.env
            // remains as a fallback for headless / dev workflows.
            let mut kv = std::collections::HashMap::new();
            for (var, default_val) in &ctx.env_defaults {
                let value = crate::config::secrets::env_lookup(var)
                    .unwrap_or_else(|| default_val.clone());
                kv.insert(format!("__env:{}", var), value);
            }
            // Layer user overrides last so they win over both the manifest
            // default and secrets.env. Also seed `__env:<VAR>` entries that
            // weren't in the manifest's [env] table — a plugin that declares
            // its api key only via `[[config]] env_var = "X"` (no `[env] X = ""`)
            // still gets the user's value here.
            for (var, value) in &ctx.user_env_overrides {
                kv.insert(format!("__env:{}", var), value.clone());
            }

            let host_state = HostState {
                wasi,
                ctx: ctx.clone(),
                kv,
                limiter: MemoryLimiter {
                    limit_bytes: (max_memory_mb as usize) * 1024 * 1024,
                },
                auth_receiver: None,
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
                        // Coarse check: plugin must declare network access.
                        let net_check = caller.data().ctx.check(&crate::sandbox::Capability::Network);
                        if let Err(e) = net_check {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked GET: {e}");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }
                        // Fine-grained check: host must be in network_hosts allowlist (if set).
                        let allowed = {
                            let p = &caller.data().ctx.permissions;
                            let host = extract_host(&url);
                            p.allows_host(&host)
                        };
                        if !allowed {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked GET: host not in network_hosts");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(WASM_HTTP_TIMEOUT_SECS))
                            .user_agent(PLUGIN_USER_AGENT)
                            .build();
                        let client = match client {
                            Ok(c) => c,
                            Err(e) => {
                                warn!(plugin=%caller.data().ctx.plugin_name, err=%e, "client builder failed, using default");
                                reqwest::Client::builder()
                                    .timeout(std::time::Duration::from_secs(WASM_HTTP_TIMEOUT_SECS))
                                    .user_agent(PLUGIN_USER_AGENT)
                                    .build()
                                    .unwrap_or_else(|_| reqwest::Client::default())
                            }
                        };
                        let result = client.get(&url).send().await;
                        let (status, body) = match result {
                            Ok(r)  => (r.status().as_u16(), r.text().await.unwrap_or_default()),
                            Err(e) => (0, e.to_string()),
                        };
                        write_response_to_memory(&mut caller, status, &body).await
                    })
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_http_get_with_headers(payload_ptr, payload_len) -> i64 ─
            // Payload JSON: {"url":"…","__stui_headers":{"Cookie":"…","X-Api-Key":"…"}}
            // Same shape as `stui_http_post` minus the body. Needed by
            // cookie-authenticated trackers (RuTracker, Zamunda, BT.etree)
            // where the bare `stui_http_get(url)` can't carry the
            // `Cookie:` / `User-Agent:` overrides those backends expect.
            linker.func_wrap_async("stui", "stui_http_get_with_headers",
                |mut caller: Caller<HostState>, (ptr, len): (i32, i32)| {
                    Box::new(async move {
                        let raw = read_str_from_memory(&mut caller, ptr, len)?;
                        let mut val: serde_json::Value = serde_json::from_str(&raw)
                            .map_err(|e| wasmtime::Error::msg(e.to_string()))?;

                        let url = val["url"].as_str().unwrap_or("").to_string();

                        let net_check = caller.data().ctx.check(&crate::sandbox::Capability::Network);
                        if let Err(e) = net_check {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked GET+headers: {e}");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }
                        let allowed = {
                            let p = &caller.data().ctx.permissions;
                            let host = extract_host(&url);
                            p.allows_host(&host)
                        };
                        if !allowed {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked GET+headers: host not in network_hosts");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }

                        let headers_val = val.as_object_mut()
                            .and_then(|m| m.remove("__stui_headers"))
                            .unwrap_or_default();

                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(WASM_HTTP_TIMEOUT_SECS))
                            .user_agent(PLUGIN_USER_AGENT)
                            .build()
                            .unwrap_or_else(|_| reqwest::Client::new());
                        let mut req = client.get(&url);

                        if let Some(h_map) = headers_val.as_object() {
                            for (k, v) in h_map {
                                if let Some(v_str) = v.as_str() {
                                    req = req.header(k.as_str(), v_str);
                                }
                            }
                        }

                        let (status, body) = match req.send().await {
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

                        // Coarse check: plugin must declare network access.
                        let net_check = caller.data().ctx.check(&crate::sandbox::Capability::Network);
                        if let Err(e) = net_check {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked POST: {e}");
                            return write_response_to_memory(&mut caller, 503, "blocked by sandbox").await;
                        }
                        // Fine-grained check: host must be in network_hosts allowlist (if set).
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

                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(WASM_HTTP_TIMEOUT_SECS))
                            .user_agent(PLUGIN_USER_AGENT)
                            .build()
                            .unwrap_or_else(|_| reqwest::Client::new());
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

            // ── stui_http_request(payload_ptr, payload_len) -> i64 ────────
            // General-purpose HTTP primitive — the only one that surfaces
            // response headers back to the WASM plugin. Required for
            // anything that needs Set-Cookie (session auth: RuTracker,
            // Zamunda), Location (redirect chains), Etag (cached APIs).
            //
            // Payload JSON: {
            //   "method":  "GET" | "POST" | "PUT" | "DELETE" | …,
            //   "url":     "https://…",
            //   "body":    "…" (optional; ignored for GET/HEAD),
            //   "headers": { "Content-Type": "…", "Cookie": "…" }
            // }
            // Response JSON: {
            //   "status":  200,
            //   "headers": [["set-cookie", "bb_session=abc"], …],
            //   "body":    "…"
            // }
            //
            // The convenience wrappers (http_get / http_post_json /
            // http_get_with_headers) stay as-is — they're the right shape
            // for plugins that don't care about response headers and want
            // a thinner SDK surface.
            linker.func_wrap_async("stui", "stui_http_request",
                |mut caller: Caller<HostState>, (ptr, len): (i32, i32)| {
                    Box::new(async move {
                        let raw = read_str_from_memory(&mut caller, ptr, len)?;
                        let mut val: serde_json::Value = serde_json::from_str(&raw)
                            .map_err(|e| wasmtime::Error::msg(e.to_string()))?;

                        let method = val["method"].as_str().unwrap_or("GET").to_uppercase();
                        let url    = val["url"].as_str().unwrap_or("").to_string();
                        let body   = val["body"].as_str().map(|s| s.to_string());

                        // Sandbox checks — same shape as stui_http_post.
                        let net_check = caller.data().ctx.check(&crate::sandbox::Capability::Network);
                        if let Err(e) = net_check {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked {method}: {e}");
                            return write_full_response_to_memory(&mut caller, 503, &[], "blocked by sandbox").await;
                        }
                        let allowed = {
                            let p = &caller.data().ctx.permissions;
                            let host = extract_host(&url);
                            p.allows_host(&host)
                        };
                        if !allowed {
                            warn!(plugin=%caller.data().ctx.plugin_name, url=%url, "blocked {method}: host not in network_hosts");
                            return write_full_response_to_memory(&mut caller, 503, &[], "blocked by sandbox").await;
                        }

                        // Headers map — applied verbatim. No default
                        // Content-Type unlike stui_http_post; callers that
                        // send form bodies set their own.
                        let headers_val = val.as_object_mut()
                            .and_then(|m| m.remove("headers"))
                            .unwrap_or_default();

                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(WASM_HTTP_TIMEOUT_SECS))
                            .user_agent(PLUGIN_USER_AGENT)
                            // Don't auto-follow redirects: a session-auth
                            // plugin needs to see the 302 → /login redirect
                            // to detect expiry, and a login-flow plugin
                            // needs to capture Set-Cookie from the redirect
                            // response itself.
                            .redirect(reqwest::redirect::Policy::none())
                            .build()
                            .unwrap_or_else(|_| reqwest::Client::new());

                        let mut req = match method.as_str() {
                            "GET"     => client.get(&url),
                            "POST"    => client.post(&url),
                            "PUT"     => client.put(&url),
                            "DELETE"  => client.delete(&url),
                            "PATCH"   => client.patch(&url),
                            "HEAD"    => client.head(&url),
                            _         => {
                                return write_full_response_to_memory(
                                    &mut caller, 0, &[],
                                    &format!("unsupported HTTP method: {method}"),
                                ).await;
                            }
                        };
                        if let Some(b) = body {
                            req = req.body(b);
                        }
                        if let Some(h_map) = headers_val.as_object() {
                            for (k, v) in h_map {
                                if let Some(v_str) = v.as_str() {
                                    req = req.header(k.as_str(), v_str);
                                }
                            }
                        }

                        match req.send().await {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                let headers: Vec<(String, String)> = r.headers()
                                    .iter()
                                    .map(|(k, v)| {
                                        (
                                            k.as_str().to_string(),
                                            v.to_str().unwrap_or("").to_string(),
                                        )
                                    })
                                    .collect();
                                let body = r.text().await.unwrap_or_default();
                                write_full_response_to_memory(&mut caller, status, &headers, &body).await
                            }
                            Err(e) => {
                                write_full_response_to_memory(&mut caller, 0, &[], &e.to_string()).await
                            }
                        }
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

            // ── stui_now_unix() -> i64 ────────────────────────────────────────────────
            linker.func_wrap("stui", "stui_now_unix",
                |_caller: Caller<HostState>| -> i64 {
                    use std::time::{SystemTime, UNIX_EPOCH};
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0)
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_auth_allocate_port() -> i32 ──────────────────────────────────────
            // Starts the callback server. Returns port. Replaces any existing receiver.
            linker.func_wrap_async("stui", "stui_auth_allocate_port",
                |mut caller: Caller<HostState>, (): ()| {
                    Box::new(async move {
                        match auth::allocate_port().await {
                            Ok((port, rx)) => {
                                caller.data_mut().auth_receiver = Some(rx);
                                Ok::<i32, wasmtime::Error>(port as i32)
                            }
                            Err(e) => {
                                tracing::warn!(plugin=%caller.data().ctx.plugin_name, err=%e, "auth_allocate_port failed");
                                Ok(-1i32)
                            }
                        }
                    })
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_auth_open_and_wait(url_ptr, url_len, timeout_ms) -> i64 ──────────
            // Opens browser and suspends until callback or timeout.
            // Returns packed (ptr<<32)|len → JSON in plugin memory.
            // Receiver is taken BEFORE .await so no Store borrow crosses the await.
            linker.func_wrap_async("stui", "stui_auth_open_and_wait",
                |mut caller: Caller<HostState>, (url_ptr, url_len, timeout_ms_raw): (i32, i32, i32)| {
                    Box::new(async move {
                        // Take receiver and URL before any await (no store borrow across await)
                        let receiver = caller.data_mut().auth_receiver.take();
                        let url_result = read_str_from_memory(&mut caller, url_ptr, url_len);

                        let url = match url_result {
                            Ok(u) => u,
                            Err(e) => {
                                let json = format!(r#"{{"error":"browser_open_failed","message":"bad url: {e}"}}"#);
                                return write_bytes_to_memory(&mut caller, json.as_bytes()).await;
                            }
                        };

                        let receiver = match receiver {
                            Some(r) => r,
                            None => {
                                let json = r#"{"error":"no_port_allocated"}"#;
                                return write_bytes_to_memory(&mut caller, json.as_bytes()).await;
                            }
                        };

                        // Clamp timeout to [1000, 300000] ms; clamp before cast to
                        // prevent a negative i32 wrapping to a huge u64 value.
                        let timeout_ms = (timeout_ms_raw.clamp(1_000, 300_000)) as u64;
                        let timeout = std::time::Duration::from_millis(timeout_ms);

                        // No store borrows held across this await
                        let result = auth::open_and_wait(&url, receiver, timeout).await;

                        let result_json: String = match result {
                            Ok(cb) => serde_json::json!({"code": cb.code, "state": cb.state}).to_string(),
                            Err(auth::AuthError::TimedOut) => r#"{"error":"timed_out"}"#.to_string(),
                            Err(auth::AuthError::Denied { message }) =>
                                serde_json::json!({"error":"denied","message":message}).to_string(),
                            Err(auth::AuthError::BrowserOpenFailed(m)) =>
                                serde_json::json!({"error":"browser_open_failed","message":m}).to_string(),
                            Err(auth::AuthError::ReceiverDropped) =>
                                r#"{"error":"timed_out"}"#.to_string(),
                        };

                        write_bytes_to_memory(&mut caller, result_json.as_bytes()).await
                    })
                }
            ).map_err(|e| AbiError::Execution(e.to_string()))?;

            // ── stui_exec(cmd_json_ptr, cmd_json_len, timeout_ms) -> i64 ───────────
            // cmd_json: {"cmd":"yt-dlp","args":["--flat-playlist","-j"],"timeout_ms":30000}
            // Returns: {"status":0,"stdout":"...","stderr":"..."}
            linker.func_wrap_async("stui", "stui_exec",
                |mut caller: Caller<HostState>, (ptr, len, timeout_ms_raw): (i32, i32, i32)| {
                    Box::new(async move {
                        let json_str = match read_str_from_memory(&mut caller, ptr, len) {
                            Ok(s) => s,
                            Err(e) => {
                                let resp = format!(r#"{{"status":-1,"stdout":"","stderr":"read error: {e}"}}"#);
                                return write_bytes_to_memory(&mut caller, resp.as_bytes()).await;
                            }
                        };

                        let val: serde_json::Value = match serde_json::from_str(&json_str) {
                            Ok(v) => v,
                            Err(e) => {
                                let resp = format!(r#"{{"status":-1,"stdout":"","stderr":"parse error: {e}"}}"#);
                                return write_bytes_to_memory(&mut caller, resp.as_bytes()).await;
                            }
                        };

                        let cmd = match val["cmd"].as_str() {
                            Some(s) if !s.is_empty() => s.to_string(),
                            _ => {
                                let resp = r#"{"status":-1,"stdout":"","stderr":"missing cmd"}"#.to_string();
                                return write_bytes_to_memory(&mut caller, resp.as_bytes()).await;
                            }
                        };

                        let args: Vec<String> = val["args"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();

                        // Clamp timeout to [1000, 60000] ms
                        let timeout_ms = (timeout_ms_raw.clamp(1_000, 60_000)) as u64;
                        let timeout = std::time::Duration::from_millis(timeout_ms);

                        let (tx, rx) = std::sync::mpsc::channel();

                        std::thread::spawn(move || {
                            let result = std::process::Command::new(cmd)
                                .args(&args)
                                .output();
                            let _ = tx.send(result);
                        });

                        let output = match rx.recv_timeout(timeout) {
                            Ok(Ok(out)) => out,
                            Ok(Err(e)) => {
                                let resp = format!(r#"{{"status":-1,"stdout":"","stderr":"exec error: {e}"}}"#);
                                return write_bytes_to_memory(&mut caller, resp.as_bytes()).await;
                            }
                            Err(_) => {
                                let resp = format!(r#"{{"status":-1,"stdout":"","stderr":"timeout after {}ms"}}"#, timeout_ms);
                                return write_bytes_to_memory(&mut caller, resp.as_bytes()).await;
                            }
                        };

                        let status = output.status.code().unwrap_or(-1);
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                        let resp = serde_json::json!({
                            "status": status,
                            "stdout": stdout,
                            "stderr": stderr
                        }).to_string();

                        write_bytes_to_memory(&mut caller, resp.as_bytes()).await
                    })
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

        /// Invoke the plugin's `stui_abi_version` export. Async because the
        /// store runs in async mode (`config.async_support(true)`), which
        /// makes sync `call()` panic at the wasmtime layer.
        pub async fn abi_version(&self) -> i32 {
            let mut store = self.store.lock().await;
            match self.instance.get_typed_func::<(), i32>(&mut *store, "stui_abi_version") {
                Ok(f) => f.call_async(&mut *store, ()).await.unwrap_or(-1),
                Err(_) => -1,
            }
        }

        /// Warn about any missing v2-specific exports. v1 plugins are expected
        /// to lack these; calling this only when `abi_version >= 2` avoids
        /// false-positive warnings for v1 plugins that load under a v2 host.
        ///
        /// A missing export at load time produces a WARN (not an error) because
        /// the first-call path in `call_export` will surface `MissingExport`
        /// as a clean `AbiError`, giving the engine a NOT_IMPLEMENTED path.
        pub async fn probe_v2_exports(&self, plugin_name: &str) {
            let mut store = self.store.lock().await;
            const V2_EXPORTS: &[&str] = &[
                "stui_get_trailers",
                "stui_get_release_info",
                "stui_get_keywords",
                "stui_get_box_office",
                "stui_get_alternative_titles",
            ];
            for &export_name in V2_EXPORTS {
                if self.instance.get_func(&mut *store, export_name).is_none() {
                    warn!(
                        plugin = %plugin_name,
                        export = export_name,
                        "v2 plugin missing new-verb export — verb will return NOT_IMPLEMENTED",
                    );
                }
            }
        }

        pub async fn call_export(&self, fn_name: &str, json_input: &str) -> Result<String, AbiError> {
            let input_bytes = json_input.as_bytes();
            let input_len = input_bytes.len() as i32;
            let mut store = self.store.lock().await;          // async lock

            let alloc = self.instance
                .get_typed_func::<i32, i32>(&mut *store, "stui_alloc")
                .map_err(|_| AbiError::MissingExport("stui_alloc".into()))?;
            let input_ptr = alloc.call_async(&mut *store, input_len).await
                .map_err(|e| AbiError::Execution(e.to_string()))?;

            let memory = self.instance
                .get_memory(&mut *store, "memory")
                .ok_or_else(|| AbiError::MissingExport("memory".into()))?;
            memory.write(&mut *store, input_ptr as usize, input_bytes)
                .map_err(|e| AbiError::Memory(e.to_string()))?;

            let func = self.instance
                .get_typed_func::<(i32, i32), i64>(&mut *store, fn_name)
                .map_err(|_| AbiError::MissingExport(fn_name.to_string()))?;
            let packed = func.call_async(&mut *store, (input_ptr, input_len)).await
                .map_err(|e| AbiError::Execution(e.to_string()))?;

            let out_ptr = ((packed >> 32) & 0xFFFFFFFF) as usize;
            let out_len = (packed & 0xFFFFFFFF) as usize;

            let data = memory.data(&*store);
            let slice = data.get(out_ptr..out_ptr + out_len)
                .ok_or_else(|| AbiError::Memory("result ptr out of bounds".into()))?;
            let result = std::str::from_utf8(slice)
                .map_err(|e| AbiError::Memory(e.to_string()))?
                .to_string();

            let free = self.instance
                .get_typed_func::<(i32, i32), ()>(&mut *store, "stui_free")
                .map_err(|_| AbiError::MissingExport("stui_free".into()))?;
            let _ = free.call_async(&mut *store, (input_ptr, input_len)).await;

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

    /// Serialise an HTTP response with response headers and write it into
    /// plugin memory. Pair to `stui_http_request` — the only host import
    /// that surfaces headers back to WASM. JSON shape matches
    /// `stui_plugin_sdk::HttpFullResponse`.
    async fn write_full_response_to_memory(
        caller: &mut Caller<'_, HostState>,
        status: u16,
        headers: &[(String, String)],
        body: &str,
    ) -> wasmtime::Result<i64> {
        let json = serde_json::to_string(&serde_json::json!({
            "status":  status,
            "headers": headers,
            "body":    body,
        }))
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

    #[cfg(test)]
    mod tests {
        // Run with: cargo test --features wasm-host abi::host::inner_impl::tests

        use super::*;

        fn make_test_sandbox_ctx() -> crate::sandbox::SandboxCtx {
            crate::sandbox::SandboxCtx {
                plugin_id: "test".to_string(),
                plugin_name: "test-plugin".to_string(),
                permissions: crate::plugin::Permissions::default(),
                mode: crate::plugin::ExecutionMode::Wasm,
                cache_dir: std::path::PathBuf::from("/tmp"),
                data_dir: std::path::PathBuf::from("/tmp"),
                env_defaults: std::collections::HashMap::new(),
                user_env_overrides: std::collections::HashMap::new(),
            }
        }

        #[tokio::test]
        async fn test_auth_receiver_stored_in_host_state() {
            let (port, rx) = crate::auth::allocate_port().await.unwrap();
            let mut state = HostState {
                wasi: wasmtime_wasi::WasiCtxBuilder::new().build_p1(),
                ctx: make_test_sandbox_ctx(),
                kv: std::collections::HashMap::new(),
                limiter: MemoryLimiter { limit_bytes: 128 * 1024 * 1024 },
                auth_receiver: Some(rx),
            };
            assert!(state.auth_receiver.is_some());
            let taken = state.auth_receiver.take();
            assert!(taken.is_some());
            assert!(state.auth_receiver.is_none(), "take() must leave None");
            let _ = (port, taken);
        }
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

        pub async fn abi_version(&self) -> i32 {
            // Return the host version so the version check passes in stub mode.
            // Real plugins need the real host.
            STUI_ABI_VERSION
        }

        /// No-op in stub mode — no wasm instance to inspect.
        pub async fn probe_v2_exports(&self, _plugin_name: &str) {}

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

// ── ABI version gate tests ────────────────────────────────────────────────────

#[cfg(test)]
mod abi_version_tests {
    use super::*;

    fn abi_check_accepts(plugin_version: i32) -> bool {
        plugin_version <= stui_plugin_sdk::STUI_ABI_VERSION
    }

    #[test]
    fn abi_check_accepts_v1_under_v2_runtime() {
        // STUI_ABI_VERSION == 2; v1 plugins should load.
        assert!(abi_check_accepts(1));
        assert!(abi_check_accepts(2));
        assert!(!abi_check_accepts(3));
    }
}
