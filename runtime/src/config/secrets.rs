//! Secure secrets management for stui.
//!
//! Secrets (API keys, passwords) are loaded from the following sources
//! in order of priority (first non-empty value wins):
//!
//! 1. Environment variables (highest priority)
//! 2. `~/.stui/secrets.env` file (`.env` format)
//!
//! # File Format (`secrets.env`)
//!
//! ```bash
//! TMDB_API_KEY=your_tmdb_key_here
//! OMDB_API_KEY=your_omdb_key_here
//! MPD_PASSWORD=your_mpd_password_here
//! ```
//!
//! # Security Notes
//!
//! - The secrets file should have restricted permissions: `chmod 600 secrets.env`
//! - Secrets are redacted from logs and config exports
//! - Never commit `secrets.env` to version control

#![allow(dead_code)]

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{debug, warn};

static SECRETS: OnceLock<Secrets> = OnceLock::new();

#[derive(Clone, Default)]
pub struct Secrets {
    values: HashMap<String, String>,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let known = self.count_known_vars();
        let total = self.values.len();
        f.debug_struct("Secrets")
            .field("known_keys_present", &known)
            .field("total_keys", &total)
            .finish()
    }
}

impl Secrets {
    pub fn load() -> Self {
        SECRETS
            .get_or_init(|| {
                let mut secrets = Secrets::default();
                secrets.load_from_file();
                secrets.load_from_env();
                debug!(
                    "secrets loaded: {} known keys present, {} total keys",
                    secrets.count_known_vars(),
                    secrets.values.len()
                );
                secrets
            })
            .clone()
    }

    fn load_from_file(&mut self) {
        let path = secrets_file_path();
        if let Some(p) = &path {
            if p.exists() {
                match load_env_file(p) {
                    Ok(vars) => {
                        for (k, v) in vars {
                            self.values.insert(k, v);
                        }
                        debug!("loaded {} secrets from {}", self.values.len(), p.display());
                    }
                    Err(e) => {
                        warn!("failed to load secrets from {}: {e}", p.display());
                    }
                }
            } else {
                debug!("no secrets file at {} — using env vars only", p.display());
            }
        }
    }

    fn load_from_env(&mut self) {
        for var in KNOWN_SECRET_VARS {
            if let Ok(val) = env::var(var) {
                if !val.is_empty() {
                    self.values.insert(var.to_string(), val);
                }
            }
        }
    }

    /// Returns the number of known secret keys present in the loaded values,
    /// regardless of whether they were loaded from a file or the environment.
    fn count_known_vars(&self) -> usize {
        self.values
            .keys()
            .filter(|k| KNOWN_SECRET_VARS.contains(&k.as_str()))
            .count()
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    pub fn tmdb_api_key(&self) -> Option<String> {
        self.get("TMDB_API_KEY").filter(|k| !k.is_empty())
    }

    pub fn omdb_api_key(&self) -> Option<String> {
        self.get("OMDB_API_KEY").filter(|k| !k.is_empty())
    }

    pub fn mpd_password(&self) -> Option<String> {
        self.get("MPD_PASSWORD").filter(|k| !k.is_empty())
    }

    pub fn prowlarr_api_key(&self) -> Option<String> {
        self.get("PROWLARR_API_KEY").filter(|k| !k.is_empty())
    }

    pub fn opensubtitles_api_key(&self) -> Option<String> {
        self.get("OPENSUBTITLES_API_KEY").filter(|k| !k.is_empty())
    }

    pub fn torrentio_api_key(&self) -> Option<String> {
        self.get("TORRENTIO_API_KEY").filter(|k| !k.is_empty())
    }

    pub fn lastfm_api_key(&self) -> Option<String> {
        self.get("LASTFM_API_KEY").filter(|k| !k.is_empty())
    }

    pub fn mdblist_api_key(&self) -> Option<String> {
        self.get("MDBLIST_API_KEY").filter(|k| !k.is_empty())
    }
}

const KNOWN_SECRET_VARS: &[&str] = &[
    "TMDB_API_KEY",
    "OMDB_API_KEY",
    "MPD_PASSWORD",
    "PROWLARR_API_KEY",
    "JACKETT_API_KEY",
    "OPENSUBTITLES_API_KEY",
    "TORRENTIO_API_KEY",
    "LASTFM_API_KEY",
    "DISCOGS_API_KEY",
    "MDBLIST_API_KEY",
    "FANART_PROJECT_KEY",
    "FANART_USER_KEY",
];

fn secrets_file_path() -> Option<PathBuf> {
    // Secrets co-located with config under ~/.config/stui/. Same
    // ownership/permissions semantics as before — only the parent
    // directory moved (XDG-compliant instead of legacy ~/.stui/).
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .map(|c| c.join("stui").join("secrets.env"))
}

fn load_env_file(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let content = fs::read_to_string(path)?;
    parse_env_content(&content)
}

fn parse_env_content(content: &str) -> anyhow::Result<HashMap<String, String>> {
    let mut vars = HashMap::new();

    for line in content.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let mut value = value.trim().to_string();

            if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                if value.len() >= 2 {
                    value = value[1..value.len() - 1].to_string();
                } else {
                    value = String::new();
                }
            }

            // Transparent decryption: values stored as `enc:<base64>` are
            // machine-id-bound AES-256-GCM ciphertexts; decrypt them on
            // load so plugins see the same plaintext they would have seen
            // from a bare `KEY=value` line. Decryption failures (tampered,
            // wrong machine, malformed) drop the entry with a warning
            // rather than crashing — the plugin then surfaces a
            // missing-key error instead of a silent stale value.
            if let Some(enc_payload) = value.strip_prefix("enc:") {
                match super::secrets_enc::decrypt(enc_payload) {
                    Ok(pt) => value = pt,
                    Err(e) => {
                        tracing::warn!(
                            key = %key,
                            error = %e,
                            "secrets.env: failed to decrypt enc-prefixed value, dropping entry"
                        );
                        continue;
                    }
                }
            }

            if !key.is_empty() && !value.is_empty() {
                vars.insert(key.to_string(), value);
            }
        }
    }

    Ok(vars)
}

/// Env-var lookup that consults stui's loaded secrets *then* the process env.
///
/// Plugins may reference secrets (TMDB_API_KEY, OMDB_API_KEY, …) that live only
/// in `~/.stui/secrets.env`. They are never exported into the daemon process
/// env, so a bare `std::env::var` lookup misses them. Any call site that
/// resolves plugin config fields or builds the plugin's WASI env should route
/// through here.
pub fn env_lookup(key: &str) -> Option<String> {
    Secrets::load().get(key).or_else(|| env::var(key).ok())
}

pub fn redact(value: &str) -> String {
    let char_count = value.chars().count();
    if char_count <= 4 {
        return "****".to_string();
    }
    let visible = 4.min(char_count / 4);
    let masked = "*".repeat(char_count - visible);
    let prefix: String = value.chars().take(visible).collect();
    format!("{}{}", prefix, masked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env_content() {
        let content = r#"
# This is a comment
TMDB_API_KEY=secret123
OMDB_API_KEY="quoted_value"
MPD_PASSWORD='single_quoted'
EMPTY_KEY=
"#;
        let vars = parse_env_content(content).unwrap();
        assert_eq!(vars.get("TMDB_API_KEY"), Some(&"secret123".to_string()));
        assert_eq!(vars.get("OMDB_API_KEY"), Some(&"quoted_value".to_string()));
        assert_eq!(vars.get("MPD_PASSWORD"), Some(&"single_quoted".to_string()));
        assert!(!vars.contains_key("EMPTY_KEY"));
    }

    #[test]
    fn test_parse_env_single_quoted_char() {
        // Empty quoted values are filtered out (correct security behavior)
        let vars = parse_env_content(r#"KEY=""#).unwrap();
        assert!(vars.get("KEY").is_none());

        let vars = parse_env_content(r#"KEY=''"#).unwrap();
        assert!(vars.get("KEY").is_none());

        // Single character non-empty quoted value is kept
        let vars = parse_env_content(r#"KEY="x""#).unwrap();
        assert_eq!(vars.get("KEY"), Some(&"x".to_string()));
    }

    #[test]
    fn test_redact() {
        assert_eq!(redact("short"), "s****");
        assert_eq!(redact("abcdefghij"), "ab********");
        assert_eq!(redact("1234567890"), "12********");
    }

    /// Encrypted-prefix round-trip: a value written via `secrets_enc::encrypt`
    /// and prefixed with `enc:` must come out of `parse_env_content` as
    /// the original plaintext.
    #[test]
    fn test_parse_env_decrypts_enc_prefix() {
        static MU: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = MU.lock().unwrap();
        env::set_var("STUI_MACHINE_ID", "deadbeef-test-machine-id");
        let ciphertext = super::super::secrets_enc::encrypt("real-tmdb-key").unwrap();
        let content = format!("TMDB_API_KEY=enc:{}\nOTHER_KEY=plain\n", ciphertext);
        let vars = parse_env_content(&content).unwrap();
        env::remove_var("STUI_MACHINE_ID");
        assert_eq!(vars.get("TMDB_API_KEY"), Some(&"real-tmdb-key".to_string()));
        // Bare values still pass through unchanged.
        assert_eq!(vars.get("OTHER_KEY"), Some(&"plain".to_string()));
    }

    /// Tampered enc-prefixed values are dropped (warned), not crashed-on.
    #[test]
    fn test_parse_env_drops_corrupt_enc_value() {
        static MU: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = MU.lock().unwrap();
        env::set_var("STUI_MACHINE_ID", "deadbeef-test-machine-id");
        // Garbage payload after enc: — decrypt fails.
        let content = "TMDB_API_KEY=enc:not-real-base64-or-cipher\nOTHER=plain\n";
        let vars = parse_env_content(content).unwrap();
        env::remove_var("STUI_MACHINE_ID");
        assert!(
            !vars.contains_key("TMDB_API_KEY"),
            "corrupt enc value should be dropped"
        );
        assert_eq!(vars.get("OTHER"), Some(&"plain".to_string()));
    }

    #[test]
    fn test_secrets_load_order() {
        use std::sync::RwLock;
        static ENV_MUTEX: RwLock<()> = RwLock::new(());

        let _guard = ENV_MUTEX.write().unwrap();
        env::set_var("TMDB_API_KEY", "from_env");
        let mut secrets = Secrets::default();
        secrets.load_from_env();
        assert_eq!(secrets.get("TMDB_API_KEY"), Some("from_env".to_string()));
        env::remove_var("TMDB_API_KEY");
    }
}
