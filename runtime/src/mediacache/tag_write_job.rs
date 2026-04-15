//! Tag-write job: builds a diff, applies writes concurrently, supports cancel.
//!
//! Separation of concerns:
//!   - `build_diff`: pure. Turns (file, RawTags) into DiffRow skipping no-ops.
//!   - `to_wire_rows`: serialize DiffRows as one-row-per-changed-field.
//!   - `apply`: concurrent write execution. Reports (succeeded, failed, skipped_cancelled).
//!   - `common_ancestor`: for scoping MPD rescan.
//!   - `JobStore` / `JobRegistry`: per-job state and cancellation flags.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ipc::v1::TagDiffRowWire;
use crate::mediacache::normalize::{self, lookup::LookupResult, NormalizationConfig, RawTags};
use crate::mediacache::tag_writer;

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub file: PathBuf,
    pub raw: RawTags,
    pub normalized: normalize::NormalizedTags,
}

/// Skip rows where the pipeline output equals the raw input byte-for-byte.
pub fn build_diff(
    files: Vec<(PathBuf, RawTags)>,
    cfg: &NormalizationConfig,
    lookups: &HashMap<PathBuf, LookupResult>,
) -> Vec<DiffRow> {
    let mut out = Vec::with_capacity(files.len());
    for (file, raw) in files {
        let lookup = lookups.get(&file);
        let normalized = normalize::normalize(&raw, cfg, lookup);
        if normalized_equals_raw(&normalized, &raw) { continue; }
        out.push(DiffRow { file, raw, normalized });
    }
    out
}

fn normalized_equals_raw(n: &normalize::NormalizedTags, r: &RawTags) -> bool {
    n.artist == r.artist
        && n.album_artist == r.album_artist
        && n.album == r.album
        && n.title == r.title
        && n.year == normalize::year::extract_year(&r.date)
        && n.genre == r.genre
        && n.track == normalize::rules::parse_track_or_disc(&r.track)
        && n.disc == normalize::rules::parse_track_or_disc(&r.disc)
}

pub fn to_wire_rows(rows: &[DiffRow]) -> Vec<TagDiffRowWire> {
    let mut out = Vec::new();
    for row in rows {
        let f = row.file.to_string_lossy().to_string();
        push_if_diff(&mut out, &f, "artist", &row.raw.artist, &row.normalized.artist);
        push_if_diff(&mut out, &f, "album_artist", &row.raw.album_artist, &row.normalized.album_artist);
        push_if_diff(&mut out, &f, "album", &row.raw.album, &row.normalized.album);
        push_if_diff(&mut out, &f, "title", &row.raw.title, &row.normalized.title);
        push_if_diff(&mut out, &f, "year", &row.raw.date, &row.normalized.year);
        push_if_diff(&mut out, &f, "genre", &row.raw.genre, &row.normalized.genre);
    }
    out
}

fn push_if_diff(out: &mut Vec<TagDiffRowWire>, file: &str, field: &str, old: &str, new: &str) {
    if old != new {
        out.push(TagDiffRowWire {
            file: file.to_string(),
            field: field.to_string(),
            old_value: old.to_string(),
            new_value: new.to_string(),
        });
    }
}

pub fn common_ancestor(files: &[PathBuf]) -> Option<PathBuf> {
    let mut iter = files.iter();
    let first = iter.next()?.clone();
    // Seed with the first file's parent (or itself if no parent).
    let mut common: PathBuf = first.parent().map(|p| p.to_path_buf()).unwrap_or(first.clone());
    for f in iter {
        while !f.starts_with(&common) {
            if !common.pop() { return None; }
        }
    }
    Some(common)
}

// ── Job state ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApplyOutcome {
    pub succeeded: usize,
    pub failed: Vec<PathBuf>,
    pub skipped_cancelled: usize,
}

#[derive(Default)]
pub struct JobStore {
    inner: Mutex<HashMap<String, Vec<DiffRow>>>,
}
impl JobStore {
    pub fn new() -> Self { Self::default() }
    pub fn insert(&self, id: String, rows: Vec<DiffRow>) {
        self.inner.lock().unwrap().insert(id, rows);
    }
    pub fn take(&self, id: &str) -> Option<Vec<DiffRow>> {
        self.inner.lock().unwrap().remove(id)
    }
}

#[derive(Default)]
pub struct JobRegistry {
    flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
}
impl JobRegistry {
    pub fn new() -> Self { Self::default() }
    pub fn register(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.flags.lock().unwrap().insert(id.to_string(), flag.clone());
        flag
    }
    pub fn cancel(&self, id: &str) -> bool {
        if let Some(flag) = self.flags.lock().unwrap().get(id) {
            flag.store(true, Ordering::Relaxed); true
        } else { false }
    }
    pub fn done(&self, id: &str) {
        self.flags.lock().unwrap().remove(id);
    }
}

// ── Apply ──────────────────────────────────────────────────────────────────

pub type ProgressSender = tokio::sync::mpsc::UnboundedSender<ApplyProgress>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApplyProgress {
    Started { job_id: String, total: usize },
    FileDone { job_id: String, file: String, ok: bool },
    Finished { job_id: String, outcome: ApplyOutcome },
}

/// Execute a diff set: writes normalized tags, concurrency capped at 4.
/// Respects the cancellation flag: files whose write has not started when
/// cancellation is observed are marked as skipped_cancelled, NOT failed.
pub async fn apply(
    job_id: String,
    rows: Vec<DiffRow>,
    cancel_flag: Arc<AtomicBool>,
    progress: Option<ProgressSender>,
) -> ApplyOutcome {
    use tokio::sync::Semaphore;
    let sem = Arc::new(Semaphore::new(4));
    let total = rows.len();
    if let Some(tx) = &progress {
        let _ = tx.send(ApplyProgress::Started { job_id: job_id.clone(), total });
    }

    let mut handles = Vec::with_capacity(total);
    for row in rows {
        let sem = sem.clone();
        let cancel = cancel_flag.clone();
        let progress = progress.clone();
        let job_id_c = job_id.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            if cancel.load(Ordering::Relaxed) {
                return FileResult::Cancelled(row.file);
            }
            let file_c = row.file.clone();
            let nd = row.normalized.clone();
            let write_res = tokio::task::spawn_blocking(move || {
                tag_writer::write_normalized(&row.file, &nd)
            }).await.unwrap();

            let ok = write_res.is_ok();
            if let Some(tx) = progress {
                let _ = tx.send(ApplyProgress::FileDone {
                    job_id: job_id_c,
                    file: file_c.to_string_lossy().to_string(),
                    ok,
                });
            }
            match write_res {
                Ok(_) => FileResult::Ok(file_c),
                Err(_) => FileResult::Failed(file_c),
            }
        }));
    }

    let mut outcome = ApplyOutcome { succeeded: 0, failed: Vec::new(), skipped_cancelled: 0 };
    for h in handles {
        match h.await.unwrap() {
            FileResult::Ok(_) => outcome.succeeded += 1,
            FileResult::Failed(p) => outcome.failed.push(p),
            FileResult::Cancelled(_) => outcome.skipped_cancelled += 1,
        }
    }
    if let Some(tx) = progress {
        let _ = tx.send(ApplyProgress::Finished { job_id, outcome: outcome.clone() });
    }
    outcome
}

enum FileResult { Ok(PathBuf), Failed(PathBuf), Cancelled(PathBuf) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_ancestor_single_file() {
        let f = PathBuf::from("/music/a/b.mp3");
        assert_eq!(common_ancestor(&[f]).unwrap(), PathBuf::from("/music/a"));
    }
    #[test]
    fn common_ancestor_same_dir() {
        let a = PathBuf::from("/music/rock/a.mp3");
        let b = PathBuf::from("/music/rock/b.mp3");
        assert_eq!(common_ancestor(&[a, b]).unwrap(), PathBuf::from("/music/rock"));
    }
    #[test]
    fn common_ancestor_divergent() {
        let a = PathBuf::from("/music/rock/a.mp3");
        let b = PathBuf::from("/music/pop/b.mp3");
        assert_eq!(common_ancestor(&[a, b]).unwrap(), PathBuf::from("/music"));
    }
    #[test]
    fn build_diff_skips_noop() {
        let ex = normalize::exceptions::ExceptionList::default();
        let cfg = NormalizationConfig { enabled: true, use_lookup: false, exceptions: &ex };
        let files = vec![(
            PathBuf::from("a.mp3"),
            RawTags { artist: "Pink Floyd".into(), album: "The Wall".into(), ..Default::default() },
        )];
        let lookups = HashMap::new();
        assert!(build_diff(files, &cfg, &lookups).is_empty());
    }
    #[test]
    fn build_diff_keeps_change() {
        let ex = normalize::exceptions::ExceptionList::default();
        let cfg = NormalizationConfig { enabled: true, use_lookup: false, exceptions: &ex };
        let files = vec![(
            PathBuf::from("a.mp3"),
            RawTags { artist: "pink floyd".into(), ..Default::default() },
        )];
        let lookups = HashMap::new();
        let diff = build_diff(files, &cfg, &lookups);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].normalized.artist, "Pink Floyd");
    }
}
