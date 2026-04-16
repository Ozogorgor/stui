//! Parser for MPD's configuration file.
//!
//! Extracts key directory paths (`music_directory`, `playlist_directory`)
//! from mpd.conf so STUI can auto-detect them without the user having to
//! duplicate the values in stui.toml.
//!
//! Search order: `~/.config/mpd/mpd.conf`, `~/.mpd/mpd.conf`, `/etc/mpd.conf`.

use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct MpdConfPaths {
    pub music_directory: Option<PathBuf>,
    pub playlist_directory: Option<PathBuf>,
}

/// Try to locate and parse mpd.conf, extracting directory paths.
pub fn detect() -> MpdConfPaths {
    let candidates = candidates();
    for path in &candidates {
        if path.exists() {
            if let Ok(paths) = parse(path) {
                tracing::info!(path = %path.display(), "parsed mpd.conf");
                return paths;
            }
        }
    }
    tracing::debug!("mpd.conf not found at any standard location");
    MpdConfPaths::default()
}

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".config").join("mpd").join("mpd.conf"));
        paths.push(home.join(".mpd").join("mpd.conf"));
    }
    paths.push(PathBuf::from("/etc/mpd.conf"));
    paths
}

fn parse(path: &Path) -> anyhow::Result<MpdConfPaths> {
    let content = std::fs::read_to_string(path)?;
    let mut result = MpdConfPaths::default();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(val) = extract_directive(trimmed, "music_directory") {
            result.music_directory = Some(expand_path(&val));
        } else if let Some(val) = extract_directive(trimmed, "playlist_directory") {
            result.playlist_directory = Some(expand_path(&val));
        }
    }
    Ok(result)
}

/// Extract the quoted value from a line like `directive "value"`.
fn extract_directive<'a>(line: &'a str, directive: &str) -> Option<String> {
    let rest = line.strip_prefix(directive)?.trim_start();
    let unquoted = rest.trim_matches('"');
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

/// Expand `~` and common `$XDG_*` variables.
fn expand_path(raw: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let expanded = raw
        .replace("~", &home.to_string_lossy())
        .replace("$XDG_MUSIC_DIR", &dirs::audio_dir()
            .unwrap_or_else(|| home.join("Music"))
            .to_string_lossy())
        .replace("$XDG_CONFIG_HOME", &dirs::config_dir()
            .unwrap_or_else(|| home.join(".config"))
            .to_string_lossy())
        .replace("$XDG_DATA_HOME", &dirs::data_local_dir()
            .unwrap_or_else(|| home.join(".local").join("share"))
            .to_string_lossy())
        .replace("$XDG_CACHE_HOME", &dirs::cache_dir()
            .unwrap_or_else(|| home.join(".cache"))
            .to_string_lossy())
        .replace("$XDG_RUNTIME_DIR", &std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() })));
    PathBuf::from(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_basic() {
        assert_eq!(
            extract_directive(r#"music_directory "~/Music""#, "music_directory"),
            Some("~/Music".to_string()),
        );
    }

    #[test]
    fn extract_with_tabs() {
        assert_eq!(
            extract_directive("playlist_directory\t\t\"~/.config/mpd/playlists\"", "playlist_directory"),
            Some("~/.config/mpd/playlists".to_string()),
        );
    }

    #[test]
    fn extract_wrong_directive() {
        assert_eq!(extract_directive(r#"music_directory "~/Music""#, "playlist_directory"), None);
    }

    #[test]
    fn expand_tilde() {
        let p = expand_path("~/Music");
        assert!(!p.to_string_lossy().contains('~'));
        assert!(p.to_string_lossy().starts_with('/'));
    }
}
