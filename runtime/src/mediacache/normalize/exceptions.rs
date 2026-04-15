//! Exception list loader.
//!
//! Two sources, merged at load time:
//!   1. Bundled: shipped with STUI, community-maintained.
//!   2. User:    `~/.config/stui/exceptions.toml`, auto-learn + manual edits.
//!
//! Membership tests are case-insensitive on pre-normalized raw values.
//! A SHA-256 content hash over merged bytes serves as a cache key.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

#[derive(Debug, Default, Deserialize, Serialize)]
struct FileShape {
    #[serde(default)] artist: FieldList,
    #[serde(default)] album_artist: FieldList,
    #[serde(default)] album: FieldList,
    #[serde(default)] title: FieldList,
    #[serde(default)] genre: FieldList,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FieldList {
    #[serde(default)] values: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ExceptionList {
    pub artist: HashSet<String>,
    pub album_artist: HashSet<String>,
    pub album: HashSet<String>,
    pub title: HashSet<String>,
    pub genre: HashSet<String>,
    /// SHA-256 hex of merged-source bytes, for cache keys.
    pub content_hash: String,
}

impl ExceptionList {
    pub fn is_artist_protected(&self, raw: &str) -> bool {
        self.artist.contains(&raw.to_lowercase())
    }
    pub fn is_album_artist_protected(&self, raw: &str) -> bool {
        self.album_artist.contains(&raw.to_lowercase())
    }
    pub fn is_album_protected(&self, raw: &str) -> bool {
        self.album.contains(&raw.to_lowercase())
    }
    pub fn is_title_protected(&self, raw: &str) -> bool {
        self.title.contains(&raw.to_lowercase())
    }
    pub fn is_genre_protected(&self, raw: &str) -> bool {
        self.genre.contains(&raw.to_lowercase())
    }
}

fn parse_file(bytes: &[u8]) -> FileShape {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    toml::from_str::<FileShape>(s).unwrap_or_default()
}

fn read_if_exists(path: &Path) -> Option<Vec<u8>> { fs::read(path).ok() }

pub fn merge(bundled: Option<&[u8]>, user: Option<&[u8]>) -> ExceptionList {
    let b = bundled.map(parse_file).unwrap_or_default();
    let u = user.map(parse_file).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bundled.unwrap_or_default());
    hasher.update(b":");
    hasher.update(user.unwrap_or_default());
    let content_hash = format!("{:x}", hasher.finalize());

    let lower = |vs: &[String]| vs.iter().map(|s| s.to_lowercase()).collect::<HashSet<_>>();
    let mut out = ExceptionList {
        artist: lower(&b.artist.values),
        album_artist: lower(&b.album_artist.values),
        album: lower(&b.album.values),
        title: lower(&b.title.values),
        genre: lower(&b.genre.values),
        content_hash,
    };
    out.artist.extend(u.artist.values.iter().map(|s| s.to_lowercase()));
    out.album_artist.extend(u.album_artist.values.iter().map(|s| s.to_lowercase()));
    out.album.extend(u.album.values.iter().map(|s| s.to_lowercase()));
    out.title.extend(u.title.values.iter().map(|s| s.to_lowercase()));
    out.genre.extend(u.genre.values.iter().map(|s| s.to_lowercase()));
    out
}

pub struct ExceptionStore {
    bundled_path: PathBuf,
    user_path: PathBuf,
    state: RwLock<Cached>,
    reload_lock: Mutex<()>,
}

#[derive(Default)]
struct Cached {
    list: ExceptionList,
    bundled_mtime: Option<SystemTime>,
    user_mtime: Option<SystemTime>,
    initialized: bool,
}

impl ExceptionStore {
    pub fn new(bundled_path: PathBuf, user_path: PathBuf) -> Self {
        Self {
            bundled_path, user_path,
            state: RwLock::new(Cached::default()),
            reload_lock: Mutex::new(()),
        }
    }

    pub fn get(&self) -> ExceptionList {
        let b_mt = fs::metadata(&self.bundled_path).and_then(|m| m.modified()).ok();
        let u_mt = fs::metadata(&self.user_path).and_then(|m| m.modified()).ok();
        {
            let st = self.state.read().unwrap();
            if st.initialized && st.bundled_mtime == b_mt && st.user_mtime == u_mt {
                return st.list.clone();
            }
        }
        let _g = self.reload_lock.lock().unwrap();
        {
            let st = self.state.read().unwrap();
            if st.initialized && st.bundled_mtime == b_mt && st.user_mtime == u_mt {
                return st.list.clone();
            }
        }
        let bundled = read_if_exists(&self.bundled_path);
        let user = read_if_exists(&self.user_path);
        let list = merge(bundled.as_deref(), user.as_deref());
        let mut st = self.state.write().unwrap();
        st.list = list.clone();
        st.bundled_mtime = b_mt;
        st.user_mtime = u_mt;
        st.initialized = true;
        list
    }

    pub fn add_user_exception(&self, field: ExceptionField, raw_value: &str) -> std::io::Result<bool> {
        let value = raw_value.trim();
        if value.is_empty() { return Ok(false); }

        let _g = self.reload_lock.lock().unwrap();
        let mut file: FileShape = fs::read(&self.user_path).ok()
            .and_then(|b| toml::from_str(std::str::from_utf8(&b).unwrap_or("")).ok())
            .unwrap_or_default();

        let list = match field {
            ExceptionField::Artist => &mut file.artist.values,
            ExceptionField::AlbumArtist => &mut file.album_artist.values,
            ExceptionField::Album => &mut file.album.values,
            ExceptionField::Title => &mut file.title.values,
            ExceptionField::Genre => &mut file.genre.values,
        };
        if list.iter().any(|v| v.eq_ignore_ascii_case(value)) { return Ok(false); }
        list.push(value.to_string());

        if let Some(parent) = self.user_path.parent() { let _ = fs::create_dir_all(parent); }
        let serialized = toml::to_string_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(&self.user_path, serialized)?;

        let mut st = self.state.write().unwrap();
        st.initialized = false;
        Ok(true)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ExceptionField { Artist, AlbumArtist, Album, Title, Genre }

impl ExceptionField {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "artist" => Some(Self::Artist),
            "album_artist" => Some(Self::AlbumArtist),
            "album" => Some(Self::Album),
            "title" => Some(Self::Title),
            "genre" => Some(Self::Genre),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_combines_sources() {
        let bundled = br#"
            [artist]
            values = ["AC/DC"]
            [album]
            values = []
        "#;
        let user = br#"
            [artist]
            values = ["deadmau5"]
        "#;
        let list = merge(Some(bundled), Some(user));
        assert!(list.is_artist_protected("ac/dc"));
        assert!(list.is_artist_protected("DEADMAU5"));
        assert!(!list.is_album_protected("anything"));
        assert!(!list.content_hash.is_empty());
    }

    #[test]
    fn merge_empty() {
        let list = merge(None, None);
        assert!(list.artist.is_empty());
        assert!(!list.content_hash.is_empty());
    }

    #[test]
    fn hash_changes_with_content() {
        let a = merge(Some(b"[artist]\nvalues = [\"x\"]\n"), None);
        let b = merge(Some(b"[artist]\nvalues = [\"y\"]\n"), None);
        assert_ne!(a.content_hash, b.content_hash);
    }

    #[test]
    fn add_user_exception_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("exceptions.toml");
        let store = ExceptionStore::new(PathBuf::from("/nonexistent"), user_path.clone());
        assert!(store.add_user_exception(ExceptionField::Artist, "deadmau5").unwrap());
        assert!(!store.add_user_exception(ExceptionField::Artist, "deadmau5").unwrap());
        let list = store.get();
        assert!(list.is_artist_protected("DEADMAU5"));
    }

    #[test]
    fn add_user_exception_empty_noop() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExceptionStore::new(
            PathBuf::from("/nonexistent"),
            dir.path().join("exceptions.toml"),
        );
        assert!(!store.add_user_exception(ExceptionField::Artist, "   ").unwrap());
    }
}
