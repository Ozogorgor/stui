//! Media storage manager — handles file organization and path generation.

pub mod download_translator;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Media storage manager for organizing downloaded media files.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct MediaStorage {
    base_movies: PathBuf,
    base_series: PathBuf,
    base_anime: PathBuf,
    base_music: PathBuf,
    base_podcasts: PathBuf,
    /// Maps torrent-engine original paths → user-visible organized paths (not persisted)
    #[allow(dead_code)] // internal: field retained for future path translation
    path_translator: Arc<RwLock<HashMap<PathBuf, PathBuf>>>,
}

impl MediaStorage {
    pub fn new(
        movies: PathBuf,
        series: PathBuf,
        anime: PathBuf,
        music: PathBuf,
        podcasts: PathBuf,
    ) -> Self {
        Self {
            base_movies: movies,
            base_series: series,
            base_anime: anime,
            base_music: music,
            base_podcasts: podcasts,
            path_translator: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Returns the base storage directory paths.
    pub fn base_paths(&self) -> StoragePaths {
        StoragePaths {
            movies: self.base_movies.clone(),
            series: self.base_series.clone(),
            anime: self.base_anime.clone(),
            music: self.base_music.clone(),
            podcasts: self.base_podcasts.clone(),
        }
    }

    // ── Movies ───────────────────────────────────────────────────────────────

    /// Returns the folder path for a movie: `base/Movies/{year} - {title}/
    pub fn movie_folder(&self, title: &str, year: Option<u32>) -> PathBuf {
        let folder_name = format_year_title(year, title);
        self.base_movies.join("Movies").join(folder_name)
    }

    /// Returns the full file path for a movie file: `base/Movies/{year} - {title}/{filename}.{ext}`
    pub fn movie_path(&self, title: &str, year: Option<u32>, filename: &str) -> PathBuf {
        let folder = self.movie_folder(title, year);
        folder.join(sanitize_filename(filename))
    }

    // ── Series ───────────────────────────────────────────────────────────────

    /// Returns the folder path for a series: `base/Series/{year} - {title}/
    pub fn series_folder(&self, show_title: &str, year: Option<u32>) -> PathBuf {
        let folder_name = format_year_title(year, show_title);
        self.base_series.join("Series").join(folder_name)
    }

    /// Returns the season folder path: `base/Series/{year} - {title}/Season {n} - {name}/
    pub fn season_folder(
        &self,
        show_title: &str,
        year: Option<u32>,
        season: u32,
        season_name: Option<&str>,
    ) -> PathBuf {
        let base = self.series_folder(show_title, year);
        let season_dir = format_season_dir(season, season_name);
        base.join(season_dir)
    }

    /// Returns the full file path for an episode.
    pub fn episode_path(
        &self,
        show_title: &str,
        year: Option<u32>,
        season: u32,
        season_name: Option<&str>,
        filename: &str,
    ) -> PathBuf {
        let folder = self.season_folder(show_title, year, season, season_name);
        folder.join(sanitize_filename(filename))
    }

    // ── Anime ───────────────────────────────────────────────────────────────

    /// Returns the folder path for anime: `base/Anime/{year} - {title}/
    pub fn anime_folder(&self, title: &str, year: Option<u32>) -> PathBuf {
        let folder_name = format_year_title(year, title);
        self.base_anime.join("Anime").join(folder_name)
    }

    /// Returns the season folder path for anime.
    pub fn anime_season_folder(
        &self,
        title: &str,
        year: Option<u32>,
        season: u32,
        season_name: Option<&str>,
    ) -> PathBuf {
        let base = self.anime_folder(title, year);
        let season_dir = format_season_dir(season, season_name);
        base.join(season_dir)
    }

    /// Returns the full file path for an anime episode.
    pub fn anime_episode_path(
        &self,
        title: &str,
        year: Option<u32>,
        season: u32,
        season_name: Option<&str>,
        filename: &str,
    ) -> PathBuf {
        let folder = self.anime_season_folder(title, year, season, season_name);
        folder.join(sanitize_filename(filename))
    }

    // ── Anime Movies ─────────────────────────────────────────────────────────

    /// Returns the folder path for an anime movie: `base/Anime/{year} - {title}/
    pub fn anime_movie_folder(&self, title: &str, year: Option<u32>) -> PathBuf {
        self.anime_folder(title, year)
    }

    /// Returns the full file path for an anime movie file.
    pub fn anime_movie_path(&self, title: &str, year: Option<u32>, filename: &str) -> PathBuf {
        let folder = self.anime_movie_folder(title, year);
        folder.join(sanitize_filename(filename))
    }

    // ── Music ───────────────────────────────────────────────────────────────

    /// Returns the artist folder path: `base/{artist}/
    pub fn artist_folder(&self, artist: &str) -> PathBuf {
        sanitize_folder_name(artist)
            .map(|t| self.base_music.join(t))
            .unwrap_or_else(|| self.base_music.clone())
    }

    /// Returns the album folder path: `base/{artist}/{year} - {album}/
    pub fn album_folder(&self, artist: &str, album: &str, year: Option<u32>) -> PathBuf {
        let artist_folder = self.artist_folder(artist);
        let album_name = format_album_folder_name(year, album);
        artist_folder.join(album_name)
    }

    /// Returns the album art folder path: `base/{artist}/{year} - {album}/Album Art/
    pub fn album_art_folder(&self, artist: &str, album: &str, year: Option<u32>) -> PathBuf {
        self.album_folder(artist, album, year).join("Album Art")
    }

    /// Returns the full file path for a track: `base/{artist}/{year} - {album}/{##} - {title}.{ext}`
    pub fn track_path(
        &self,
        artist: &str,
        album: &str,
        year: Option<u32>,
        track_number: Option<u32>,
        title: &str,
        extension: &str,
    ) -> PathBuf {
        let folder = self.album_folder(artist, album, year);
        let ext = extension.trim_start_matches('.');
        let filename = match track_number {
            Some(n) => format!("{:02} - {}.{}", n, sanitize_filename(title), ext),
            None => format!("{}.{}", sanitize_filename(title), ext),
        };
        folder.join(filename)
    }

    /// Returns path for album art: `base/{artist}/{year} - {album}/Album Art/{filename}`
    pub fn album_art_path(
        &self,
        artist: &str,
        album: &str,
        year: Option<u32>,
        filename: &str,
    ) -> PathBuf {
        let folder = self.album_art_folder(artist, album, year);
        folder.join(sanitize_filename(filename))
    }

    // ── Podcasts ───────────────────────────────────────────────────────────

    /// Returns the podcast folder path: `base/Podcasts/{podcast}/
    pub fn podcast_folder(&self, podcast: &str) -> PathBuf {
        sanitize_folder_name(podcast)
            .map(|t| self.base_podcasts.join("Podcasts").join(t))
            .unwrap_or_else(|| self.base_podcasts.join("Podcasts"))
    }

    /// Returns the full file path for a podcast episode.
    pub fn podcast_episode_path(&self, podcast: &str, filename: &str) -> PathBuf {
        let folder = self.podcast_folder(podcast);
        folder.join(sanitize_filename(filename))
    }

    // ── Subtitles ───────────────────────────────────────────────────────────

    /// Returns the subtitle file path in the same folder as the media.
    pub fn subtitle_path(media_path: &Path, language: Option<&str>, extension: &str) -> PathBuf {
        let parent = media_path.parent().unwrap_or(media_path);
        let stem = media_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("subtitle");
        let ext = extension.trim_start_matches('.');

        let filename = match language {
            Some(lang) => format!("{}.{}.{}", stem, lang, ext),
            None => format!("{}.{}", stem, ext),
        };

        parent.join(filename)
    }

    // ── Torrent Path Translation Layer ─────────────────────────────────────────────

    /// Register a mapping from the torrent engine's original path to our organized path.
    /// This is called when a download starts to track the translation.
    pub async fn register_translation(&self, original: PathBuf, organized: PathBuf) {
        let mut translator = self.path_translator.write().await;
        translator.insert(original.clone(), organized.clone());
        debug!(
            original = %original.display(),
            organized = %organized.display(),
            "registered path translation"
        );
    }

    /// Get the organized path for a torrent-engine original path.
    pub async fn get_organized_path(&self, original: &PathBuf) -> Option<PathBuf> {
        let translator = self.path_translator.read().await;
        translator.get(original).cloned()
    }

    /// Get the original path for an organized path (reverse lookup).
    pub async fn get_original_path(&self, organized: &PathBuf) -> Option<PathBuf> {
        let translator = self.path_translator.read().await;
        translator
            .iter()
            .find(|(_, v)| *v == organized)
            .map(|(k, _)| k.clone())
    }

    /// Remove a translation mapping (when download completes or is removed).
    pub async fn remove_translation(&self, original: &PathBuf) {
        let mut translator = self.path_translator.write().await;
        if let Some(organized) = translator.remove(original) {
            debug!(
                original = %original.display(),
                organized = %organized.display(),
                "removed path translation"
            );
        }
    }

    /// List all active translations (for persistence).
    pub async fn get_all_translations(&self) -> HashMap<PathBuf, PathBuf> {
        let translator = self.path_translator.read().await;
        translator.clone()
    }

    /// Restore translations from persisted state.
    pub async fn restore_translations(&self, translations: HashMap<PathBuf, PathBuf>) {
        let mut translator = self.path_translator.write().await;
        *translator = translations;
        info!(count = translator.len(), "restored path translations");
    }

    // ── Folder Creation ────────────────────────────────────────────────────

    /// Ensures a folder exists, creating it if necessary.
    pub fn ensure_folder(path: &PathBuf) -> io::Result<()> {
        if !path.exists() {
            info!(path = %path.display(), "creating storage folder");
            fs::create_dir_all(path)?;
        } else {
            debug!(path = %path.display(), "storage folder exists");
        }
        Ok(())
    }

    /// Ensures the folder for a movie exists.
    pub fn ensure_movie_folder(&self, title: &str, year: Option<u32>) -> io::Result<PathBuf> {
        let path = self.movie_folder(title, year);
        Self::ensure_folder(&path)?;
        Ok(path)
    }

    /// Ensures the season folder for a series exists.
    pub fn ensure_season_folder(
        &self,
        show_title: &str,
        year: Option<u32>,
        season: u32,
        season_name: Option<&str>,
    ) -> io::Result<PathBuf> {
        let path = self.season_folder(show_title, year, season, season_name);
        Self::ensure_folder(&path)?;
        Ok(path)
    }

    /// Ensures the album folder for music exists.
    pub fn ensure_album_folder(
        &self,
        artist: &str,
        album: &str,
        year: Option<u32>,
    ) -> io::Result<PathBuf> {
        let path = self.album_folder(artist, album, year);
        Self::ensure_folder(&path)?;
        // Also create Album Art subfolder
        let art_path = self.album_art_folder(artist, album, year);
        Self::ensure_folder(&art_path)?;
        Ok(path)
    }

    /// Ensures the podcast folder exists.
    pub fn ensure_podcast_folder(&self, podcast: &str) -> io::Result<PathBuf> {
        let path = self.podcast_folder(podcast);
        Self::ensure_folder(&path)?;
        Ok(path)
    }
}

/// Storage paths container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePaths {
    pub movies: PathBuf,
    pub series: PathBuf,
    pub anime: PathBuf,
    pub music: PathBuf,
    pub podcasts: PathBuf,
}

// ── Formatting Helpers ────────────────────────────────────────────────────────

fn format_year_title(year: Option<u32>, title: &str) -> String {
    match year {
        Some(y) => format!("{} - {}", y, title),
        None => title.to_string(),
    }
}

fn format_season_dir(season: u32, name: Option<&str>) -> String {
    match name {
        Some(n) if !n.is_empty() => format!("Season {} - {}", season, n),
        _ => format!("Season {}", season),
    }
}

fn format_album_folder_name(year: Option<u32>, album: &str) -> String {
    match year {
        Some(y) => format!("{} - {}", y, album),
        None => album.to_string(),
    }
}

/// Sanitizes a folder name for use in filesystem paths.
fn sanitize_folder_name(name: &str) -> Option<String> {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

/// Sanitizes a filename for use in filesystem paths.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_storage() -> MediaStorage {
        MediaStorage::new(
            PathBuf::from("/home/user/Videos"),
            PathBuf::from("/home/user/Videos"),
            PathBuf::from("/home/user/Videos"),
            PathBuf::from("/home/user/Music"),
            PathBuf::from("/home/user/Music"),
        )
    }

    #[test]
    fn test_movie_path_with_year() {
        let storage = test_storage();
        let path = storage.movie_path("Dune Part Two", Some(2024), "Dune.Part.Two.2024.1080p.mp4");
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Videos/Movies/2024 - Dune Part Two/Dune.Part.Two.2024.1080p.mp4"
        );
    }

    #[test]
    fn test_movie_path_without_year() {
        let storage = test_storage();
        let path = storage.movie_path("Unknown Movie", None, "video.mp4");
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Videos/Movies/Unknown Movie/video.mp4"
        );
    }

    #[test]
    fn test_series_season_with_name() {
        let storage = test_storage();
        let path = storage.season_folder("Breaking Bad", Some(2008), 1, Some("Pilot"));
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Videos/Series/2008 - Breaking Bad/Season 1 - Pilot"
        );
    }

    #[test]
    fn test_series_season_without_name() {
        let storage = test_storage();
        let path = storage.season_folder("Breaking Bad", Some(2008), 1, None);
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Videos/Series/2008 - Breaking Bad/Season 1"
        );
    }

    #[test]
    fn test_episode_path() {
        let storage = test_storage();
        let path = storage.episode_path("Breaking Bad", Some(2008), 1, Some("Pilot"), "S01E01.mkv");
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Videos/Series/2008 - Breaking Bad/Season 1 - Pilot/S01E01.mkv"
        );
    }

    #[test]
    fn test_album_folder_with_year() {
        let storage = test_storage();
        let path = storage.album_folder("Pink Floyd", "Dark Side of the Moon", Some(1973));
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Music/Pink Floyd/1973 - Dark Side of the Moon"
        );
    }

    #[test]
    fn test_track_path() {
        let storage = test_storage();
        let path = storage.track_path(
            "Pink Floyd",
            "Dark Side of the Moon",
            Some(1973),
            Some(1),
            "Speak to Me",
            "flac",
        );
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Music/Pink Floyd/1973 - Dark Side of the Moon/01 - Speak to Me.flac"
        );
    }

    #[test]
    fn test_album_art_folder() {
        let storage = test_storage();
        let path = storage.album_art_folder("Pink Floyd", "Dark Side of the Moon", Some(1973));
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Music/Pink Floyd/1973 - Dark Side of the Moon/Album Art"
        );
    }

    #[test]
    fn test_anime_folder() {
        let storage = test_storage();
        let path = storage.anime_folder("Attack on Titan", Some(2013));
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/Videos/Anime/2013 - Attack on Titan"
        );
    }

    #[test]
    fn test_subtitle_path() {
        let media = PathBuf::from("/videos/Movies/2024 - Dune/video.mp4");
        let sub = MediaStorage::subtitle_path(&media, Some("eng"), "srt");
        assert_eq!(
            sub.to_str().unwrap(),
            "/videos/Movies/2024 - Dune/video.eng.srt"
        );
    }

    #[test]
    fn test_sanitize_removes_invalid_chars() {
        let storage = test_storage();
        let path = storage.movie_path("Star Wars Episode IV", Some(1977), "movie:file*.mkv");
        assert!(!path.file_name().unwrap().to_str().unwrap().contains(':'));
        assert!(!path.file_name().unwrap().to_str().unwrap().contains('*'));
    }
}
