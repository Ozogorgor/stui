//! Writes normalized tags to audio files on disk via lofty.
//!
//! Per-file flow:
//!   1. Read original tags via lofty.
//!   2. Write sidecar JSON backup (if not already present).
//!   3. Write new tags via lofty.
//!
//! Primary backup: `<file>.stui-tag-backup.json`.
//! Fallback (when audio dir is read-only): `~/.local/share/stui/tag-backups/<sha256>.json`.

use lofty::{
    file::TaggedFileExt,
    probe::Probe,
    tag::{Accessor, ItemKey, TagExt},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use super::normalize::NormalizedTags;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OriginalTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub year: String,
    pub genre: String,
    pub track: u32,
    pub disc: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SidecarBackup {
    pub created: String,
    pub stui_version: String,
    pub original: OriginalTags,
}

// Fields are consumed by unit tests (`BackupLocation::Sidecar(p) => p.exists()`,
// `report.wrote_backup`) and by the `tracing::info!(backup = ?backup_location)`
// call in `write_normalized` below — hence the dead_code allow rather than
// dropping the payloads.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum BackupLocation {
    Sidecar(PathBuf),
    Central(PathBuf),
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct WriteReport {
    pub path: PathBuf,
    pub backup_location: BackupLocation,
    pub wrote_backup: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum TagWriteError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("lofty: {0}")]
    Lofty(#[from] lofty::error::LoftyError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no tag in file")]
    NoTag,
    #[error("backup write failed; tag write skipped for safety")]
    BackupFailed,
}

pub fn read_original(path: &Path) -> Result<OriginalTags, TagWriteError> {
    let tagged = Probe::open(path)?.read()?;
    let tag = tagged
        .primary_tag()
        .or(tagged.first_tag())
        .ok_or(TagWriteError::NoTag)?;
    Ok(OriginalTags {
        artist: tag.artist().map(|c| c.to_string()).unwrap_or_default(),
        album_artist: tag
            .get_string(&ItemKey::AlbumArtist)
            .unwrap_or_default()
            .to_string(),
        album: tag.album().map(|c| c.to_string()).unwrap_or_default(),
        title: tag.title().map(|c| c.to_string()).unwrap_or_default(),
        year: tag.year().map(|y| y.to_string()).unwrap_or_default(),
        genre: tag.genre().map(|c| c.to_string()).unwrap_or_default(),
        track: tag.track().unwrap_or(0),
        disc: tag.disk().unwrap_or(0),
    })
}

fn sidecar_path_beside(audio: &Path) -> PathBuf {
    let mut p = audio.as_os_str().to_os_string();
    p.push(".stui-tag-backup.json");
    PathBuf::from(p)
}

fn sidecar_path_central(audio: &Path) -> PathBuf {
    let abs = audio.canonicalize().unwrap_or_else(|_| audio.to_path_buf());
    let mut h = Sha256::new();
    h.update(abs.to_string_lossy().as_bytes());
    let name = format!("{:x}.json", h.finalize());
    let dir = dirs::data_local_dir()
        .map(|d| d.join("stui").join("tag-backups"))
        .unwrap_or_else(|| PathBuf::from(".stui-tag-backups"));
    dir.join(name)
}

fn write_backup_once(
    audio: &Path,
    original: &OriginalTags,
) -> Result<(bool, BackupLocation), TagWriteError> {
    let side = sidecar_path_beside(audio);
    if side.exists() {
        return Ok((false, BackupLocation::Sidecar(side)));
    }

    let payload = SidecarBackup {
        created: chrono::Utc::now().to_rfc3339(),
        stui_version: env!("CARGO_PKG_VERSION").to_string(),
        original: original.clone(),
    };
    let json = serde_json::to_vec_pretty(&payload)?;

    match fs::write(&side, &json) {
        Ok(()) => Ok((true, BackupLocation::Sidecar(side))),
        Err(_) => {
            let central = sidecar_path_central(audio);
            if central.exists() {
                return Ok((false, BackupLocation::Central(central)));
            }
            if let Some(parent) = central.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&central, &json)?;
            Ok((true, BackupLocation::Central(central)))
        }
    }
}

pub fn write_normalized(path: &Path, n: &NormalizedTags) -> Result<WriteReport, TagWriteError> {
    let original = read_original(path)?;
    let (wrote_backup, backup_location) =
        write_backup_once(path, &original).map_err(|_| TagWriteError::BackupFailed)?;

    let mut tagged = Probe::open(path)?.read()?;
    let tag = tagged.primary_tag_mut().ok_or(TagWriteError::NoTag)?;

    tag.set_artist(n.artist.clone());
    tag.insert_text(ItemKey::AlbumArtist, n.album_artist.clone());
    tag.set_album(n.album.clone());
    tag.set_title(n.title.clone());
    if let Ok(y) = n.year.parse::<u32>() {
        tag.set_year(y);
    }
    tag.set_genre(n.genre.clone());
    if n.track > 0 {
        tag.set_track(n.track);
    }
    if n.disc > 0 {
        tag.set_disk(n.disc);
    }

    tag.save_to_path(path, lofty::config::WriteOptions::default())?;

    tracing::info!(
        path = %path.display(),
        backup = ?backup_location,
        wrote_backup,
        "tag_writer: wrote normalized tags",
    );

    Ok(WriteReport {
        path: path.to_path_buf(),
        backup_location,
        wrote_backup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_bytes() -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample.mp3");
        assert!(
            path.exists(),
            "fixture missing at {}; regenerate via:\n  \
             ffmpeg -f lavfi -i \"sine=frequency=440:duration=0.1\" \
             -metadata artist=\"pink floyd\" -metadata album=\"the wall\" \
             -metadata title=\"comfortably numb\" -c:a libmp3lame \
             runtime/tests/fixtures/sample.mp3",
            path.display()
        );
        fs::read(&path).unwrap()
    }

    #[test]
    fn writes_backup_and_tags() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.mp3");
        let mut f = fs::File::create(&file).unwrap();
        f.write_all(&fixture_bytes()).unwrap();
        drop(f);

        let n = NormalizedTags {
            artist: "Pink Floyd".into(),
            album: "The Wall".into(),
            title: "Comfortably Numb".into(),
            year: "1979".into(),
            ..Default::default()
        };
        let report = write_normalized(&file, &n).unwrap();
        assert!(report.wrote_backup);
        match &report.backup_location {
            BackupLocation::Sidecar(p) => assert!(p.exists()),
            BackupLocation::Central(p) => assert!(p.exists()),
        }

        let o = read_original(&file).unwrap();
        assert_eq!(o.artist, "Pink Floyd");
        assert_eq!(o.album, "The Wall");
        assert_eq!(o.title, "Comfortably Numb");

        // Backup must not be overwritten on second write.
        let report2 = write_normalized(&file, &n).unwrap();
        assert!(!report2.wrote_backup);
    }
}
