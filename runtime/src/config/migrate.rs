//! migrate — one-shot relocation from the legacy `~/.stui/` single
//! root to the XDG-compliant split (`~/.config/stui/` for config,
//! plugins, data; `~/.cache/stui/` for caches).
//!
//! Idempotent: each migration only runs when the destination is
//! missing AND the source still exists. Existing user files at the
//! new locations are left strictly alone — we never overwrite. The
//! source `~/.stui/` is left in place after the move so a user can
//! verify the relocation and clean it up at their own pace.
//!
//! Caches are NOT migrated — they regenerate on demand and copying
//! them just wastes I/O.
//!
//! Called once at runtime startup before config load (see
//! `runtime/src/main.rs`).
//!
//! ## What moves
//!
//! | source                          | destination                            |
//! |---------------------------------|----------------------------------------|
//! | `~/.stui/plugins/`              | `~/.config/stui/plugins/`              |
//! | `~/.stui/secrets.env`           | `~/.config/stui/secrets.env`           |
//! | `~/.stui/config/stui.toml`      | `~/.config/stui/runtime.toml`          |
//! | `~/.stui/data/`                 | `~/.config/stui/data/`                 |

use std::path::{Path, PathBuf};

use tracing::{info, warn};

fn legacy_base() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".stui")
}

fn config_base() -> PathBuf {
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stui")
}

/// Run the legacy → XDG migration. Safe to call on every startup;
/// each step is a no-op when the destination already exists or the
/// source doesn't.
pub fn migrate_legacy_paths() {
    let legacy = legacy_base();
    let config = config_base();
    if !legacy.exists() {
        // Fresh install or already migrated — nothing to do.
        return;
    }

    move_if_destination_missing(&legacy.join("plugins"), &config.join("plugins"), "plugins");
    move_if_destination_missing(
        &legacy.join("secrets.env"),
        &config.join("secrets.env"),
        "secrets",
    );
    move_if_destination_missing(
        &legacy.join("config").join("stui.toml"),
        &config.join("runtime.toml"),
        "runtime config",
    );
    move_if_destination_missing(&legacy.join("data"), &config.join("data"), "data");
}

fn move_if_destination_missing(src: &Path, dst: &Path, label: &str) {
    if !src.exists() || dst.exists() {
        return;
    }
    if let Some(parent) = dst.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!(
                kind = label,
                parent = %parent.display(),
                error = %e,
                "migrate: create parent failed",
            );
            return;
        }
    }
    match std::fs::rename(src, dst) {
        Ok(_) => info!(
            kind = label,
            from = %src.display(),
            to   = %dst.display(),
            "migrate: relocated",
        ),
        Err(e) => warn!(
            kind = label,
            from = %src.display(),
            to   = %dst.display(),
            error = %e,
            "migrate: rename failed (cross-device? read-only?) — leaving source in place",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(p: &PathBuf) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn move_runs_when_dest_missing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        touch(&src);
        move_if_destination_missing(&src, &dst, "test");
        assert!(!src.exists(), "src should be moved away");
        assert!(dst.exists(), "dst should now exist");
    }

    #[test]
    fn move_skips_when_dest_exists() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        touch(&src);
        touch(&dst);
        let dst_data_before = fs::read(&dst).unwrap();
        move_if_destination_missing(&src, &dst, "test");
        assert!(src.exists(), "src must remain when dst already exists");
        assert_eq!(fs::read(&dst).unwrap(), dst_data_before, "dst untouched");
    }
}
