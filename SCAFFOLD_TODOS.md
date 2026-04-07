# Scaffold TODOs

Incomplete features that are implemented but not yet wired up.
Each section links to the relevant code and describes what remains.

---

## 1. Catalog Aggregator — multi-provider merge ✅ DONE

**Status:** Wired. `engine::search()` now calls `CatalogAggregator::merge()` on all
provider results before caching and returning them. Sort and genre/rating/year filters
are applied per-request after cache retrieval.

**Changes made:**
- `runtime/src/engine/mod.rs` — added `SearchOptions`, `apply_search_options()`,
  `catalog_entries_to_media()`; wired `CatalogAggregator::merge()` into fan-out path
  and applied filter/sort in both live and cache-hit paths.
- `runtime/src/pipeline/search.rs` — builds `SearchOptions` from `SearchRequest` fields.
- `runtime/src/ipc/v1/mod.rs` — added `sort`, `genre`, `min_rating`, `year_from`,
  `year_to` fields to `SearchRequest`.
- `tui/internal/ipc/requests.go` — `Search()` accepts optional `SearchOptions`.
- `tui/internal/ipc/types.go` — added `SearchOptions` struct.

---

## 2. Sandbox / Permission Enforcement ✅ DONE

**Status:** Wired. `SandboxCtx::check(Capability::Network)` is now called as a coarse
gate in both `stui_http_get` and `stui_http_post` before the fine-grained
`Permissions::allows_host()` check. WASI preopens are built from
`ctx.allowed_fs_roots()` so a plugin with an empty `filesystem` list gets no
filesystem access at the OS level.

**Changes made:**
- `runtime/src/sandbox.rs` — removed all `#[allow(dead_code)]`; updated
  `check(Capability::Network)` to also pass when `network_hosts` is non-empty (consistent
  with `allows_host()`); made `allowed_fs_roots()` public; added unit tests for the three
  permission cases (denied, allowed by flag, allowed by host allowlist).
- `runtime/src/abi/host.rs` — added `DirPerms`/`FilePerms` imports; WASI setup now
  iterates `ctx.allowed_fs_roots()` and adds a preopen per allowed directory; both HTTP
  host functions call `ctx.check(Capability::Network)` before `allows_host()`.

---

## 3. Roon RAAT Integration — ✅ DONE (Extension API layer; RAAT TCP transport pending)

**Status:** Roon Extension API integration is complete. The redundant `plugins/roon/`
was deleted. `roon.rs` has a working `RoonClient` with proper extension registration,
request/response correlation via oneshot channels, and a correctly wired shutdown channel.
`RaatProcessor::discover_endpoints()` uses `RoonClient::discover()` (real mDNS).
`RaatProcessor::connect()` calls `RoonClient::connect()` (real WebSocket handshake).
`RoonOutput` (no-op `AudioOutput`) is wired into `open_output()` for `OutputTarget::RoonRaat`.
`Pipeline` holds `roon: Option<Arc<RoonClient>>`.

**What was done:**
- Deleted `plugins/roon/` (called non-existent REST endpoints; superseded by runtime integration)
- Fixed `roon.rs`: correct Roon Extension API handshake (`com.roonlabs.registry:1/register`),
  request correlation (`pending: Arc<Mutex<HashMap<u32, oneshot::Sender<Value>>>>`),
  write task shutdown via `mpsc::Sender<()>` + `tokio::select!`, removed unused `write_tx`
- `dsp/raat.rs`: `RaatProcessor` now holds `Option<Arc<RoonClient>>`; `new()` accepts it;
  `discover_endpoints()` maps `RoonServer → RaatEndpoint`; `connect()` calls `client.connect()`
- `dsp/output/roon.rs`: `RoonOutput` implements `AudioOutput` as a no-op (drops audio)
- `dsp/output/mod.rs`: `OutputTarget::RoonRaat` returns `Box::new(RoonOutput::new(...))`
- `engine/pipeline.rs`: `Pipeline.roon: Option<Arc<RoonClient>>`

**What remains (RAAT TCP transport):**
- Implement actual RAAT TCP audio framing (`send_audio()` currently drops all samples).
- This requires reverse-engineering or an official RAAT SDK — blocked on Roon licensing.

---

## 4. DSD Format Detection — hardcoded DSD64 assumption

**Status:** `DsdConverter` can convert DSD to PCM, but the input rate is always
assumed to be DSD64 (2.8 MHz) regardless of the actual source.

**Files:**
- `runtime/src/dsp/dsd.rs` — `DsdFormat` enum, `DsdConverter::infer_dsd_rate()`

**What remains:**
- Parse SACD ISO, DSF, or DFF file headers to detect the actual DSD rate.
- Pass the detected `DsdFormat` variant into `DsdConverter` so it uses the correct
  input rate for decimation.
- Replace the sigma-delta stub (`simple decimation`) with a proper SDM demodulation
  algorithm for acceptable audio quality.

---

## 5. DSP Upsampling — config exists, node not connected

**Status:** `UpsampleRatio` enum and `UpsampleRatio::value()` are defined in
`DspConfig`. The resample node (`Resampler`) exists but ignores the ratio setting.

**Files:**
- `runtime/src/dsp/config.rs` — `UpsampleRatio`
- `runtime/src/dsp/resample.rs` — `Resampler`
- `runtime/src/dsp/mod.rs` — DSP pipeline

**What remains:**
- In `DspPipeline`, when `DspConfig::upsample_ratio != Ratio1x`, create a
  `Resampler` node with `output_rate = input_rate * ratio.value()`.
- Expose the setting in the audio settings TUI panel.
- Add a quality note: upsampling above 4x has diminishing returns and may
  increase CPU usage significantly.

---
