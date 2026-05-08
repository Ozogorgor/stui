//! Machine-ID-derived AES-256-GCM secrets encryption.
//!
//! Stops casual grep / accidental git-commit of API keys stored on disk. Does
//! *not* protect against an attacker with shell access on the same machine —
//! they can re-run the same derivation. The threat model here is "user
//! accidentally shares secrets.env" or "file ends up in a dotfiles repo".
//!
//! Storage format: `base64(nonce || ciphertext || tag)` as a single line.
//! Nonce is 12 random bytes (AES-GCM standard); tag is the 16-byte GCM auth
//! tag appended by AES-GCM. Total overhead: 12 + 16 = 28 bytes + base64 blow-up.
//!
//! Key derivation: `SHA-256(machine_id || DOMAIN)`. The domain tag prevents
//! the same key from being reused for other purposes (defense-in-depth if we
//! ever add a second encrypted-at-rest feature).
//!
//! Fallbacks: tries `/etc/machine-id` then `/var/lib/dbus/machine-id`. Also
//! honors `STUI_MACHINE_ID` (env override) for tests and portable runs.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"stui-secrets-v1";
const NONCE_LEN: usize = 12;
const MACHINE_ID_PATHS: &[&str] = &["/etc/machine-id", "/var/lib/dbus/machine-id"];

fn machine_id() -> Result<String> {
    if let Ok(override_id) = std::env::var("STUI_MACHINE_ID") {
        if !override_id.trim().is_empty() {
            return Ok(override_id.trim().to_string());
        }
    }
    for path in MACHINE_ID_PATHS {
        if let Ok(raw) = std::fs::read_to_string(path) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Err(anyhow!(
        "no machine-id available (tried {:?} and STUI_MACHINE_ID env)",
        MACHINE_ID_PATHS
    ))
}

fn derive_key() -> Result<[u8; 32]> {
    let mid = machine_id()?;
    let mut h = Sha256::new();
    h.update(mid.as_bytes());
    h.update(DOMAIN);
    Ok(h.finalize().into())
}

/// Encrypt a plaintext secret, returning `base64(nonce || ciphertext_with_tag)`.
pub fn encrypt(plaintext: &str) -> Result<String> {
    let key = derive_key()?;
    let cipher = Aes256Gcm::new((&key).into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes).context("generating nonce")?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| anyhow!("aes-gcm encrypt: {e}"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// Decrypt a secret previously produced by [`encrypt`]. Returns an error if
/// the ciphertext was tampered with, the machine-id changed, or the input is
/// malformed.
pub fn decrypt(b64: &str) -> Result<String> {
    let raw = B64.decode(b64.trim()).context("base64 decode")?;
    if raw.len() <= NONCE_LEN {
        return Err(anyhow!("ciphertext too short"));
    }
    let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
    let key = derive_key()?;
    let cipher = Aes256Gcm::new((&key).into());
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, ct)
        .map_err(|e| anyhow!("aes-gcm decrypt (tampered or wrong machine?): {e}"))?;
    String::from_utf8(pt).context("decrypted bytes were not utf-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_test_machine_id<T>(f: impl FnOnce() -> T) -> T {
        // Serialize so tests don't race on the env var.
        static MU: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = MU.lock().unwrap();
        std::env::set_var("STUI_MACHINE_ID", "deadbeef-test-machine-id");
        let out = f();
        std::env::remove_var("STUI_MACHINE_ID");
        out
    }

    #[test]
    fn roundtrip() {
        with_test_machine_id(|| {
            let enc = encrypt("tvdb-api-key-example").unwrap();
            let dec = decrypt(&enc).unwrap();
            assert_eq!(dec, "tvdb-api-key-example");
        });
    }

    #[test]
    fn different_nonces_each_call() {
        with_test_machine_id(|| {
            let a = encrypt("same-plaintext").unwrap();
            let b = encrypt("same-plaintext").unwrap();
            assert_ne!(a, b, "random nonce must produce distinct ciphertexts");
        });
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        with_test_machine_id(|| {
            let enc = encrypt("secret").unwrap();
            let mut raw = B64.decode(&enc).unwrap();
            // Flip one bit in the ciphertext payload (after the nonce).
            raw[NONCE_LEN + 2] ^= 0x01;
            let tampered = B64.encode(&raw);
            assert!(decrypt(&tampered).is_err());
        });
    }

    #[test]
    fn wrong_machine_id_rejected() {
        let enc = with_test_machine_id(|| encrypt("secret").unwrap());
        static MU: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = MU.lock().unwrap();
        std::env::set_var("STUI_MACHINE_ID", "a-different-machine");
        let result = decrypt(&enc);
        std::env::remove_var("STUI_MACHINE_ID");
        assert!(
            result.is_err(),
            "ciphertext from one machine must not decrypt on another"
        );
    }

    #[test]
    fn truncated_input_errors() {
        with_test_machine_id(|| {
            assert!(decrypt("").is_err());
            assert!(decrypt("AAAA").is_err());
        });
    }
}
