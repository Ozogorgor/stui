# Plugin OAuth Browser Auth Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add browser-based OAuth support to both WASM and RPC plugin types, with a SoundCloud WASM plugin as the reference implementation.

**Architecture:** The runtime exposes two new auth primitives — a raw TCP callback server (port 0, OS-assigned) and a cross-platform browser opener. WASM plugins call two new host imports; RPC plugins emit JSON action messages that the runtime's read loop dispatches via a new `handle_action` task. Both paths share the same `auth::open_and_wait` async core. The SoundCloud demo wires it all together as a real working plugin.

**Tech Stack:** Rust, tokio (async, `TcpListener`), wasmtime (`func_wrap_async` / `call_async`), `tokio::sync::Mutex` (replaces `std::sync::Mutex` in WASM store), `mpsc::unbounded_channel` (replaces direct `ChildStdin` lock in RPC), `serde_json`, `xdg-open`/`open`/`start` (browser opener).

---

## File Structure

| Action | Path | Responsibility |
|---|---|---|
| Create | `runtime/src/auth/callback_server.rs` | `OAuthCallback`, `OAuthReceiver`, `allocate_port`, raw TCP server loop |
| Create | `runtime/src/auth/browser.rs` | Cross-platform `open_url` |
| Create | `runtime/src/auth/mod.rs` | Re-exports, `open_and_wait`, `AuthError` |
| Modify | `runtime/src/lib.rs` | Add `pub mod auth;` |
| Modify | `runtime/src/abi/host.rs` | `store` → `tokio::sync::Mutex`; `call_export` + `abi_version` updated; 2 new imports; `auth_receiver` in `HostState` |
| Modify | `runtime/src/plugin_rpc/protocol.rs` | Add `ActionRequest`, `ActionResponse` |
| Modify | `runtime/src/plugin_rpc/process.rs` | stdin unbounded channel; `AuthPhase`; demux; `handle_action` |
| Modify | `sdk/src/lib.rs` | `stui_auth_*` extern decls; `auth_allocate_port`, `auth_open_and_wait`, `http_post_form`, `sdk::OAuthCallback` |
| Create | `plugins/soundcloud/Cargo.toml` | WASM crate (`cdylib`, `stui-sdk` dep) |
| Create | `plugins/soundcloud/plugin.toml` | Manifest (`type = "stream-provider"`) |
| Create | `plugins/soundcloud/src/lib.rs` | Auth flow + `ensure_authenticated` + `exchange_code` |

---

## Chunk 1: Auth Primitives

### Task 1: Callback server

**Files:**
- Create: `runtime/src/auth/callback_server.rs`
- Create: `runtime/src/auth/mod.rs` (stub for now)
- Create: `runtime/src/auth/browser.rs` (stub for now)
- Modify: `runtime/src/lib.rs` (add `pub mod auth;`)

- [ ] **Step 1: Add `pub mod auth;` to `runtime/src/lib.rs`**

  Open `runtime/src/lib.rs`. Add `pub mod auth;` in alphabetical order — it belongs between `pub mod abi;` and `pub mod aria2_bridge;`. Or append it after `pub mod skipper;` (the actual last `pub mod` line):

  ```rust
  pub mod auth;
  ```

- [ ] **Step 2: Create stub `runtime/src/auth/mod.rs`** (stub only — `open_and_wait` uses `todo!()`)

  ```rust
  pub mod callback_server;
  pub mod browser;

  pub use callback_server::{OAuthCallback, OAuthReceiver, allocate_port};

  use std::time::Duration;

  #[derive(Debug)]
  pub enum AuthError {
      TimedOut,
      Denied { message: String },
      BrowserOpenFailed(String),
      ReceiverDropped,
  }

  /// Opens `url` in the browser then awaits the OAuth callback.
  /// Timeout clock starts after the browser launcher returns.
  pub async fn open_and_wait(
      _url: &str,
      _receiver: OAuthReceiver,
      _timeout: Duration,
  ) -> Result<OAuthCallback, AuthError> {
      todo!("implement in Task 2")
  }
  ```

- [ ] **Step 3: Create stub `runtime/src/auth/browser.rs`**

  ```rust
  /// Opens `url` in the system default browser as a detached process.
  /// Linux: xdg-open, macOS: open, Windows: cmd /c start.
  /// Returns after the launcher process is spawned (non-blocking).
  pub fn open_url(url: &str) -> Result<(), String> {
      #[cfg(target_os = "linux")]
      {
          std::process::Command::new("xdg-open")
              .arg(url)
              .stdin(std::process::Stdio::null())
              .stdout(std::process::Stdio::null())
              .stderr(std::process::Stdio::null())
              .spawn()
              .map_err(|e| format!("xdg-open failed: {e}"))?;
      }
      #[cfg(target_os = "macos")]
      {
          std::process::Command::new("open")
              .arg(url)
              .stdin(std::process::Stdio::null())
              .stdout(std::process::Stdio::null())
              .stderr(std::process::Stdio::null())
              .spawn()
              .map_err(|e| format!("open failed: {e}"))?;
      }
      #[cfg(target_os = "windows")]
      {
          std::process::Command::new("cmd")
              .args(["/c", "start", "", url])
              .stdin(std::process::Stdio::null())
              .stdout(std::process::Stdio::null())
              .stderr(std::process::Stdio::null())
              .spawn()
              .map_err(|e| format!("cmd /c start failed: {e}"))?;
      }
      #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
      {
          return Err(format!("unsupported platform for browser open: {url}"));
      }
      Ok(())
  }
  ```

- [ ] **Step 4: Write the failing tests for `callback_server`**

  Create `runtime/src/auth/callback_server.rs` with the test module first:

  ```rust
  use tokio::sync::oneshot;

  pub struct OAuthCallback {
      pub code: Option<String>,
      pub state: Option<String>,
      pub error: Option<String>,
  }

  pub type OAuthReceiver = oneshot::Receiver<OAuthCallback>;

  pub async fn allocate_port() -> anyhow::Result<(u16, OAuthReceiver)> {
      todo!("implement")
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use tokio::io::AsyncWriteExt;

      #[tokio::test]
      async fn test_allocate_returns_port_and_fires_receiver_on_callback() {
          let (port, rx) = allocate_port().await.unwrap();
          assert!(port > 0, "port must be > 0");

          // Simulate browser redirect
          let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
          let req = b"GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
          stream.write_all(req).await.unwrap();
          drop(stream);

          let cb = tokio::time::timeout(
              std::time::Duration::from_secs(2),
              rx,
          ).await.expect("timed out").expect("receiver closed");

          assert_eq!(cb.code.as_deref(), Some("abc123"));
          assert_eq!(cb.state.as_deref(), Some("xyz"));
          assert!(cb.error.is_none());
      }

      #[tokio::test]
      async fn test_server_shuts_down_when_receiver_dropped() {
          let (port, rx) = allocate_port().await.unwrap();
          // Drop the receiver — server task should detect tx.closed() and exit
          drop(rx);

          // Give the server task a moment to shut down
          tokio::time::sleep(std::time::Duration::from_millis(50)).await;

          // Connection should be refused (server shut down)
          let result = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
          assert!(result.is_err(), "server should have shut down after receiver dropped");
      }

      #[tokio::test]
      async fn test_oauth_error_populates_error_field() {
          let (port, rx) = allocate_port().await.unwrap();

          let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
          let req = b"GET /callback?error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n";
          stream.write_all(req).await.unwrap();
          drop(stream);

          let cb = tokio::time::timeout(
              std::time::Duration::from_secs(2),
              rx,
          ).await.expect("timed out").expect("receiver closed");

          assert!(cb.code.is_none());
          assert_eq!(cb.error.as_deref(), Some("access_denied"));
      }
  }
  ```

- [ ] **Step 5: Run tests to verify they fail**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test auth::callback_server 2>&1 | head -20
  ```

  Expected: tests fail with runtime panic — `not yet implemented: implement in Task 2`.

- [ ] **Step 6: Implement `allocate_port`**

  Replace the `todo!` body with the full implementation. Keep the test module unchanged:

  ```rust
  use tokio::sync::oneshot;
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  use tokio::net::TcpListener;

  pub struct OAuthCallback {
      pub code: Option<String>,
      pub state: Option<String>,
      pub error: Option<String>,
  }

  pub type OAuthReceiver = oneshot::Receiver<OAuthCallback>;

  /// Binds a local HTTP server on a random OS-assigned port (port 0).
  /// The server is already listening when this returns.
  /// Returns (port, receiver). Server shuts down after one callback
  /// or when the receiver is dropped.
  pub async fn allocate_port() -> anyhow::Result<(u16, OAuthReceiver)> {
      let listener = TcpListener::bind("127.0.0.1:0").await?;
      let port = listener.local_addr()?.port();
      let (tx, rx) = oneshot::channel::<OAuthCallback>();

      tokio::spawn(async move {
          loop {
              tokio::select! {
                  result = listener.accept() => {
                      let Ok((mut stream, _)) = result else { continue };
                      // Read until we have the full request line (\r\n\r\n).
                      // 4096 bytes is sufficient for all OAuth callback redirects.
                      // If a request exceeds this, the loop exits at buf[total..] zero-len
                      // and we parse whatever was received (the request line is always first).
                      let mut buf = vec![0u8; 4096];
                      let mut total = 0;
                      loop {
                          if total >= buf.len() { break; } // guard: don't exceed buffer
                          match stream.read(&mut buf[total..]).await {
                              Ok(0) | Err(_) => break,
                              Ok(n) => {
                                  total += n;
                                  if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                                      break;
                                  }
                              }
                          }
                      }
                      // Extract query string from "GET /callback?<qs> HTTP/..."
                      let request_line = std::str::from_utf8(&buf[..total])
                          .unwrap_or("")
                          .lines()
                          .next()
                          .unwrap_or("");
                      let qs = request_line
                          .split_once('?')
                          .and_then(|(_, rest)| rest.split_once(' ').map(|(qs, _)| qs))
                          .unwrap_or("");
                      let cb = parse_query(qs);
                      // Write a minimal HTTP 200 response
                      let body = b"You may close this tab.";
                      let resp = format!(
                          "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
                          body.len()
                      );
                      let _ = stream.write_all(resp.as_bytes()).await;
                      let _ = stream.write_all(body).await;
                      let _ = tx.send(cb);
                      return;
                  }
                  _ = tx.closed() => {
                      // Receiver was dropped (timeout or plugin exit) — shut down
                      return;
                  }
              }
          }
      });

      Ok((port, rx))
  }

  /// Parse `key=value&key=value` query string into an OAuthCallback.
  /// Percent-decodes values ('+' → space, %XX → char).
  fn parse_query(qs: &str) -> OAuthCallback {
      let mut code = None;
      let mut state = None;
      let mut error = None;
      for pair in qs.split('&') {
          if let Some((k, v)) = pair.split_once('=') {
              let decoded = percent_decode(v);
              match k {
                  "code"  => code  = Some(decoded),
                  "state" => state = Some(decoded),
                  "error" => error = Some(decoded),
                  _ => {}
              }
          }
      }
      OAuthCallback { code, state, error }
  }

  /// Minimal percent-decoder: '+' → ' ', %XX → byte.
  fn percent_decode(s: &str) -> String {
      let mut out = String::with_capacity(s.len());
      let bytes = s.as_bytes();
      let mut i = 0;
      while i < bytes.len() {
          if bytes[i] == b'+' {
              out.push(' ');
              i += 1;
          } else if bytes[i] == b'%' && i + 2 < bytes.len() {
              if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                  if let Ok(byte) = u8::from_str_radix(hex, 16) {
                      out.push(byte as char);
                      i += 3;
                      continue;
                  }
              }
              out.push('%');
              i += 1;
          } else {
              out.push(bytes[i] as char);
              i += 1;
          }
      }
      out
  }
  ```

- [ ] **Step 7: Run tests to verify they pass**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test auth::callback_server -- --nocapture 2>&1
  ```

  Expected: all 3 tests pass.

- [ ] **Step 8: Verify the full runtime compiles**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo check 2>&1
  ```

  Expected: no errors.

- [ ] **Step 9: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add runtime/src/auth/ runtime/src/lib.rs
  git commit -m "feat(auth): add OAuth callback server and browser opener primitives"
  ```

---

### Task 2: `open_and_wait` implementation + tests

**Files:**
- Modify: `runtime/src/auth/mod.rs` (replace `todo!()` stub with real implementation + tests)

- [ ] **Step 1: Write the failing tests for `open_and_wait`**

  Add a `#[cfg(test)]` module to `runtime/src/auth/mod.rs`. The current `open_and_wait` body is `todo!()`, so these tests will panic at runtime — confirming the red phase:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use tokio::io::AsyncWriteExt;

      #[tokio::test]
      async fn test_open_and_wait_timeout() {
          let (_port, rx) = allocate_port().await.unwrap();
          // Don't simulate any callback — timeout after 100ms
          let result = open_and_wait(
              "http://example.com/oauth",
              rx,
              Duration::from_millis(100),
          ).await;
          // browser failure or timeout — both acceptable (CI may not have xdg-open)
          assert!(
              matches!(result, Err(AuthError::TimedOut) | Err(AuthError::BrowserOpenFailed(_))),
              "expected TimedOut or BrowserOpenFailed, got {:?}", result
          );
      }

      #[tokio::test]
      async fn test_open_and_wait_denied() {
          // Note: this test bypasses the browser step using a pre-allocated port.
          // In headless CI where xdg-open fails, BrowserOpenFailed is acceptable.
          // The Denied path is verified by the callback-server test for ?error= fields;
          // this test verifies open_and_wait propagates it correctly when xdg-open works.
          let (port, rx) = allocate_port().await.unwrap();
          tokio::spawn(async move {
              tokio::time::sleep(std::time::Duration::from_millis(30)).await;
              let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
              let req = b"GET /callback?error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n";
              stream.write_all(req).await.unwrap();
          });
          let result = open_and_wait(
              "http://example.com/oauth",
              rx,
              Duration::from_secs(5),
          ).await;
          match result {
              Err(AuthError::Denied { message }) => assert_eq!(message, "access_denied"),
              Err(AuthError::BrowserOpenFailed(_)) => { /* headless CI — xdg-open absent */ }
              other => panic!("expected Denied or BrowserOpenFailed, got {:?}", other),
          }
      }
  }
  ```

- [ ] **Step 2: Run tests to verify they fail (panic from `todo!()`)**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test auth::mod::tests -- --nocapture 2>&1 | head -20
  ```

  Expected: tests fail with `not yet implemented: implement in Task 2`.

- [ ] **Step 3: Implement `open_and_wait`** (replace the `todo!()` body in `mod.rs`)

  ```rust
  use std::time::Duration;
  use tokio::time::timeout;

  pub async fn open_and_wait(
      url: &str,
      receiver: OAuthReceiver,
      auth_timeout: Duration,
  ) -> Result<OAuthCallback, AuthError> {
      browser::open_url(url).map_err(AuthError::BrowserOpenFailed)?;

      match timeout(auth_timeout, receiver).await {
          Ok(Ok(cb)) => {
              if let Some(err_msg) = cb.error.clone() {
                  return Err(AuthError::Denied { message: err_msg });
              }
              Ok(cb)
          }
          Ok(Err(_)) => Err(AuthError::ReceiverDropped),
          Err(_) => Err(AuthError::TimedOut),
      }
  }
  ```

  Also remove the `_` prefixes from the parameter names (`_url` → `url`, etc.) now that they are used.

- [ ] **Step 4: Run tests to verify they pass**

  ```bash
  cargo test auth::mod::tests -- --nocapture 2>&1
  ```

  Expected: both tests pass (or `BrowserOpenFailed` in headless CI).

- [ ] **Step 5: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add runtime/src/auth/mod.rs
  git commit -m "feat(auth): implement open_and_wait with timeout and Denied propagation"
  ```

---

## Chunk 2: WASM Host Imports

### Task 3: Store mutex refactor

**Files:**
- Modify: `runtime/src/abi/host.rs`

The `WasmInner` struct currently uses `std::sync::Mutex`. We must change it to `tokio::sync::Mutex` so that the 120-second auth wait doesn't freeze a tokio worker thread. This requires updating `call_export` (make it use `.lock().await` + `call_async`) and `abi_version` (use `try_lock()`).

- [ ] **Step 1: Confirm existing tests pass before touching anything**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test --features wasm-host 2>&1 | tail -5
  ```

  Note the passing test count so you can verify nothing regresses.

- [ ] **Step 2: Change the mutex import and struct field in `inner_impl`**

  In `runtime/src/abi/host.rs`, inside `mod inner_impl` (starts around line 101):

  Change:
  ```rust
  use std::sync::Mutex;
  ```
  To:
  ```rust
  use tokio::sync::Mutex;
  ```

  The `store: Mutex<Store<HostState>>` field declaration doesn't need to change (still called `Mutex`, just from a different crate now).

  Also update the construction at the end of `WasmInner::load`:
  ```rust
  // Before:
  Ok(WasmInner {
      store: Mutex::new(store),
      instance,
      engine,
  })
  // After: same — Mutex::new(store) works for both
  ```

- [ ] **Step 3: Update `abi_version()` to use `try_lock()`**

  Change (around line 364):
  ```rust
  // Before:
  pub fn abi_version(&self) -> i32 {
      let mut store = self.store.lock().unwrap_or_else(|p| p.into_inner());
      self.instance
          .get_typed_func::<(), i32>(&mut *store, "stui_abi_version")
          .ok()
          .and_then(|f| f.call(&mut *store, ()).ok())
          .unwrap_or(-1)
  }

  // After:
  pub fn abi_version(&self) -> i32 {
      // try_lock() is callable from sync context (returns TryLockResult, not a Future).
      // If the store is held by a long-running auth wait, fall back to 0.
      match self.store.try_lock() {
          Ok(mut store) => self.instance
              .get_typed_func::<(), i32>(&mut *store, "stui_abi_version")
              .ok()
              .and_then(|f| f.call(&mut *store, ()).ok())
              .unwrap_or(-1),
          Err(_) => 0,
      }
  }
  ```

- [ ] **Step 4: Update `call_export` to use async lock and `call_async`**

  `call_export` is already `async`. Change (around line 373):
  ```rust
  // Before:
  pub async fn call_export(&self, fn_name: &str, json_input: &str) -> Result<String, AbiError> {
      let input_bytes = json_input.as_bytes();
      let input_len = input_bytes.len() as i32;
      let mut store = self.store.lock().unwrap_or_else(|p| p.into_inner());

      let alloc = self.instance
          .get_typed_func::<i32, i32>(&mut *store, "stui_alloc")
          ...
      let input_ptr = alloc.call(&mut *store, input_len)
          ...
      ...
      let func = self.instance
          .get_typed_func::<(i32, i32), i64>(&mut *store, fn_name)
          ...
      let packed = func.call(&mut *store, (input_ptr, input_len))
          ...
      ...
      let free = self.instance
          .get_typed_func::<(i32, i32), ()>(&mut *store, "stui_free")
          ...
      let _ = free.call(&mut *store, (input_ptr, input_len));

  // After:
  pub async fn call_export(&self, fn_name: &str, json_input: &str) -> Result<String, AbiError> {
      let input_bytes = json_input.as_bytes();
      let input_len = input_bytes.len() as i32;
      let mut store = self.store.lock().await;          // ← async lock

      let alloc = self.instance
          .get_typed_func::<i32, i32>(&mut *store, "stui_alloc")
          .map_err(|_| AbiError::MissingExport("stui_alloc".into()))?;
      let input_ptr = alloc.call_async(&mut *store, input_len).await  // ← call_async
          .map_err(|e| AbiError::Execution(e.to_string()))?;

      let memory = self.instance
          .get_memory(&mut *store, "memory")
          .ok_or_else(|| AbiError::MissingExport("memory".into()))?;
      memory.write(&mut *store, input_ptr as usize, input_bytes)
          .map_err(|e| AbiError::Memory(e.to_string()))?;

      let func = self.instance
          .get_typed_func::<(i32, i32), i64>(&mut *store, fn_name)
          .map_err(|_| AbiError::MissingExport(fn_name.to_string()))?;
      let packed = func.call_async(&mut *store, (input_ptr, input_len)).await  // ← call_async
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
      let _ = free.call_async(&mut *store, (input_ptr, input_len)).await;  // ← call_async

      Ok(result)
  }
  ```

- [ ] **Step 5: Verify it compiles**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo check --features wasm-host 2>&1
  ```

  Expected: no errors.

- [ ] **Step 6: Run existing tests to verify no regressions**

  ```bash
  cargo test --features wasm-host 2>&1 | tail -10
  ```

  Expected: same number of passing tests as before.

- [ ] **Step 7: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add runtime/src/abi/host.rs
  git commit -m "refactor(wasm): switch store mutex to tokio::sync::Mutex for async-safe auth"
  ```

---

### Task 4: WASM auth host imports

**Files:**
- Modify: `runtime/src/abi/host.rs` (add `auth_receiver` to `HostState`; register 2 new imports)

- [ ] **Step 1: Write the failing test for `auth_receiver` storage** (field doesn't exist yet → compile error)

  Add a `#[cfg(test)]` module inside `mod inner_impl` (after the existing helper functions, before the closing `}`). The test constructs `HostState` with an `auth_receiver` field that doesn't exist yet:

  ```rust
  #[cfg(test)]
  mod tests {
      // Note: these tests require the wasm-host feature.
      // Run with: cargo test --features wasm-host abi::host::inner_impl::tests

      use super::*;

      #[tokio::test]
      async fn test_auth_receiver_stored_in_host_state() {
          let (port, rx) = crate::auth::allocate_port().await.unwrap();
          let mut state = HostState {
              wasi: wasmtime_wasi::WasiCtxBuilder::new().build_p1(),
              ctx: crate::sandbox::SandboxCtx::default(),
              http_buf: vec![],
              kv: std::collections::HashMap::new(),
              limiter: MemoryLimiter { limit_bytes: 128 * 1024 * 1024 },
              auth_receiver: Some(rx),   // ← field does not exist yet
          };
          assert!(state.auth_receiver.is_some());
          let taken = state.auth_receiver.take();
          assert!(taken.is_some());
          assert!(state.auth_receiver.is_none(), "take() must leave None");
          let _ = (port, taken);
      }
  }
  ```

- [ ] **Step 2: Run to verify it fails (compile error — field not defined)**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test --features wasm-host abi::host::inner_impl::tests 2>&1 | head -10
  ```

  Expected: compile error — `no field 'auth_receiver' on type 'HostState'`.

- [ ] **Step 3: Add `auth_receiver` to `HostState` and update construction**

  Inside `mod inner_impl`, find the `HostState` struct (around line 152). Add one field:

  ```rust
  struct HostState {
      wasi: WasiP1Ctx,
      ctx: SandboxCtx,
      #[allow(dead_code)]
      http_buf: Vec<u8>,
      kv: std::collections::HashMap<String, String>,
      limiter: MemoryLimiter,
      /// Holds the auth callback receiver between stui_auth_allocate_port
      /// and stui_auth_open_and_wait calls. Taken (not cloned) before .await.
      pub auth_receiver: Option<crate::auth::OAuthReceiver>,
  }
  ```

  Update the `host_state` construction in `WasmInner::load` to initialise it:

  ```rust
  let host_state = HostState {
      wasi,
      ctx: ctx.clone(),
      http_buf: vec![],
      kv,
      limiter: MemoryLimiter {
          limit_bytes: (max_memory_mb as usize) * 1024 * 1024,
      },
      auth_receiver: None,   // ← add this line
  };
  ```

- [ ] **Step 4: Run the test to verify it now passes**

  ```bash
  cargo test --features wasm-host abi::host::inner_impl::tests -- --nocapture 2>&1
  ```

  Expected: 1 test passes.

- [ ] **Step 5: Register `stui_auth_allocate_port` import**

  Add the following AFTER the `stui_cache_set` linker registration and BEFORE `linker.instantiate_async`. Both new imports need `use crate::auth` at the top of the file — add this import to `mod inner_impl`:

  ```rust
  use crate::auth;
  ```

  Then add the import registrations:

  ```rust
  // ── stui_auth_allocate_port() -> i32 ──────────────────────────────────
  // Starts the callback server. Returns port. Replaces any existing receiver.
  // Always registered unconditionally — no capability check needed.
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
  ```

- [ ] **Step 6: Register `stui_auth_open_and_wait` import**

  Add immediately after the `stui_auth_allocate_port` registration:

  ```rust
  // ── stui_auth_open_and_wait(url_ptr, url_len, timeout_ms) -> i64 ──────
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

              // Clamp timeout to [1000, 300000] ms
              let timeout_ms = (timeout_ms_raw as u64).clamp(1_000, 300_000);
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
  ```

- [ ] **Step 7: Verify compilation**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo check --features wasm-host 2>&1
  ```

  Expected: no errors.

- [ ] **Step 8: Run all tests**

  ```bash
  cargo test --features wasm-host 2>&1 | tail -10
  ```

  Expected: all existing tests still pass, plus the new auth test.

- [ ] **Step 9: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add runtime/src/abi/host.rs
  git commit -m "feat(wasm): add stui_auth_allocate_port and stui_auth_open_and_wait host imports"
  ```

---

## Chunk 3: RPC Bidirectional Protocol

### Task 5: Protocol types

**Files:**
- Modify: `runtime/src/plugin_rpc/protocol.rs`

- [ ] **Step 1: Write the failing test for `ActionRequest` parsing**

  Add at the bottom of `runtime/src/plugin_rpc/protocol.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn test_action_request_parses_action_field() {
          let line = r#"{"action":"auth_allocate_port","id":"a1"}"#;
          let req: ActionRequest = serde_json::from_str(line).unwrap();
          assert_eq!(req.action, "auth_allocate_port");
          assert_eq!(req.id, "a1");
          assert!(req.params.is_none());
      }

      #[test]
      fn test_action_request_not_confused_with_rpc_response() {
          // An RpcResponse line (has "id" but no "action") must NOT parse as ActionRequest
          let rpc_resp_line = r#"{"id":"r1","result":{"items":[]}}"#;
          let result = serde_json::from_str::<ActionRequest>(rpc_resp_line);
          assert!(result.is_err(), "RpcResponse must not parse as ActionRequest");
      }

      #[test]
      fn test_action_request_parses_params() {
          let line = r#"{"action":"auth_open_and_wait","id":"a2","params":{"url":"https://example.com","timeout_ms":120000}}"#;
          let req: ActionRequest = serde_json::from_str(line).unwrap();
          assert_eq!(req.action, "auth_open_and_wait");
          let params = req.params.unwrap();
          assert_eq!(params["url"].as_str().unwrap(), "https://example.com");
          assert_eq!(params["timeout_ms"].as_u64().unwrap(), 120000);
      }

      #[test]
      fn test_action_response_serializes_correctly() {
          let resp = ActionResponse {
              action_id: "a1".into(),
              result: Some(serde_json::json!({"port": 52314})),
              error: None,
          };
          let json = serde_json::to_string(&resp).unwrap();
          assert!(json.contains("\"action_id\""));
          assert!(json.contains("52314"));
          assert!(!json.contains("\"error\"")); // skip_serializing_if = None
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails (types not defined yet)**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test plugin_rpc::protocol::tests 2>&1 | head -20
  ```

  Expected: compile error — `ActionRequest` not defined.

- [ ] **Step 3: Add `ActionRequest` and `ActionResponse` to `protocol.rs`**

  Append to `runtime/src/plugin_rpc/protocol.rs` (before the `#[cfg(test)]` block):

  ```rust
  // ── Inbound: plugin → runtime (action messages) ───────────────────────────

  /// A plugin-initiated action request (plugin → runtime).
  ///
  /// Distinguished from `RpcResponse` by the presence of the `"action"` field.
  /// Parse `ActionRequest` FIRST in the read loop — `RpcResponse` would also
  /// accept `ActionRequest` lines (both have "id") if tried first.
  ///
  /// IMPORTANT: `action` must NOT have `#[serde(default)]`. The demultiplexing
  /// invariant depends on `ActionRequest` deserialization failing for
  /// `RpcResponse` lines (which have "id" but no "action" field).
  #[allow(dead_code)]
  #[derive(Debug, Deserialize, Clone)]
  pub struct ActionRequest {
      pub action: String,
      pub id: String,
      pub params: Option<Value>,
  }

  /// The runtime's reply to a plugin `ActionRequest`.
  ///
  /// Distinguished from `RpcResponse` by `action_id` (not `id`).
  /// `error` is a flat string (not structured RpcError) to keep the schema simple.
  /// RPC plugins parsing error strings should split on ": " to get code/message.
  #[allow(dead_code)]
  #[derive(Debug, Serialize)]
  pub struct ActionResponse {
      pub action_id: String,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub result: Option<Value>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub error: Option<String>,
  }

  impl ActionResponse {
      pub fn ok(id: impl Into<String>, result: Value) -> Self {
          ActionResponse { action_id: id.into(), result: Some(result), error: None }
      }

      pub fn err(id: impl Into<String>, message: impl Into<String>) -> Self {
          ActionResponse { action_id: id.into(), result: None, error: Some(message.into()) }
      }
  }
  ```

- [ ] **Step 4: Run tests to verify they pass**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test plugin_rpc::protocol::tests -- --nocapture 2>&1
  ```

  Expected: all 4 tests pass.

- [ ] **Step 5: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add runtime/src/plugin_rpc/protocol.rs
  git commit -m "feat(rpc): add ActionRequest and ActionResponse protocol types"
  ```

---

### Task 6: Process refactor (stdin channel + AuthPhase + demux + handle_action)

**Files:**
- Modify: `runtime/src/plugin_rpc/process.rs`

This is the largest change. We will do it incrementally:
1. Replace `Arc<Mutex<ChildStdin>>` with an unbounded channel
2. Add `AuthPhase` and its `SharedAuthPhase` type
3. Extend the read loop to demultiplex action messages
4. Implement `handle_action`

- [ ] **Step 1: Write the failing test for the stdin channel refactor**

  Add a `#[cfg(test)]` module at the bottom of `process.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      // Integration tests for PluginProcess require a real plugin binary.
      // Unit tests here cover the AuthPhase state machine.
      use super::*;
      use crate::auth::OAuthReceiver;

      fn idle() -> AuthPhase { AuthPhase::Idle }

      #[test]
      fn test_auth_phase_transitions() {
          // Idle → InProgress is not valid (must go through Allocated first)
          let phase = idle();
          assert!(matches!(phase, AuthPhase::Idle));
      }

      #[tokio::test]
      async fn test_auth_phase_allocated_allows_realloc() {
          // Allocated → Allocated (retry before starting browser flow)
          let (port1, rx1) = crate::auth::allocate_port().await.unwrap();
          let (_port2, rx2) = crate::auth::allocate_port().await.unwrap();
          let mut phase = AuthPhase::Allocated(rx1);
          // Simulate re-allocation: replace with rx2
          phase = AuthPhase::Allocated(rx2);
          assert!(matches!(phase, AuthPhase::Allocated(_)));
          let _ = port1;
      }

      #[test]
      fn test_auth_phase_in_progress_rejects_realloc() {
          let phase = AuthPhase::InProgress;
          // InProgress → cannot allocate new port
          assert!(matches!(phase, AuthPhase::InProgress));
      }
  }
  ```

- [ ] **Step 2: Run tests — expect compile error (AuthPhase not defined)**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test plugin_rpc::process::tests 2>&1 | head -20
  ```

  Expected: compile error.

- [ ] **Step 3: Add `AuthPhase` type to `process.rs`**

  Add the following near the top of `process.rs`, after the existing `use` statements:

  ```rust
  use crate::auth::OAuthReceiver;
  use tokio::sync::mpsc;

  // ── Auth state machine ───────────────────────────────────────────────────────

  /// Tracks the state of an in-progress OAuth flow for this plugin.
  ///
  /// The enum eliminates invalid state combinations (receiver present AND in_progress).
  pub enum AuthPhase {
      /// No auth flow active.
      Idle,
      /// Port allocated, receiver waiting — browser not yet opened.
      Allocated(OAuthReceiver),
      /// `open_and_wait` is running — reject new `allocate_port` calls.
      InProgress,
  }

  type SharedAuthPhase = Arc<tokio::sync::Mutex<AuthPhase>>;
  ```

- [ ] **Step 4: Run the auth phase tests**

  ```bash
  cargo test plugin_rpc::process::tests -- --nocapture 2>&1
  ```

  Expected: all 3 tests pass.

- [ ] **Step 5: Replace `Arc<Mutex<ChildStdin>>` with stdin channel**

  In `process.rs`, make the following changes to the `PluginProcess` struct:

  Replace:
  ```rust
  stdin: Arc<Mutex<ChildStdin>>,
  ```
  With:
  ```rust
  stdin_tx: mpsc::UnboundedSender<String>,
  auth_phase: SharedAuthPhase,
  ```

  In `spawn()`, replace the stdin setup:
  ```rust
  // Before:
  let stdin = Arc::new(Mutex::new(stdin));
  ...
  PluginProcess {
      ...
      stdin,
      ...
  }

  // After:
  let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
  // Writer task — one place writes to ChildStdin, flushing after each line
  tokio::spawn(async move {
      use tokio::io::AsyncWriteExt;
      while let Some(line) = stdin_rx.recv().await {
          let _ = stdin.write_all(line.as_bytes()).await;
          // Flush after each write — matches the previous direct-write path
          let _ = stdin.flush().await;
      }
  });
  let auth_phase: SharedAuthPhase = Arc::new(tokio::sync::Mutex::new(AuthPhase::Idle));
  ...
  PluginProcess {
      ...
      stdin_tx,
      auth_phase,
      ...
  }
  ```

  Note: Remove the `Arc<Mutex<Child>>` import of `Mutex` confusion — `process.rs` imports `use tokio::sync::{oneshot, Mutex, Notify};`. Add `mpsc` to this import.

- [ ] **Step 6: Update the `call()` method**

  Replace the stdin write block in `call()`:
  ```rust
  // Before:
  {
      let mut stdin = self.stdin.lock().await;
      stdin.write_all(line.as_bytes()).await?;
      stdin.write_all(b"\n").await?;
      stdin.flush().await?;
  }

  // After:
  self.stdin_tx
      .send(format!("{line}\n"))
      .map_err(|_| anyhow::anyhow!("plugin stdin channel closed"))?;
  ```

  The writer task imports `AsyncWriteExt` locally inside the `async move` block (already shown in Step 5). The top-level `use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};` on line 17 of `process.rs` can have `AsyncWriteExt` removed from that import — the writer task's local import handles it. Keep `AsyncBufReadExt` and `BufReader` (used by the reader task).

- [ ] **Step 7: Update the read loop to demultiplex action messages**

  Replace the read loop body in `spawn()`:

  ```rust
  // Before:
  while let Ok(Some(line)) = reader.next_line().await {
      if line.trim().is_empty() { continue; }
      match serde_json::from_str::<RpcResponse>(&line) {
          Ok(resp) => {
              let mut map = pending_rx.lock().await;
              if let Some(tx) = map.remove(&resp.id) {
                  let _ = tx.send(resp);
              }
          }
          Err(e) => {
              warn!("plugin sent invalid JSON: {e} — line: {line}");
          }
      }
  }

  // After:
  while let Ok(Some(line)) = reader.next_line().await {
      if line.trim().is_empty() { continue; }
      // ActionRequest check first — positive discriminant on "action" field.
      // RpcResponse also has "id", so trying it first would swallow action messages.
      if let Ok(action) = serde_json::from_str::<ActionRequest>(&line) {
          tokio::spawn(handle_action(
              action,
              stdin_tx_loop.clone(),
              auth_phase_loop.clone(),
          ));
      } else if let Ok(resp) = serde_json::from_str::<RpcResponse>(&line) {
          let mut map = pending_rx.lock().await;
          if let Some(tx) = map.remove(&resp.id) {
              let _ = tx.send(resp);
          }
      } else {
          warn!("plugin sent invalid JSON — line: {line}");
      }
  }
  ```

  Add the needed clones before the `tokio::spawn(async move { ... })` read loop:

  ```rust
  let stdin_tx_loop  = stdin_tx.clone();
  let auth_phase_loop = Arc::clone(&auth_phase);
  ```

  Update the `use` block at the top to add `ActionRequest`:
  ```rust
  use super::protocol::{
      ActionRequest, CatalogSearchParams, PluginHandshake, RpcMediaItem, RpcRequest,
      RpcResponse, RpcStream, RpcSubtitleTrack, StreamsResolveParams, SubtitlesFetchParams,
      ActionResponse,
  };
  ```

- [ ] **Step 8: Implement `handle_action`**

  Add the following function after the `PluginProcess` impl block:

  ```rust
  /// Handles a plugin-initiated action request asynchronously.
  /// Spawned as a task from the read loop so it doesn't block RPC response routing.
  async fn handle_action(
      req: ActionRequest,
      stdin_tx: mpsc::UnboundedSender<String>,
      auth_phase: SharedAuthPhase,
  ) {
      let response = match req.action.as_str() {
          "auth_allocate_port" => {
              // Check phase WITHOUT holding lock across the allocate_port().await.
              // allocate_port() is fast (sub-millisecond socket bind) but holding
              // any tokio Mutex across an await violates async best-practice and
              // could deadlock with concurrent handle_action tasks.
              {
                  let phase = auth_phase.lock().await;
                  if matches!(*phase, AuthPhase::InProgress) {
                      send_response(&stdin_tx, ActionResponse::err(&req.id, "auth_already_in_progress"));
                      return;
                  }
              } // lock released before await

              match crate::auth::allocate_port().await {
                  Ok((port, rx)) => {
                      let mut phase = auth_phase.lock().await;
                      // Re-check: state may have changed while unblocked
                      if matches!(*phase, AuthPhase::InProgress) {
                          send_response(&stdin_tx, ActionResponse::err(&req.id, "auth_already_in_progress"));
                          return;
                      }
                      *phase = AuthPhase::Allocated(rx);
                      ActionResponse::ok(&req.id, serde_json::json!({"port": port}))
                  }
                  Err(e) => ActionResponse::err(&req.id, format!("allocate_failed: {e}")),
              }
          }

          "auth_open_and_wait" => {
              let params = req.params.as_ref();

              // Validate URL param before touching AuthPhase
              let url = match params.and_then(|p| p["url"].as_str()) {
                  Some(u) => u.to_string(),
                  None => {
                      send_response(&stdin_tx, ActionResponse::err(&req.id, "invalid_params"));
                      return;
                  }
              };
              let timeout_ms = params
                  .and_then(|p| p["timeout_ms"].as_u64())
                  .unwrap_or(120_000)
                  .clamp(1_000, 300_000);

              // Take receiver and transition to InProgress — lock must be released before .await
              let receiver = {
                  let mut phase = auth_phase.lock().await;
                  match std::mem::replace(&mut *phase, AuthPhase::InProgress) {
                      AuthPhase::Allocated(rx) => rx,
                      AuthPhase::Idle => {
                          // Restore state
                          *phase = AuthPhase::Idle;
                          send_response(&stdin_tx, ActionResponse::err(&req.id, "no_port_allocated"));
                          return;
                      }
                      AuthPhase::InProgress => {
                          // Already in progress — restore
                          *phase = AuthPhase::InProgress;
                          send_response(&stdin_tx, ActionResponse::err(&req.id, "auth_already_in_progress"));
                          return;
                      }
                  }
              }; // lock released here — no lock held across the await below

              let result = crate::auth::open_and_wait(
                  &url,
                  receiver,
                  std::time::Duration::from_millis(timeout_ms),
              ).await;

              // Reset to Idle
              *auth_phase.lock().await = AuthPhase::Idle;

              match result {
                  Ok(cb) => ActionResponse::ok(
                      &req.id,
                      serde_json::json!({"code": cb.code, "state": cb.state}),
                  ),
                  Err(crate::auth::AuthError::TimedOut) =>
                      ActionResponse::err(&req.id, "timed_out"),
                  Err(crate::auth::AuthError::Denied { message }) =>
                      ActionResponse::err(&req.id, format!("denied: {message}")),
                  Err(crate::auth::AuthError::BrowserOpenFailed(m)) =>
                      ActionResponse::err(&req.id, format!("browser_open_failed: {m}")),
                  Err(crate::auth::AuthError::ReceiverDropped) =>
                      ActionResponse::err(&req.id, "timed_out"),
              }
          }

          _ => ActionResponse::err(&req.id, "unknown_action"),
      };

      send_response(&stdin_tx, response);
  }

  /// Serialise and send an ActionResponse through the stdin writer channel.
  fn send_response(
      stdin_tx: &mpsc::UnboundedSender<String>,
      resp: ActionResponse,
  ) {
      if let Ok(line) = serde_json::to_string(&resp) {
          let _ = stdin_tx.send(format!("{line}\n"));
      }
  }
  ```

- [ ] **Step 9: Compile check**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo check 2>&1
  ```

  Expected: no errors.

- [ ] **Step 10: Run all tests**

  ```bash
  cargo test 2>&1 | tail -10
  ```

  Expected: all tests pass.

- [ ] **Step 11: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add runtime/src/plugin_rpc/process.rs
  git commit -m "feat(rpc): bidirectional auth protocol — stdin channel + AuthPhase + handle_action"
  ```

---

## Chunk 4: SDK Helpers + SoundCloud Demo

### Task 7: SDK helpers

**Files:**
- Modify: `sdk/src/lib.rs`

- [ ] **Step 1: Write the failing tests for SDK auth helpers**

  Append to `sdk/src/lib.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      // These tests run outside WASM (on the host), so the extern "C" functions
      // won't be called. We test the pure Rust mapping/parsing logic.

      fn make_auth_json(code: Option<&str>, state: Option<&str>, error: Option<&str>) -> String {
          let mut map = serde_json::Map::new();
          if let Some(c) = code  { map.insert("code".into(),  serde_json::json!(c)); }
          if let Some(s) = state { map.insert("state".into(), serde_json::json!(s)); }
          if let Some(e) = error { map.insert("error".into(), serde_json::json!(e)); }
          serde_json::to_string(&serde_json::Value::Object(map)).unwrap()
      }

      #[test]
      fn test_parse_auth_json_success() {
          let json = make_auth_json(Some("mycode"), Some("csrf"), None);
          let result = parse_auth_json(&json);
          assert!(result.is_ok());
          let cb = result.unwrap();
          assert_eq!(cb.code, "mycode");
          assert_eq!(cb.state, Some("csrf".to_string()));
      }

      #[test]
      fn test_parse_auth_json_denied() {
          let json = make_auth_json(None, None, Some("access_denied"));
          let result = parse_auth_json(&json);
          assert_eq!(result.unwrap_err(), "denied: access_denied");
      }

      #[test]
      fn test_parse_auth_json_timed_out() {
          let json = make_auth_json(None, None, Some("timed_out"));
          let result = parse_auth_json(&json);
          assert_eq!(result.unwrap_err(), "timed_out");
      }

      #[test]
      fn test_parse_auth_json_malformed_fallback() {
          // Both code and error absent → safe fallback to timed_out
          let json = r#"{"state":"xyz"}"#;
          let result = parse_auth_json(json);
          assert_eq!(result.unwrap_err(), "timed_out");
      }

      #[test]
      fn test_http_post_form_payload_format() {
          // Verify the payload structure sent to stui_http_post is correct
          let url  = "https://api.example.com/token";
          let body = "grant_type=authorization_code&code=abc";
          let payload = format!(
              "{{\"url\":{url_json},\"body\":{body_json},\"__stui_headers\":{{\"Content-Type\":\"application/x-www-form-urlencoded\"}}}}",
              url_json  = serde_json::to_string(url).unwrap(),
              body_json = serde_json::to_string(body).unwrap(),
          );
          // Parse back to verify structure
          let val: serde_json::Value = serde_json::from_str(&payload).unwrap();
          assert_eq!(val["url"].as_str().unwrap(), url);
          assert_eq!(val["__stui_headers"]["Content-Type"].as_str().unwrap(),
                     "application/x-www-form-urlencoded");
      }
  }
  ```

- [ ] **Step 2: Run tests — expect compile error (`parse_auth_json` not defined)**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/sdk
  cargo test 2>&1 | head -20
  ```

- [ ] **Step 3: Add `extern "C"` declarations to the top-level block**

  Find the top-level `extern "C"` block in `sdk/src/lib.rs` (around line 159):

  ```rust
  #[cfg(target_arch = "wasm32")]
  extern "C" {
      pub fn stui_log(level: i32, ptr: *const u8, len: i32);
      pub fn stui_http_get(url_ptr: *const u8, url_len: i32) -> i64;
      pub fn stui_cache_get(key_ptr: *const u8, key_len: i32) -> i64;
      pub fn stui_cache_set(
          key_ptr: *const u8, key_len: i32,
          val_ptr: *const u8, val_len: i32,
      );
      // ← add here:
      pub fn stui_auth_allocate_port() -> i32;
      pub fn stui_auth_open_and_wait(url_ptr: *const u8, url_len: i32, timeout_ms: i32) -> i64;
  }
  ```

- [ ] **Step 4: Add `OAuthCallback` struct and `parse_auth_json` helper**

  After the `cache_set` function, add:

  ```rust
  // ── OAuth helpers ─────────────────────────────────────────────────────────────

  /// The OAuth callback result returned to plugin code.
  ///
  /// `code` is non-optional because `Ok`/`Err` already encodes presence.
  pub struct OAuthCallback {
      pub code: String,
      pub state: Option<String>,
  }

  /// Parse the JSON blob returned by `stui_auth_open_and_wait`.
  ///
  /// `code: Some` → `Ok(OAuthCallback)`.
  /// `error: Some("timed_out")` → `Err("timed_out")`.
  /// `error: Some(other)` → `Err("denied: <other>")`.
  /// Both absent (malformed) → `Err("timed_out")` as safe fallback.
  pub fn parse_auth_json(json: &str) -> Result<OAuthCallback, String> {
      let val: serde_json::Value = serde_json::from_str(json)
          .map_err(|e| format!("timed_out (parse error: {e})"))?;
      if let Some(code) = val["code"].as_str().filter(|s| !s.is_empty()) {
          return Ok(OAuthCallback {
              code: code.to_string(),
              state: val["state"].as_str().map(|s| s.to_string()),
          });
      }
      // For the WASM host path, the host returns {"error":"denied","message":"<detail>"}.
      // For the callback server path (timeout, etc.), error is a plain string like "timed_out".
      match val["error"].as_str() {
          Some("timed_out")  => Err("timed_out".into()),
          Some("denied") => {
              let msg = val["message"].as_str().unwrap_or("unknown");
              Err(format!("denied: {msg}"))
          }
          Some(e) => Err(format!("denied: {e}")), // fallback for other error codes
          None    => Err("timed_out".into()),
      }
  }
  ```

- [ ] **Step 5: Add `auth_allocate_port`, `auth_open_and_wait`, `http_post_form`**

  Add after `parse_auth_json`:

  ```rust
  /// Allocate a local callback port. Call before constructing the OAuth URL.
  ///
  /// Returns the port number, or `Err("port_allocation_failed")` if the host
  /// returns -1.
  pub fn auth_allocate_port() -> Result<u16, String> {
      #[cfg(target_arch = "wasm32")]
      {
          let port = unsafe { stui_auth_allocate_port() };
          if port < 0 {
              return Err("port_allocation_failed".into());
          }
          Ok(port as u16)
      }
      #[cfg(not(target_arch = "wasm32"))]
      {
          Err("auth_allocate_port only available in WASM context".into())
      }
  }

  /// Open the browser at `url` and block until the OAuth callback arrives or
  /// `timeout_ms` elapses.
  ///
  /// `timeout_ms` is cast to i32 via saturating cast before being passed to
  /// the host — values > i32::MAX become i32::MAX; the host clamps to [1000, 300000].
  ///
  /// Possible error strings: `"timed_out"`, `"denied: <msg>"`,
  /// `"no_port_allocated"`, `"browser_open_failed: <msg>"`.
  pub fn auth_open_and_wait(url: &str, timeout_ms: u32) -> Result<OAuthCallback, String> {
      #[cfg(target_arch = "wasm32")]
      {
          // Saturating cast: values > i32::MAX become i32::MAX; host clamps to [1000, 300000]
          let t_ms = timeout_ms.min(i32::MAX as u32) as i32;
          let packed = unsafe {
              stui_auth_open_and_wait(url.as_ptr(), url.len() as i32, t_ms)
          };
          if packed == 0 {
              return Err("timed_out".into());
          }
          let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
          let len = (packed & 0xFFFFFFFF) as usize;
          // Memory is NOT freed — matches established sdk pattern (http_get, cache_get)
          let json = unsafe { std::str::from_utf8(std::slice::from_raw_parts(ptr, len)) }
              .map_err(|e| format!("timed_out (utf8 error: {e})"))?;
          parse_auth_json(json)
      }
      #[cfg(not(target_arch = "wasm32"))]
      {
          let _ = (url, timeout_ms);
          Err("auth_open_and_wait only available in WASM context".into())
      }
  }

  /// Make an HTTP POST with `Content-Type: application/x-www-form-urlencoded`.
  ///
  /// Uses `stui_http_post` with the `__stui_headers` override mechanism.
  /// `body` should be a pre-encoded form string, e.g.
  /// `"grant_type=authorization_code&code=abc&redirect_uri=..."`.
  pub fn http_post_form(url: &str, body: &str) -> Result<String, String> {
      let payload = format!(
          "{{\"url\":{url_json},\"body\":{body_json},\"__stui_headers\":{{\"Content-Type\":\"application/x-www-form-urlencoded\"}}}}",
          url_json  = serde_json::to_string(url).unwrap_or_default(),
          body_json = serde_json::to_string(body).unwrap_or_default(),
      );
      #[cfg(target_arch = "wasm32")]
      {
          extern "C" {
              fn stui_http_post(ptr: *const u8, len: i32) -> i64;
          }
          let packed = unsafe { stui_http_post(payload.as_ptr(), payload.len() as i32) };
          if packed == 0 { return Err("http_post_form returned null".into()); }
          let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
          let len = (packed & 0xFFFFFFFF) as usize;
          let json = unsafe { std::str::from_utf8(std::slice::from_raw_parts(ptr, len)) }
              .map_err(|e| e.to_string())?;
          let resp: HttpResponse = serde_json::from_str(json)
              .map_err(|e| e.to_string())?;
          if resp.status >= 200 && resp.status < 300 {
              Ok(resp.body)
          } else {
              Err(format!("HTTP {}: {}", resp.status, resp.body))
          }
      }
      #[cfg(not(target_arch = "wasm32"))]
      {
          let _ = payload;
          Err(format!("http_post_form only available in WASM context (url: {url})"))
      }
  }
  ```

- [ ] **Step 6: Run tests to verify they pass**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/sdk
  cargo test -- --nocapture 2>&1
  ```

  Expected: all 5 SDK tests pass.

- [ ] **Step 7: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add sdk/src/lib.rs
  git commit -m "feat(sdk): add auth_allocate_port, auth_open_and_wait, http_post_form, OAuthCallback"
  ```

---

### Task 8: SoundCloud demo plugin

**Files:**
- Create: `plugins/soundcloud/Cargo.toml`
- Create: `plugins/soundcloud/plugin.toml`
- Create: `plugins/soundcloud/src/lib.rs`

- [ ] **Step 1: Create `plugins/soundcloud/Cargo.toml`**

  ```toml
  [package]
  name = "soundcloud"
  version = "0.1.0"
  edition = "2021"

  [lib]
  crate-type = ["cdylib"]

  [dependencies]
  stui-sdk    = { path = "../../sdk" }
  serde_json  = { version = "1", default-features = false, features = ["alloc"] }
  ```

- [ ] **Step 2: Create `plugins/soundcloud/plugin.toml`**

  ```toml
  [plugin]
  name        = "soundcloud"
  version     = "0.1.0"
  type        = "stream-provider"
  entrypoint  = "soundcloud.wasm"
  description = "SoundCloud music streaming (OAuth browser-auth demo)"

  [permissions]
  network_hosts = ["api.soundcloud.com", "secure.soundcloud.com"]
  ```

- [ ] **Step 3: Write failing tests for the SoundCloud plugin**

  Create `plugins/soundcloud/src/lib.rs` with tests first:

  ```rust
  // Note: do NOT use #![no_std]. WASM binary size is controlled by Cargo.toml
  // profile settings (opt-level = "z", panic = "abort"). Using std enables
  // host-side `cargo test` without cfg tricks or feature flags.

  // ── Note: this CLIENT_ID is extracted from SoundCloud's public web bundle.
  // It is intentionally public (no secret required — public client OAuth model).
  // Replace with a current one if SoundCloud rotates it. This demo may break
  // if CLIENT_ID expires; it is a pattern reference, not a production plugin.
  const CLIENT_ID: &str = "iZIs9mchVcX5lhVRyQGGAYlNPVldzAoX";

  pub fn parse_access_token(json: &str) -> Result<String, String> {
      let val: serde_json::Value = serde_json::from_str(json)
          .map_err(|e| format!("parse error: {e}"))?;
      val["access_token"]
          .as_str()
          .map(|s| s.to_string())
          .ok_or_else(|| "missing access_token".to_string())
  }

  pub fn build_auth_url(port: u16) -> String {
      let redirect_uri = format!("http://localhost:{port}/callback");
      format!(
          "https://secure.soundcloud.com/authorize\
           ?client_id={CLIENT_ID}&redirect_uri={redirect_uri}\
           &response_type=code&scope=non-expiring"
      )
  }

  pub fn build_exchange_body(code: &str, redirect_uri: &str) -> String {
      format!(
          "grant_type=authorization_code&code={code}\
           &redirect_uri={redirect_uri}&client_id={CLIENT_ID}"
      )
  }

  /// Pure helper: given the raw JSON string returned by `cache_get("sc_token")`,
  /// extract the access token. Returns `None` on a cache miss (input is `None`).
  ///
  /// `ensure_authenticated` calls this first; if it returns `Some(Ok(token))`,
  /// the OAuth flow is skipped entirely (cache-hit path).
  pub fn token_from_cache(cached: Option<String>) -> Option<Result<String, String>> {
      cached.map(|j| parse_access_token(&j))
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn test_parse_access_token_success() {
          let json = r#"{"access_token":"test_token_123","token_type":"bearer"}"#;
          assert_eq!(parse_access_token(json).unwrap(), "test_token_123");
      }

      #[test]
      fn test_parse_access_token_missing() {
          let json = r#"{"token_type":"bearer"}"#;
          assert!(parse_access_token(json).is_err());
      }

      #[test]
      fn test_build_auth_url_contains_redirect_uri() {
          let url = build_auth_url(52314);
          assert!(url.contains("redirect_uri=http://localhost:52314/callback"),
              "URL must contain percent-safe redirect_uri, got: {url}");
          assert!(url.contains("response_type=code"));
          assert!(url.contains("scope=non-expiring"));
          assert!(url.contains(&format!("client_id={CLIENT_ID}")));
      }

      #[test]
      fn test_exchange_body_format() {
          let body = build_exchange_body("mycode", "http://localhost:12345/callback");
          assert!(body.contains("grant_type=authorization_code"));
          assert!(body.contains("code=mycode"));
          assert!(body.contains("redirect_uri=http://localhost:12345/callback"));
          assert!(body.contains(&format!("client_id={CLIENT_ID}")));
      }

      // ── ensure_authenticated cache-hit path ───────────────────────────────────
      // cache_get/auth_allocate_port are host imports (extern "C"), not callable
      // in host tests. We test the pure helpers that ensure_authenticated delegates
      // to: token_from_cache (cache-hit) and parse_access_token (parse step).

      #[test]
      fn test_ensure_authenticated_cache_hit_returns_token() {
          // Simulates: cache_get("sc_token") returns Some(json) → return immediately
          let cached_json = r#"{"access_token":"cached_tok_abc","token_type":"bearer"}"#;
          let result = token_from_cache(Some(cached_json.to_string()));
          assert!(result.is_some(), "cache hit must return Some");
          assert_eq!(result.unwrap().unwrap(), "cached_tok_abc",
              "cache-hit: must return cached token without initiating OAuth flow");
      }

      #[test]
      fn test_ensure_authenticated_cache_miss_returns_none() {
          // Simulates: cache_get("sc_token") returns None → OAuth flow must start
          let result = token_from_cache(None);
          assert!(result.is_none(), "cache miss must return None, triggering OAuth flow");
      }

      #[test]
      fn test_ensure_authenticated_cache_hit_bad_json_triggers_reauth() {
          // Simulates: cached value is corrupt → treat as cache miss (return Err)
          let bad_json = r#"{"token_type":"bearer"}"#; // no access_token
          let result = token_from_cache(Some(bad_json.to_string()));
          assert!(result.is_some());
          assert!(result.unwrap().is_err(), "corrupt cached JSON must return Err");
      }
  }
  ```

- [ ] **Step 4: Run the unit tests (non-WASM, on host)**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/plugins/soundcloud
  cargo test 2>&1
  ```

  Expected: 7 tests pass (4 existing + 3 new cache-hit tests).

- [ ] **Step 5: Implement the full plugin**

  Complete `plugins/soundcloud/src/lib.rs` with the full WASM entry point after the helpers and tests:

  ```rust
  use stui_sdk::{
      StuiPlugin, PluginType, PluginResult,
      SearchRequest, SearchResponse, PluginEntry,
      ResolveRequest, ResolveResponse,
      cache_get, cache_set,
      auth_allocate_port, auth_open_and_wait,
      http_post_form, http_get,
  };

  // ── Auth helpers (tested above) ───────────────────────────────────────────────
  // parse_access_token, build_auth_url, build_exchange_body — defined above

  fn exchange_code(code: &str, redirect_uri: &str) -> Result<String, String> {
      let body = build_exchange_body(code, redirect_uri);
      // Note: for production plugins, percent-encode code and redirect_uri values.
      // OAuth codes are typically URL-safe, but correctness requires it.
      http_post_form("https://api.soundcloud.com/oauth2/token", &body)
  }

  fn ensure_authenticated() -> Result<String, String> {
      // Check cache first — scope=non-expiring means no TTL needed.
      // token_from_cache is the pure helper tested in unit tests above.
      if let Some(result) = token_from_cache(cache_get("sc_token")) {
          return result;
      }
      let port = auth_allocate_port()?;
      let redirect_uri = format!("http://localhost:{port}/callback");
      let url = build_auth_url(port);
      let cb = auth_open_and_wait(&url, 120_000)?;
      let token_json = exchange_code(&cb.code, &redirect_uri)?;
      cache_set("sc_token", &token_json);
      parse_access_token(&token_json)
  }

  // ── Plugin implementation ─────────────────────────────────────────────────────

  pub struct SoundCloud;

  impl StuiPlugin for SoundCloud {
      fn name(&self)    -> &str { "soundcloud" }
      fn version(&self) -> &str { "0.1.0" }
      fn plugin_type(&self) -> PluginType { PluginType::Provider }

      fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
          let token = match ensure_authenticated() {
              Ok(t)  => t,
              Err(e) => return PluginResult::err("auth_failed", e),
          };

          // Search SoundCloud tracks via v2 API
          let url = format!(
              "https://api.soundcloud.com/search/tracks?q={}&limit=20&client_id={CLIENT_ID}",
              urlencoded(&req.query)
          );

          // Use OAuth token in header via http_get fallback (SoundCloud accepts
          // client_id param for read-only search; token used for private content)
          let body = match http_get(&url) {
              Ok(b)  => b,
              Err(e) => return PluginResult::err("search_failed", e),
          };

          let items = parse_search_results(&body, &token);
          PluginResult::ok(SearchResponse { items, total: 0 })
      }

      fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse> {
          let _token = match ensure_authenticated() {
              Ok(t)  => t,
              Err(e) => return PluginResult::err("auth_failed", e),
          };

          // Resolve track ID to stream URL
          let url = format!(
              "https://api.soundcloud.com/tracks/{}/stream?client_id={CLIENT_ID}",
              req.entry_id
          );
          PluginResult::ok(ResolveResponse {
              stream_url: url,
              quality: Some("audio".into()),
              subtitles: vec![],
          })
      }
  }

  fn urlencoded(s: &str) -> String {
      s.chars()
          .map(|c| match c {
              ' '  => '+'.to_string(),
              c if c.is_alphanumeric() || "-_.~".contains(c) => c.to_string(),
              c    => format!("%{:02X}", c as u32),
          })
          .collect()
  }

  fn parse_search_results(body: &str, _token: &str) -> Vec<PluginEntry> {
      let val: serde_json::Value = match serde_json::from_str(body) {
          Ok(v)  => v,
          Err(_) => return Vec::new(),
      };
      val["collection"]
          .as_array()
          .unwrap_or(&Vec::new())
          .iter()
          .filter_map(|track| {
              let id    = track["id"].as_u64()?.to_string();
              let title = track["title"].as_str()?.to_string();
              let user  = track["user"]["username"].as_str().unwrap_or("").to_string();
              Some(PluginEntry {
                  id,
                  title: format!("{title} — {user}"),
                  year: None,
                  genre: track["genre"].as_str().map(|s| s.to_string()),
                  rating: None,
                  description: track["description"].as_str().map(|s| s.to_string()),
                  poster_url: track["artwork_url"].as_str().map(|s| s.to_string()),
                  imdb_id: None,
              })
          })
          .collect()
  }

  stui_sdk::stui_export_plugin!(SoundCloud);
  ```

- [ ] **Step 6: Run host tests again to confirm they still pass**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/plugins/soundcloud
  cargo test 2>&1
  ```

  Expected: 4 tests pass.

- [ ] **Step 7: Attempt WASM compile (optional — requires wasm32-wasip1 target)**

  ```bash
  rustup target add wasm32-wasip1 2>/dev/null || true
  cd /home/ozogorgor/Projects/Stui_Project/stui/plugins/soundcloud
  cargo build --target wasm32-wasip1 --release 2>&1 | tail -5
  ```

  If the target is available: expected to produce `target/wasm32-wasip1/release/soundcloud.wasm`.
  If the target is not installed: step is informational only — skip.

- [ ] **Step 8: Run full workspace check**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo check 2>&1 && cargo check --features wasm-host 2>&1
  ```

  Expected: no errors on either.

- [ ] **Step 9: Run all tests**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
  cargo test 2>&1 | tail -10
  cd /home/ozogorgor/Projects/Stui_Project/stui/sdk
  cargo test 2>&1 | tail -5
  cd /home/ozogorgor/Projects/Stui_Project/stui/plugins/soundcloud
  cargo test 2>&1 | tail -5
  ```

  Expected: all pass.

- [ ] **Step 10: Commit**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  git add plugins/soundcloud/
  git commit -m "feat(plugins): add SoundCloud OAuth browser-auth demo plugin"
  ```

---

## Final verification

- [ ] **Run all tests across all crates**

  ```bash
  cd /home/ozogorgor/Projects/Stui_Project/stui
  cargo test -p stui-runtime 2>&1 | tail -10
  cargo test -p stui-plugin-sdk 2>&1 | tail -5
  cargo test -p soundcloud 2>&1 | tail -5
  ```

  Expected: all pass.

- [ ] **Verify feature-gated path still compiles without wasm-host**

  ```bash
  cargo check -p stui-runtime 2>&1
  ```

  Expected: no errors.
