//! Album art extraction from audio files via lofty.
//!
//! Extracts the first embedded picture (front cover preferred) and caches
//! it to `~/.stui/cache/art/<sha256>.jpg` so subsequent requests are instant.

use lofty::{file::TaggedFileExt, probe::Probe};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Extract album art from an audio file and return the cached image path.
/// Returns None if the file has no embedded pictures.
pub fn extract(audio_path: &Path) -> Option<PathBuf> {
    let cache_dir = art_cache_dir()?;

    // Cache key: hash of the audio file path
    let mut hasher = Sha256::new();
    hasher.update(audio_path.to_string_lossy().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let cached = cache_dir.join(format!("{}.jpg", &hash[..16]));

    // Cache hit
    if cached.exists() {
        return Some(cached);
    }

    // Extract via lofty
    let tagged = match Probe::open(audio_path) {
        Ok(p) => match p.read() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(path = %audio_path.display(), error = %e, "album_art: lofty read failed");
                return None;
            }
        },
        Err(e) => {
            tracing::warn!(path = %audio_path.display(), error = %e, "album_art: lofty open failed");
            return None;
        }
    };
    let tag = match tagged.primary_tag().or(tagged.first_tag()) {
        Some(t) => t,
        None => {
            tracing::warn!(path = %audio_path.display(), "album_art: no tags found");
            return None;
        }
    };
    let pictures = tag.pictures();
    tracing::info!(path = %audio_path.display(), count = pictures.len(), "album_art: pictures found");
    if pictures.is_empty() {
        return None;
    }

    // Prefer front cover, fall back to first picture
    let pic = pictures
        .iter()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
        .unwrap_or(&pictures[0]);

    let data = pic.data();
    if data.is_empty() {
        return None;
    }

    // Write to cache
    fs::create_dir_all(&cache_dir).ok()?;
    fs::write(&cached, data).ok()?;

    tracing::info!(
        audio = %audio_path.display(),
        cached = %cached.display(),
        size = data.len(),
        "album_art: extracted embedded picture",
    );

    Some(cached)
}

fn art_cache_dir() -> Option<PathBuf> {
    // Album-art tiles are caches, so they live under XDG_CACHE_HOME
    // (`~/.cache/stui/art/`) — safe to delete, regenerated on next
    // playback. Falls back to `~/.cache/` if XDG_CACHE_HOME is unset.
    dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
        .map(|c| c.join("stui").join("art"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_exists() {
        let dir = art_cache_dir();
        assert!(dir.is_some());
    }
}
