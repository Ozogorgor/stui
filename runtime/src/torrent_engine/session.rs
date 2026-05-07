use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use librqbit::Session;

pub struct TorrentSession {
    pub(crate) inner: Arc<Session>,
    pub(crate) staging_dir: PathBuf,
}

impl TorrentSession {
    pub async fn new(staging_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&staging_dir)?;
        let inner = Session::new(staging_dir.clone()).await?;
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
