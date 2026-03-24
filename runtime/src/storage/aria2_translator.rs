//! Aria2 Translation Layer — bridges aria2's original file names with user-organized structure.
//!
//! ## Problem
//!
//! aria2 requires original torrent file names to:
//! - Continue incomplete downloads
//! - Seed torrents properly
//!
//! But users want organized file structures like:
//! - `Movies/2024 - Dune/movie.mp4` instead of `Dune.Part.Two.2024.1080p.WEBRip.x264.mkv`
//!
//! ## Solution
//!
//! This layer maintains a mapping between aria2's working directory/files
//! and the user's organized structure:
//!
//! ```text
//! aria2 working dir:  /tmp/stui/downloads/abc123/
//!   |-- Dune.Part.Two.2024.1080p.WEBRip.x264.mkv   <- aria2 sees this
//!   
//! User visible dir:  ~/Videos/Movies/2024 - Dune/
//!   |-- Dune.Part.Two.2024.1080p.WEBRip.x264.mkv   <- symlink or copy
//! ```
//!
//! The mapping is persisted so downloads can be resumed even after restart.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Manages translation between aria2 paths and user-organized paths.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct Aria2Translator {
    /// Maps aria2 GID (download ID) → DownloadSession
    pub sessions: Arc<RwLock<HashMap<String, DownloadSession>>>,
    /// Persisted path for saving/loading translations
    persist_path: PathBuf,
}

impl Aria2Translator {
    pub fn new(persist_path: PathBuf) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persist_path,
        }
    }

    /// Initialize and load persisted translations.
    pub async fn init(&self) -> io::Result<()> {
        if self.persist_path.exists() {
            match fs::read_to_string(&self.persist_path) {
                Ok(data) => {
                    match serde_json::from_str::<PersistedState>(&data) {
                        Ok(state) => {
                            let mut sessions = self.sessions.write().await;
                            for (gid, session) in state.sessions {
                                sessions.insert(gid, session);
                            }
                            info!(count = sessions.len(), "loaded aria2 translations from disk");
                        }
                        Err(e) => {
                            warn!(err = %e, "failed to parse persisted translations, starting fresh");
                        }
                    }
                }
                Err(e) => {
                    warn!(err = %e, "failed to read persisted translations");
                }
            }
        }
        Ok(())
    }

    /// Persist current translations to disk.
    pub async fn persist(&self) -> io::Result<()> {
        let sessions = self.sessions.read().await;
        let state = PersistedState {
            sessions: sessions.clone(),
        };
        
        if let Some(parent) = self.persist_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        let data = serde_json::to_string_pretty(&state)?;
        fs::write(&self.persist_path, data)?;
        debug!(path = %self.persist_path.display(), "persisted aria2 translations");
        Ok(())
    }

    /// Register a new download session before aria2 starts.
    pub async fn register_session(&self, gid: String, session: DownloadSession) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(gid.clone(), session);
        debug!(gid = %gid, "registered aria2 download session");
    }

    /// Get a download session by GID.
    pub async fn get_session(&self, gid: &str) -> Option<DownloadSession> {
        let sessions = self.sessions.read().await;
        sessions.get(gid).cloned()
    }

    /// Update session status and organize files when download completes.
    pub async fn on_download_complete(&self, gid: &str) -> io::Result<()> {
        let mut sessions = self.sessions.write().await;
        
        if let Some(session) = sessions.get_mut(gid) {
            session.status = SessionStatus::Completed;
            
            // Create organized directory structure
            if let Err(e) = self.organize_files(session).await {
                error!(gid = %gid, err = %e, "failed to organize files after download");
                session.status = SessionStatus::OrganizeFailed;
                return Err(e);
            }
            
            debug!(gid = %gid, "download completed and files organized");
        }
        
        drop(sessions);
        self.persist().await
    }

    /// Handle download failure.
    pub async fn on_download_error(&self, gid: &str, error: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(gid) {
            session.status = SessionStatus::Failed;
            session.error_message = Some(error.to_string());
            warn!(gid = %gid, error = %error, "aria2 download failed");
        }
    }

    /// Remove a session (after user removes the download).
    pub async fn remove_session(&self, gid: &str) -> io::Result<()> {
        let mut sessions = self.sessions.write().await;
        
        if let Some(session) = sessions.remove(gid) {
            // Optionally delete the aria2 working directory
            if session.cleanup_on_remove
                && session.aria2_dir.exists() {
                    debug!(gid = %gid, path = %session.aria2_dir.display(), "cleaning up aria2 directory");
                    // Don't delete if files are still being organized
                    if session.status == SessionStatus::OrganizeFailed {
                        warn!(gid = %gid, "skipping cleanup due to previous organize failure");
                    } else {
                        fs::remove_dir_all(&session.aria2_dir).ok();
                    }
                }
        }
        
        drop(sessions);
        self.persist().await
    }

    /// Get all active sessions.
    pub async fn get_active_sessions(&self) -> Vec<DownloadSession> {
        let sessions = self.sessions.read().await;
        sessions.values()
            .filter(|s| s.status == SessionStatus::Active)
            .cloned()
            .collect()
    }

    /// Get the organized path for an aria2 file.
    pub async fn get_organized_path(&self, gid: &str, original_filename: &str) -> Option<PathBuf> {
        let sessions = self.sessions.read().await;
        sessions.get(gid)
            .and_then(|s| s.file_mappings.get(original_filename).cloned())
    }

    /// Organize files from aria2 directory to organized structure.
    /// Uses symlinks to preserve aria2's ability to seed while user sees organized structure.
    async fn organize_files(&self, session: &mut DownloadSession) -> io::Result<()> {
        let organized_base = &session.organized_base;
        
        // Ensure the organized directory exists
        fs::create_dir_all(organized_base)?;
        
        for (original, organized) in &session.file_mappings {
            let original_path = session.aria2_dir.join(original);
            let organized_path = organized_base.join(organized);
            
            if !organized_path.exists() {
                // Create parent directories
                if let Some(parent) = organized_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                
                // Create symlink from organized path to aria2 file
                // This way aria2 continues to work with original names, 
                // but user sees organized structure
                match fs::symlink_metadata(&organized_path) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        // Already linked
                        debug!(path = %organized_path.display(), "symlink already exists");
                    }
                    Ok(_) => {
                        warn!(path = %organized_path.display(), "path exists but is not a symlink");
                    }
                    Err(_) => {
                        // Create relative symlink
                        let relative = Self::relative_path(&organized_path, &original_path);
                        debug!(
                            original = %original_path.display(),
                            organized = %organized_path.display(),
                            relative = %relative.display(),
                            "creating symlink"
                        );
                        #[cfg(unix)]
                        std::os::unix::fs::symlink(&original_path, &organized_path)?;
                        #[cfg(not(unix))]
                        {
                            // On non-Unix, copy the file instead
                            fs::copy(&original_path, &organized_path)?;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Calculate relative path from target to link.
    fn relative_path(target: &Path, link: &Path) -> PathBuf {
        use std::path::Component;
        
        let target_parts: Vec<_> = target.components().collect();
        let link_parts: Vec<_> = link.parent().unwrap_or(Path::new(".")).components().collect();
        
        // Find common prefix
        let common = target_parts.iter()
            .zip(link_parts.iter())
            .take_while(|(a, b)| a == b)
            .count();
        
        let mut result = PathBuf::new();
        
        // Add ../ for each level from link to common ancestor
        for _ in link_parts.iter().skip(common) {
            result.push("..");
        }
        
        // Add remaining parts from target
        for part in target_parts.iter().skip(common) {
            if let Component::Normal(s) = part { result.push(s) }
        }
        
        result
    }
}

/// Represents a single aria2 download session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadSession {
    /// Aria2 GID (Global ID) for this download.
    pub gid: String,
    /// The torrent/magnet name or selected content name.
    pub name: String,
    /// Where aria2 is downloading files to.
    pub aria2_dir: PathBuf,
    /// Where files should be organized to.
    pub organized_base: PathBuf,
    /// Media type for folder structure.
    pub media_type: MediaType,
    /// Optional year for folder naming.
    pub year: Option<u32>,
    /// Map of original filename → organized path (relative to organized_base).
    pub file_mappings: HashMap<String, PathBuf>,
    /// Current status of the session.
    pub status: SessionStatus,
    /// Error message if failed.
    pub error_message: Option<String>,
    /// Whether to clean up aria2 dir when session is removed.
    pub cleanup_on_remove: bool,
    /// Unix timestamp when session was created.
    pub created_at: i64,
}

impl DownloadSession {
    pub fn new(
        gid: String,
        name: String,
        aria2_dir: PathBuf,
        organized_base: PathBuf,
        media_type: MediaType,
        year: Option<u32>,
    ) -> Self {
        Self {
            gid,
            name,
            aria2_dir,
            organized_base,
            media_type,
            year,
            file_mappings: HashMap::new(),
            status: SessionStatus::Active,
            error_message: None,
            cleanup_on_remove: false,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        }
    }

    /// Add a file mapping: original name → organized path.
    pub fn add_file(&mut self, original_filename: &str, organized_relative: &str) {
        self.file_mappings.insert(
            original_filename.to_string(),
            PathBuf::from(organized_relative),
        );
    }
}

/// Status of a download session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    /// Download is in progress.
    Active,
    /// Download completed and files are organized.
    Completed,
    /// Download completed but file organization failed.
    OrganizeFailed,
    /// Download failed.
    Failed,
}

/// Media type for determining folder structure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MediaType {
    Movie,
    Series,
    AnimeMovie,
    AnimeSeries,
    Music,
    Podcast,
}

impl MediaType {
    /// Convert from IPC MediaType to Aria2Translator MediaType.
    pub fn from_ipc(ipc_type: &crate::ipc::MediaType) -> Self {
        use crate::ipc::MediaType as IpcMediaType;
        match ipc_type {
            IpcMediaType::Movie => MediaType::Movie,
            IpcMediaType::Series | IpcMediaType::Episode => MediaType::Series,
            IpcMediaType::Music | IpcMediaType::Album | IpcMediaType::Track => MediaType::Music,
            IpcMediaType::Unknown => MediaType::Movie,
        }
    }
}

/// Persisted state for restoring sessions after restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedState {
    sessions: HashMap<String, DownloadSession>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relative_path() {
        let target = PathBuf::from("/home/user/Videos/Movies/Dune/video.mkv");
        let link = PathBuf::from("/tmp/aria2/file.mkv");
        
        let result = Aria2Translator::relative_path(&target, &link);
        // Result should navigate from /tmp/aria2 to /home/user/Videos/Movies/Dune
        assert!(result.to_string_lossy().contains(".."));
    }

    #[test]
    fn test_session_file_mapping() {
        let mut session = DownloadSession::new(
            "gid123".to_string(),
            "Dune Part Two".to_string(),
            PathBuf::from("/tmp/aria2/gid123"),
            PathBuf::from("/home/user/Videos/Movies/2024 - Dune Part Two"),
            MediaType::Movie,
            Some(2024),
        );
        
        session.add_file("Dune.Part.Two.2024.1080p.mkv", "Dune.Part.Two.2024.1080p.mkv");
        session.add_file("Dune.Part.Two.2024.eng.srt", "Dune.Part.Two.2024.eng.srt");
        
        assert_eq!(session.file_mappings.len(), 2);
    }
}
