//! TVDB v4 integration — runtime-level metadata provider.
//!
//! Not a plugin: TVDB ships as part of the runtime binary so there is always
//! an always-on fallback source even when every user plugin is disabled or
//! broken. Works hand-in-hand with the plugin fan-out:
//!   - `client::TvdbClient::search` is dispatched in parallel with plugins
//!     in `engine::search_catalog_entries`. Results flow through the same
//!     dedup/merge pass (by imdb_id / title+year), so duplicate entries
//!     collapse and fields are unioned across sources.
//!   - The `enrich`/`credits`/`artwork`/`episodes` verbs are served by the
//!     `tvdb::source` adapters and dispatched alongside plugins by the
//!     metadata orchestrator — same fan-out, same merge semantics.
//!
//! # Key management
//!
//! TVDB issues API keys to projects, not end users — the key is shipped
//! with the binary. `build.rs` XOR-obfuscates the key at compile time and
//! writes it to `$OUT_DIR/tvdb_embed.bin`; we reverse the XOR at startup.
//! The obfuscation only defeats naive `strings` extraction — a determined
//! attacker with a disassembler trivially recovers both salt and key.
//!
//! **Runtime source:** setting `TVDB_API_KEY` in `~/.config/stui/secrets.env`
//! (or the process environment as a fallback) when
//! launching `stui-runtime` replaces the embedded key. Useful for dev
//! testing against a different TVDB account without rebuilding.
//!
//! For USER-provided keys (tmdb, omdb, last.fm, …) the machine-ID AES-GCM
//! helper in `config::secrets_enc` is the right tool. TVDB is a separate
//! case because the key belongs to the project, not the user.

pub mod client;
pub mod http;
pub mod source;
pub mod types;

pub use client::{SearchKind, TvdbClient, TvdbEntry, TvdbEpisode};

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

/// Embedded obfuscated key bytes (possibly empty if built without a key).
const TVDB_OBFUSCATED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tvdb_embed.bin"));

/// **MUST stay byte-identical to `XOR_SALT` in `runtime/build.rs`.** The
/// build script obfuscates the plaintext key with that salt; mismatched
/// bytes here silently return garbage (the utf-8 check then rejects, and
/// `embedded_client` logs "no key available"). There is no compile-time
/// cross-check — if you edit one constant, grep for the other.
const XOR_SALT: &[u8] = b"stui-tvdb-embed-v1-obfuscation-salt-f3a9";

fn deobfuscate_embedded() -> Option<String> {
    if TVDB_OBFUSCATED.is_empty() {
        return None;
    }
    let plaintext: Vec<u8> = TVDB_OBFUSCATED
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ XOR_SALT[i % XOR_SALT.len()])
        .collect();
    let s = String::from_utf8(plaintext).ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Build a TVDB client using, in order of preference:
///   1. `TVDB_API_KEY` from `~/.config/stui/secrets.env` or the
///      process env (resolved via `config::secrets::env_lookup`,
///      which already encodes the secrets-then-env fallback chain
///      that plugins use)
///   2. The build-time embedded (XOR-obfuscated) key
///
/// Returns `Ok(None)` — NOT an error — when neither source yields a key.
/// That means TVDB is simply inactive for this process; plugins still work.
pub fn embedded_client() -> Result<Option<Arc<TvdbClient>>> {
    if let Some(k) = crate::config::secrets::env_lookup("TVDB_API_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            info!("tvdb: using TVDB_API_KEY from secrets.env / process env");
            return Ok(Some(TvdbClient::new(k)?));
        }
    }
    match deobfuscate_embedded() {
        Some(key) => {
            info!("tvdb: using embedded project key");
            Ok(Some(TvdbClient::new(key)?))
        }
        None => {
            warn!(
                "tvdb: no key available (neither env nor embed) — tvdb disabled \
                 (rebuild with TVDB_API_KEY set, or drop a plaintext key into \
                 runtime/.tvdb-key)"
            );
            Ok(None)
        }
    }
}
