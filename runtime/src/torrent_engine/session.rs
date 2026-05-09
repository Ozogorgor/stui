use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use librqbit::{Session, SessionOptions, SessionPersistenceConfig};

pub struct TorrentSession {
    pub(crate) inner: Arc<Session>,
    pub(crate) staging_dir: PathBuf,
}

impl TorrentSession {
    /// Boot a librqbit session that persists its torrent list across runtime
    /// restarts. Without persistence, every restart hands back an empty
    /// session — re-picking a magnet that was healthy minutes ago then has
    /// to re-fetch metadata from peers from a cold DHT, which routinely
    /// hits our 60 s `start_stream` timeout. Persisting to
    /// `<staging>/.session.json` lets `add_or_reuse_handle` find the
    /// already-managed torrent and short-circuit `add_torrent` entirely.
    pub async fn new(staging_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&staging_dir)?;
        let opts = SessionOptions {
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(staging_dir.clone()),
            }),
            ..Default::default()
        };
        let inner = Session::new_with_opts(staging_dir.clone(), opts).await?;
        Ok(Self { inner, staging_dir })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_boots_in_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let s = TorrentSession::new(tmp.path().to_path_buf()).await.unwrap();
        assert_eq!(s.staging_dir, tmp.path());
    }
}
