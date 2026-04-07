# Plugin OAuth Browser Auth Design

**Goal:** Add browser-based OAuth support to both WASM and RPC plugin types, using a bidirectional action protocol for RPC and new host imports for WASM, with a SoundCloud WASM plugin as the reference implementation.

**Architecture:** The runtime provides two primitives — a local callback HTTP server (random port) and a cross-platform browser opener. Plugins request a port, construct the full OAuth URL with `redirect_uri=http://localhost:{port}/callback`, then call a single blocking `open_and_wait` function that opens the browser and returns the auth code when the user completes login. Token exchange and storage are the plugin's responsibility, using the existing HTTP host functions and KV cache.

**Tech Stack:** Rust (runtime + WASM SDK), tokio (async, raw `TcpListener` for callback server — no additional HTTP framework dep), wasmtime (WASM host imports), JSON action messages (RPC bidirectional protocol), `xdg-open`/`open`/`start` (browser opener).

---

## 1. Auth Primitives (`runtime/src/auth/`)

Three new source files plus a `pub mod auth;` declaration in `runtime/src/lib.rs`.

### Types (defined in `callback_server.rs`, re-exported by `mod.rs`)

```rust
// Runtime-internal type. The SDK defines its own sdk::OAuthCallback (§4).
pub struct OAuthCallback {
    pub code: Option<String>,   // present on success
    pub state: Option<String>,  // echoed from request; CSRF validation is caller's responsibility
    pub error: Option<String>,  // present on OAuth error (e.g. "access_denied")
}
pub type OAuthReceiver = oneshot::Receiver<OAuthCallback>;
```

### `auth/callback_server.rs`

Binds to port 0 (OS-assigned — guaranteed to succeed) via `tokio::net::TcpListener::bind("127.0.0.1:0")`. The actual port is read via `local_addr().port()`. The server is fully listening from the moment `allocate_port()` returns.

Implemented with a raw `TcpListener` loop — no additional HTTP crate. The server task holds both the one-shot sender and the TCP listener. It runs until either a callback arrives or the sender detects that the receiver was dropped (via `tx.closed().await`), whichever comes first. This ensures the server always shuts down when `open_and_wait` times out or the plugin drops the receiver:

```rust
tokio::spawn(async move {
    loop {
        tokio::select! {
            result = listener.accept() => {
                // Minimal HTTP/1.x parse (no external crate):
                // 1. Read bytes until "\r\n\r\n" to get the request line.
                // 2. Extract query string: "GET /callback?<qs> HTTP/1.1"
                // 3. Split on '&', then split each pair on '='.
                // 4. Percent-decode each value with a simple helper
                //    (replace '+' → ' ', decode %XX sequences).
                // 5. Populate OAuthCallback { code, state, error }.
                // 6. Write "HTTP/1.1 200 OK\r\nContent-Length:...\r\n\r\nYou may close this tab."
                let _ = tx.send(callback);
                return;
            }
            _ = tx.closed() => {
                // Receiver was dropped (timeout or plugin exit) — shut down cleanly
                return;
            }
        }
    }
});
```

```rust
/// Binds a local HTTP server on a random OS-assigned port (already listening on return).
/// Returns (port, receiver). The server shuts down after one callback or when the
/// receiver is dropped.
pub async fn allocate_port() -> Result<(u16, OAuthReceiver)>
```

### `auth/browser.rs`

```rust
/// Opens `url` in the system default browser as a detached process.
/// Linux: xdg-open, macOS: open, Windows: cmd /c start.
/// Returns after the launcher process is spawned (non-blocking).
pub fn open_url(url: &str) -> Result<()>
```

### `auth/mod.rs`

Re-exports types and delegates `allocate_port` to `callback_server::allocate_port`.

```rust
pub use callback_server::{OAuthCallback, OAuthReceiver};
pub use callback_server::allocate_port;  // direct re-export; no wrapper needed

/// Opens `url` in the browser then awaits the OAuth callback.
/// Timeout begins after `open_url` returns (after launcher is spawned).
/// When the timeout fires, the OAuthReceiver is dropped, which causes the
/// server task to detect `tx.closed()` and shut down.
pub async fn open_and_wait(
    url: &str,
    receiver: OAuthReceiver,
    timeout: Duration,
) -> Result<OAuthCallback, AuthError>

#[derive(Debug)]
pub enum AuthError {
    TimedOut,
    Denied { message: String },
    BrowserOpenFailed(String),
    ReceiverDropped,  // server shut down before callback arrived (e.g. server task panicked)
}
```

Timeout is always explicit — the SDK default of 120 s is applied in §4 SDK wrappers, not here.

---

## 2. WASM Host Imports (`runtime/src/abi/host.rs`)

### Store mutex refactor (required for long-running async imports)

The existing `store: Mutex<Store<HostState>>` in `WasmInner` uses `std::sync::Mutex`, which blocks a thread when held across `.await`. For a 120-second auth wait this would freeze a tokio worker thread, which is unacceptable.

Change:
```rust
// Before:
store: std::sync::Mutex<Store<HostState>>

// After:
store: tokio::sync::Mutex<Store<HostState>>
```

Change `call_export` accordingly:
```rust
// Before: self.store.lock().unwrap()   +   func.call(...)
// After:  self.store.lock().await      +   func.call_async(...).await
```

`abi_version()` (sync context) uses `tokio::sync::Mutex::try_lock()` — this method is callable from sync code and returns `Result<MutexGuard, TryLockError>`. If the lock is contended (store held by a long-running auth), return `0` as the fallback version value. This refactor is behaviour-preserving for all existing imports.

### ABI memory convention

Follows the existing convention used by `stui_http_get`/`stui_cache_get`: the host calls the plugin's `stui_alloc(len)` export to get a pointer, writes bytes into linear memory at that pointer, and returns the packed `(ptr << 32) | len`. The plugin does **not** call `stui_free` after reading — this matches the established SDK pattern (see `http_get` and `cache_get` in `sdk/src/lib.rs`).

### Feature gating

All new imports are inside the `#[cfg(feature = "wasm-host")]` block of `inner_impl`, identical to existing imports. The `#[cfg(not(feature = "wasm-host"))]` stub `WasmInner::call_export` already returns `Err(AbiError::Execution(...))` for every call — no additional stub entries needed.

### New host import signatures (registered as `"stui"` module, consistent with existing)

```c
// Starts callback server. Returns port as i32.
// If a receiver is already held, drops it and allocates a new port (safe retry).
// Auth imports are always registered unconditionally — no capability check at load time.
stui_auth_allocate_port() -> i32

// Opens browser and suspends until callback or timeout.
// timeout_ms: i32, clamped to [1000, 300000].
// Must be called after stui_auth_allocate_port; if called without a prior allocation
// returns error JSON immediately without opening a browser.
// Returns packed (ptr<<32)|len → JSON in plugin memory:
//   success: {"code":"...","state":"..."}   (state may be null)
//   errors:  {"error":"timed_out"}
//          | {"error":"denied","message":"..."}
//          | {"error":"no_port_allocated"}
//          | {"error":"browser_open_failed","message":"..."}
stui_auth_open_and_wait(url_ptr: i32, url_len: i32, timeout_ms: i32) -> i64
```

### SDK `extern "C"` declarations (added to the top-level `extern "C"` block in `sdk/src/lib.rs`)

The SDK has two extern patterns: a top-level block at the top of the file containing `stui_log`, `stui_http_get`, `stui_cache_get`, etc., and an inline block inside `http_post_json`. The new auth declarations go in the **top-level block** (matching the majority pattern), not inline:

```rust
#[cfg(target_arch = "wasm32")]
extern "C" {
    // existing: stui_log, stui_http_get, stui_cache_get, stui_cache_set ...
    pub fn stui_auth_allocate_port() -> i32;
    pub fn stui_auth_open_and_wait(url_ptr: *const u8, url_len: i32, timeout_ms: i32) -> i64;
}
```

### Auth state location

Stored in `HostState` (inside the wasmtime `Store`) — not on `WasmInstance`:

```rust
// Added to HostState in inner_impl:
pub auth_receiver: Option<OAuthReceiver>
// No additional locking needed — HostState is accessed only through the Store,
// and WASM plugins are single-threaded. The receiver is taken before .await
// so no Store borrow is held across the async wait.
```

### Avoiding store borrow across await

`stui_auth_open_and_wait` uses `linker.func_wrap_async`. The receiver is taken from `HostState` before any `.await`, using the same pattern as `stui_http_get`:

```rust
// In func_wrap_async closure:
let receiver = caller.data_mut().auth_receiver.take();
let url = read_str_from_memory(&mut caller, url_ptr, url_len)?;
// All borrows of caller/store state are complete before the await:
let result = auth::open_and_wait(&url, receiver?, Duration::from_millis(timeout_ms as u64)).await;
// Serialize result to JSON before re-borrowing the store:
let result_json: String = match result {
    Ok(cb) => serde_json::json!({"code": cb.code, "state": cb.state}).to_string(),
    Err(AuthError::TimedOut) => r#"{"error":"timed_out"}"#.to_string(),
    Err(AuthError::Denied { message }) => serde_json::json!({"error":"denied","message":message}).to_string(),
    Err(AuthError::BrowserOpenFailed(m)) => serde_json::json!({"error":"browser_open_failed","message":m}).to_string(),
    Err(AuthError::ReceiverDropped) => r#"{"error":"timed_out"}"#.to_string(),
};
// Re-borrow after await to write result into plugin memory:
write_bytes_to_memory(&mut caller, result_json.as_bytes()).await
```

### `WasmInner` thread-safety after mutex change

wasmtime's `Instance` type is `Send + Sync` when the engine is configured with `async_support(true)` (already set in the existing codebase). Holding `instance` and `store: tokio::sync::Mutex<Store<HostState>>` as separate fields is the correct pattern — callers must lock the store before calling into the instance, which the existing `call_export` enforces. The mutex change does not introduce new safety concerns here.

### WASM concurrency (single-threaded, no concurrent auth)

WASM plugins are single-threaded; concurrent calls cannot occur within a single instance. `stui_auth_allocate_port` drops any existing receiver and stores the new one in `HostState.auth_receiver`. `stui_auth_open_and_wait` takes via `Option::take()`; if `None`, returns `{"error":"no_port_allocated"}` immediately.

### WASM error strings

| `result["error"]` | Meaning |
|---|---|
| `"timed_out"` | User did not complete auth within `timeout_ms` |
| `"denied"` | OAuth provider returned `?error=`; see `result["message"]` |
| `"no_port_allocated"` | `open_and_wait` called without prior `allocate_port` |
| `"browser_open_failed"` | OS launcher failed; see `result["message"]` |

---

## 3. RPC Bidirectional Protocol

### `plugin_rpc/protocol.rs` additions

```rust
#[derive(Deserialize)]
pub struct ActionRequest {
    pub action: String,
    pub id: String,
    pub params: Option<Value>,
}
// Normative params schemas:
//   "auth_allocate_port": params absent or {}
//   "auth_open_and_wait": {
//       "url": String (required),
//       "timeout_ms": u32 (optional, default 120000, clamped [1000, 300000])
//   }

#[derive(Serialize)]
pub struct ActionResponse {
    pub action_id: String,  // echoes the plugin's ActionRequest.id
    pub result: Option<Value>,
    pub error: Option<String>,
}
// Result shapes:
//   "auth_allocate_port" success: {"port": u16}
//   "auth_open_and_wait" success: {"code": String, "state": String|null}
```

**Plugin author note:** `ActionResponse` is a distinct message type from `RpcResponse`. Plugin authors must distinguish them by field: responses with `action_id` are `ActionResponse`; responses with `id` are `RpcResponse`. The `error` field in `ActionResponse` is a flat string (not a structured `RpcError` object as in `RpcResponse`). The RPC path flattens errors into strings (e.g. `"denied: access_denied"`) while WASM returns structured JSON (`{"error":"denied","message":"..."}`) — this is a deliberate tradeoff to keep `ActionResponse` schema simple; RPC plugins parsing the `error` string should split on `": "` to separate code from message.

**Demultiplexing:** `ActionRequest` parse is attempted **first** (positive discriminant on `"action"` field). Only lines that fail `ActionRequest` parsing proceed to `RpcResponse` dispatch. This order is required — `RpcResponse` only requires an `"id"` field and would silently accept an `ActionRequest` if tried first.

### `plugin_rpc/process.rs` — stdin write refactor

The existing direct `Arc<Mutex<ChildStdin>>` write path is replaced with an unbounded channel. An unbounded channel is used because responses must never be dropped, and the number of in-flight plugin calls is bounded by the supervisor's request timeout:

```rust
let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
tokio::spawn(async move {
    while let Some(line) = stdin_rx.recv().await {
        let _ = stdin.write_all(line.as_bytes()).await;
        // Flush after each write — matches the existing direct-write path which
        // also calls flush(). tokio's ChildStdin is not guaranteed to be unbuffered.
        let _ = stdin.flush().await;
    }
});
```

The existing `call()` method sends through `stdin_tx` instead of locking `ChildStdin`. Behaviour-preserving.

### `AuthPhase` enum

Using an enum eliminates the invalid `{receiver: Some, in_progress: true}` state:

```rust
enum AuthPhase {
    Idle,
    Allocated(OAuthReceiver),  // port allocated, waiting for open_and_wait call
    InProgress,                // open_and_wait is running
}

type SharedAuthPhase = Arc<Mutex<AuthPhase>>;
// Initialised in PluginProcess::spawn() alongside `pending`:
//   let auth_phase: SharedAuthPhase = Arc::new(Mutex::new(AuthPhase::Idle));
// Then cloned into the read loop closure (same pattern as `pending_rx`):
//   let auth_phase_loop = auth_phase.clone();
// And stored as a field on PluginProcess alongside the `pending` response map.
```

### Read loop

```rust
// ActionRequest check first (positive discriminant):
if let Ok(action) = serde_json::from_str::<ActionRequest>(&line) {
    tokio::spawn(handle_action(action, stdin_tx.clone(), auth_phase.clone()));
} else if let Ok(resp) = serde_json::from_str::<RpcResponse>(&line) {
    // existing id-based dispatch to pending map
}
```

### `handle_action`

```rust
async fn handle_action(
    req: ActionRequest,
    stdin_tx: mpsc::UnboundedSender<String>,
    auth_phase: SharedAuthPhase,
)
```

**`"auth_allocate_port"`:**
```
lock auth_phase
match phase:
  Idle | Allocated(_) → call auth::allocate_port(), set phase = Allocated(receiver), respond {"port": N}
  InProgress → respond error: "auth_already_in_progress"
unlock
```

**`"auth_open_and_wait"`:**
```
lock auth_phase
match phase:
  Allocated(receiver) → take receiver, set phase = InProgress
  Idle → respond error: "no_port_allocated" immediately; return
  InProgress → respond error: "auth_already_in_progress"; return
unlock (before await — no lock held across wait)

result = auth::open_and_wait(url, receiver, timeout).await

lock auth_phase; set phase = Idle; unlock

respond with result
```

If `url` is absent from params: respond `error: "invalid_params"` without touching `AuthPhase`.

Unknown action: respond `error: "unknown_action"`.

### Concurrent `auth_allocate_port` race protection

`AuthPhase::InProgress` prevents a new `allocate_port` from clobbering a mid-flight auth — the lock check rejects it immediately. `AuthPhase::Allocated(_)` allows re-allocation (plugin retry before starting the browser flow).

### RPC error table

| `ActionResponse.error` | Meaning |
|---|---|
| `"timed_out"` | Timeout |
| `"denied: <message>"` | OAuth `?error=` |
| `"no_port_allocated"` | `open_and_wait` without prior `allocate_port` |
| `"browser_open_failed: <message>"` | OS launcher failed |
| `"auth_already_in_progress"` | Auth flow already running |
| `"invalid_params"` | Required param absent |
| `"unknown_action"` | Unrecognised action string |

### Plugin restart during auth

If a plugin is restarted mid-auth, the in-progress auth is silently abandoned — `OAuthReceiver` is dropped with the `AuthPhase`, the callback server shuts down via `tx.closed()`, and the browser redirect returns connection-refused. User must retry. No supervisor-level cancellation needed.

### RPC wire example

```json
→ {"action":"auth_allocate_port","id":"a1"}
← {"action_id":"a1","result":{"port":52314}}

→ {"action":"auth_open_and_wait","id":"a2",
   "params":{"url":"https://accounts.spotify.com/authorize?...&redirect_uri=http://localhost:52314/callback","timeout_ms":120000}}
← {"action_id":"a2","result":{"code":"abc123","state":"xyz"}}
```

---

## 4. SDK Helpers (`sdk/src/lib.rs`)

```rust
// sdk::OAuthCallback — distinct from runtime's auth::OAuthCallback.
// code is String (non-optional) because Ok/Err already encodes presence.
pub struct OAuthCallback {
    pub code: String,
    pub state: Option<String>,
}

/// Allocates a local callback port. Call before constructing the OAuth URL.
pub fn auth_allocate_port() -> Result<u16, String>
// Returns Err("port_allocation_failed") if stui_auth_allocate_port() returns -1.

/// Opens the browser at `url` and blocks until the OAuth callback arrives or
/// `timeout_ms` elapses. `timeout_ms` (u32) is cast to i32 via saturating cast
/// before passing to the host (values > i32::MAX become i32::MAX; host clamps to
/// [1000, 300000]).
/// Returns Ok(OAuthCallback) if code is present.
/// Possible error strings: "timed_out", "denied: <msg>",
///   "no_port_allocated", "browser_open_failed: <msg>"
pub fn auth_open_and_wait(url: &str, timeout_ms: u32) -> Result<OAuthCallback, String>
```

**Result mapping:** `code: Some(c)` → `Ok(OAuthCallback { code: c, state })`; `error: Some(e)` → `Err("denied: <e>")` or `Err("timed_out")` etc.; both absent (malformed callback) → `Err("timed_out")` as safe fallback.

**Memory:** The packed pointer returned by `stui_auth_open_and_wait` is read and not freed — matching the established SDK pattern (`http_get`, `cache_get` both follow the same convention).

**New `http_post_form` helper** (required for token exchange in §5):

```rust
/// Make an HTTP POST with Content-Type: application/x-www-form-urlencoded.
/// Uses the existing stui_http_post host function with the __stui_headers override.
pub fn http_post_form(url: &str, body: &str) -> Result<String, String>
// Payload sent to host:
// {"url":"...","body":"...","__stui_headers":{"Content-Type":"application/x-www-form-urlencoded"}}
```

**Plugin type and capabilities:** The SoundCloud plugin uses `type = "stream-provider"` in `plugin.toml`. Capabilities (`Catalog`, `Streams`) are derived from the type by the runtime — there is no `capabilities` array in the manifest or SDK. The auth host imports are always registered unconditionally; no special type declaration is needed to use them.

---

## 5. SoundCloud Demo Plugin (`plugins/soundcloud/`)

**API note:** SoundCloud's official API registration closed ~2019. This demo uses the public v2 API with a `CLIENT_ID` extracted from the web bundle (same approach as `yt-dlp`/`scdl`). SoundCloud's token endpoint does not require a `CLIENT_SECRET` for web-app-extracted client IDs (public client OAuth model). The demo requests `scope=non-expiring` so tokens persist until revoked — no refresh logic needed. The demo is a pattern reference and may break if SoundCloud rotates its public `CLIENT_ID`.

### `Cargo.toml`

```toml
[package]
name = "soundcloud"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
stui-sdk = { path = "../../sdk" }
serde_json = { version = "1", default-features = false, features = ["alloc"] }
```

### `plugin.toml` (matches actual `PluginMeta` schema)

```toml
[plugin]
name = "soundcloud"
version = "0.1.0"
type = "stream-provider"
entrypoint = "soundcloud.wasm"
description = "SoundCloud music streaming (OAuth browser-auth demo)"

[permissions]
network_hosts = ["api.soundcloud.com", "secure.soundcloud.com"]
```

No `[env]` section — `CLIENT_ID` is embedded as a compile-time constant.

### Auth flow (lazy, called on first search)

```rust
fn ensure_authenticated() -> Result<String, String> {
    if let Some(token_json) = cache_get("sc_token") {
        return Ok(parse_access_token(&token_json)?);
    }
    let port = auth_allocate_port()?;
    let redirect_uri = format!("http://localhost:{port}/callback");
    let url = format!(
        "https://secure.soundcloud.com/authorize\
         ?client_id={CLIENT_ID}&redirect_uri={redirect_uri}\
         &response_type=code&scope=non-expiring"
    );
    let cb = auth_open_and_wait(&url, 120_000)?;
    let token_json = exchange_code(&cb.code, &redirect_uri)?;
    // scope=non-expiring: no TTL needed
    cache_set("sc_token", &token_json);
    Ok(parse_access_token(&token_json)?)
}
```

### `exchange_code`

```rust
fn exchange_code(code: &str, redirect_uri: &str) -> Result<String, String> {
    // POST https://api.soundcloud.com/oauth2/token
    // Content-Type: application/x-www-form-urlencoded (via http_post_form)
    // redirect_uri must match exactly what was sent to /authorize
    let body = format!(
        "grant_type=authorization_code&code={code}\
         &redirect_uri={redirect_uri}&client_id={CLIENT_ID}"
    );
    // No client_secret required (public client)
    // Note: for production plugins, `code` and `redirect_uri` must be percent-encoded
    // before concatenation. OAuth codes are typically URL-safe, but correctness requires it.
    http_post_form("https://api.soundcloud.com/oauth2/token", &body)
    // Response: {"access_token":"...","token_type":"bearer",...}
    // Stored as-is; parse_access_token extracts the access_token field
}
```

---

## File Map

| Action | File | What changes |
|---|---|---|
| Create | `runtime/src/auth/mod.rs` | Re-exports, `open_and_wait`, `AuthError` |
| Create | `runtime/src/auth/callback_server.rs` | Raw TcpListener server, `OAuthCallback`, `OAuthReceiver`, `allocate_port` |
| Create | `runtime/src/auth/browser.rs` | Cross-platform `open_url` |
| Modify | `runtime/src/lib.rs` | Add `pub mod auth;` |
| Modify | `runtime/src/abi/host.rs` | `store` → `tokio::sync::Mutex`; `call_export` → `call_async`; 2 new host imports; `auth_receiver` in `HostState` |
| Modify | `runtime/src/plugin_rpc/protocol.rs` | Add `ActionRequest`, `ActionResponse` |
| Modify | `runtime/src/plugin_rpc/process.rs` | stdin unbounded channel refactor; `AuthPhase`; demux; `handle_action` |
| Modify | `sdk/src/lib.rs` | `extern "C"` decls (wasm32 gated); `auth_allocate_port`, `auth_open_and_wait`, `http_post_form`, `sdk::OAuthCallback` |
| Create | `plugins/soundcloud/Cargo.toml` | WASM crate (`crate-type = ["cdylib"]`, dep on `stui-sdk`) |
| Create | `plugins/soundcloud/plugin.toml` | Plugin discovery manifest |
| Create | `plugins/soundcloud/src/lib.rs` | Demo plugin |

---

## Error Handling

| Scenario | Behaviour |
|---|---|
| User does not complete auth | `open_and_wait` → `TimedOut`; server shuts down via `tx.closed()` |
| OAuth `?error=access_denied` | `open_and_wait` → `Denied { message }` |
| `open_and_wait` without `allocate_port` (WASM) | `{"error":"no_port_allocated"}` immediately |
| `open_and_wait` without `allocate_port` (RPC) | `error: "no_port_allocated"` immediately |
| Concurrent auth in RPC | `error: "auth_already_in_progress"` immediately |
| Plugin crashes during auth | Receiver dropped → server shuts down via `tx.closed()` |
| `url` missing from RPC params | `error: "invalid_params"` immediately |
| `timeout_ms` out of range | Clamped to [1000, 300000] |
| Receiver dropped before callback (e.g. server panic) | `AuthError::ReceiverDropped` |

---

## Testing

- **Unit (`callback_server`)**: bind to port 0; GET `/callback?code=abc&state=xyz` from test HTTP client; assert receiver fires with correct fields and server shuts down; drop receiver without sending — assert server task exits (via `tx.closed()` path); test `?error=access_denied`
- **Unit (`browser`)**: mock subprocess; assert correct binary per platform; assert spawned as detached
- **Integration (WASM)**: load minimal test WASM that calls `stui_auth_allocate_port` then `stui_auth_open_and_wait`; test harness POSTs to returned port; assert result JSON; assert `stui_auth_open_and_wait` without prior `stui_auth_allocate_port` returns `no_port_allocated`
- **Integration (RPC)**: spawn test plugin script emitting action messages; harness POSTs to port; assert `ActionResponse` written to plugin stdin; test `InProgress` rejection
- **Unit (SDK)**: `code=None, error=None` → `Err("timed_out")`; `code=Some` → `Ok`; `error=Some("access_denied")` → `Err("denied: access_denied")`; `timeout_ms > i32::MAX` → saturating cast; `http_post_form` encodes headers correctly
- **Unit (SoundCloud)**: token cache hit (no auth flow); OAuth URL construction (`redirect_uri` present); `exchange_code` body format; error propagation from `auth_open_and_wait`
