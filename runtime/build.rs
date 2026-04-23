//! Build script: embed the project's TVDB API key into the binary.
//!
//! TVDB issues keys to projects rather than end users, so the key ships with
//! stui itself. To keep it out of `strings dist/stui-runtime`, the key is
//! XOR-obfuscated at build time against a fixed salt and written as a raw
//! byte blob into `$OUT_DIR/tvdb_embed.bin`. The runtime loads it via
//! `include_bytes!` and reverses the XOR at startup.
//!
//! Priority for the plaintext input, highest first:
//!   1. `TVDB_API_KEY` build-time env var
//!   2. `runtime/.tvdb-key` file contents (single line, trimmed)
//!
//! If neither source is set, an empty blob is emitted and the compiled
//! binary simply has TVDB disabled. That's fine for users building from
//! source without the release key — plugins still provide metadata.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Fixed salt XORed against the plaintext. Not a secret — its purpose is
/// purely to break naive `strings`/`grep` extraction of the plaintext key
/// from the binary. A determined attacker with a disassembler trivially
/// recovers both the salt and the key; that's the inherent ceiling of any
/// shipped-key scheme.
///
/// **MUST stay byte-identical to `XOR_SALT` in `src/tvdb/mod.rs`.** The
/// runtime deobfuscator XORs against the same bytes; drift silently
/// returns garbage plaintext (fails the utf-8 check → "no key embedded").
/// There is no compile-time cross-check between the two constants — if you
/// edit one, grep for the other.
const XOR_SALT: &[u8] = b"stui-tvdb-embed-v1-obfuscation-salt-f3a9";

fn main() {
    // Cargo rebuild triggers.
    println!("cargo:rerun-if-env-changed=TVDB_API_KEY");
    println!("cargo:rerun-if-changed=.tvdb-key");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR").expect("cargo must set OUT_DIR");
    let out_path = PathBuf::from(&out_dir).join("tvdb_embed.bin");

    let key = env::var("TVDB_API_KEY")
        .ok()
        .or_else(|| fs::read_to_string("runtime/.tvdb-key").ok())
        .or_else(|| fs::read_to_string(".tvdb-key").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    if key.is_empty() {
        println!(
            "cargo:warning=TVDB_API_KEY unset and no runtime/.tvdb-key found — \
             tvdb will be disabled in this binary"
        );
    }

    let obfuscated: Vec<u8> = key
        .bytes()
        .enumerate()
        .map(|(i, b)| b ^ XOR_SALT[i % XOR_SALT.len()])
        .collect();

    fs::write(&out_path, &obfuscated).expect("writing tvdb_embed.bin");
}
