//! Plugin supervisor — automatic restart, backoff, crash-loop detection,
//! and memory-usage monitoring for external plugin processes.
//!
//! # Restart policy
//!
//! When a plugin exits unexpectedly the supervisor re-spawns it with
//! exponential backoff starting at 1 s and capping at 60 s.
//!
//! If the plugin crashes more than `max_restarts` times within
//! `crash_window_secs` seconds the supervisor marks it **permanently failed**
//! and stops attempting restarts.  The `PluginRpcManager` will stop routing
//! calls to it, and a warning is logged so the user can investigate.
//!
//! # Resource monitoring
//!
//! When `max_memory_mb` is set the supervisor polls `/proc/{pid}/status`
//! every 10 seconds.  If VmRSS exceeds the limit the process is killed and
//! a normal restart cycle begins.
//!
//! # Resource limits
//!
//! - `max_memory_mb`: Maximum resident memory (RSS) in MB
//! - `cpu_nice_value`: Scheduling priority (`nice` level) for the plugin process;
//!   lowers OS scheduling priority so the plugin yields CPU to other processes.
//!   Does **not** enforce a hard CPU cap. 0 = no adjustment.
//! - `request_timeout_ms`: Timeout for individual RPC calls in milliseconds

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{Mutex, RwLock};
use tokio::time::timeout;
use tracing::{error, info, warn};

use super::process::PluginProcess;
use super::protocol::{PluginHandshake, RpcMediaItem, RpcStream, RpcSubtitleTrack};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Tunable parameters for a single plugin supervisor instance.
#[derive(Debug, Clone)]
#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
pub struct SupervisorConfig {
    /// Maximum number of restarts allowed within `crash_window_secs`.
    pub max_restarts: u32,
    /// Sliding window (seconds) for counting crashes.
    pub crash_window_secs: u64,
    /// Initial backoff delay (milliseconds) before the first restart.
    pub backoff_base_ms: u64,
    /// Maximum backoff delay (milliseconds).
    pub backoff_max_ms: u64,
    /// Kill the plugin if its resident memory (VmRSS) exceeds this many MB.
    /// `None` disables memory monitoring.
    pub max_memory_mb: Option<u64>,
    /// `nice` scheduling priority adjustment for the plugin process (0 = no adjustment).
    /// On Linux, a positive value lowers scheduling priority so the plugin yields
    /// CPU time to higher-priority processes.  This does **not** enforce a hard
    /// CPU percentage cap — a plugin can still saturate a core when the system is
    /// otherwise idle.  Typical range: 0 (normal) to 19 (lowest priority).
    pub cpu_nice_value: u32,
    /// Timeout for individual RPC calls in milliseconds.
    /// If a call takes longer, it returns an error.
    #[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
    pub request_timeout_ms: u64,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        SupervisorConfig {
            max_restarts: 5,
            crash_window_secs: 60,
            backoff_base_ms: 1_000,
            backoff_max_ms: 60_000,
            max_memory_mb: Some(512),
            cpu_nice_value: 0,
            request_timeout_ms: 30_000,
        }
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────────

/// Live health snapshot for a supervised plugin.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
pub struct SupervisorStats {
    /// Number of times the plugin has crashed since load.
    pub crash_count: u32,
    /// Number of successful restarts.
    pub restart_count: u32,
    /// Whether a plugin process is currently alive.
    pub is_alive: bool,
    /// Whether the supervisor has given up (crash loop detected).
    pub permanently_failed: bool,
    /// Current memory usage in MB (0 if unknown).
    pub memory_mb: u64,
    /// Number of requests that timed out.
    pub timeout_count: u32,
}

// ── Supervisor ────────────────────────────────────────────────────────────────

/// Wraps a `PluginProcess` with automatic restart and resource monitoring.
///
/// All RPC methods delegate to the currently live `PluginProcess`.
/// If the process is mid-restart callers receive an immediate error rather
/// than blocking.
pub struct PluginSupervisor {
    /// Path to the plugin executable — used for respawns.
    pub bin: PathBuf,
    config: SupervisorConfig,
    /// The currently live process, or `None` while restarting.
    process: Arc<RwLock<Option<Arc<PluginProcess>>>>,
    /// Cached capability / handshake info from the last successful spawn.
    pub info: Arc<RwLock<PluginHandshake>>,
    stats: Arc<Mutex<SupervisorStats>>,
    /// Set to `true` when the crash loop threshold is reached.
    failed: Arc<AtomicBool>,
}

#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
impl PluginSupervisor {
    /// Spawn the plugin and start supervising it.
    #[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
    pub async fn spawn(bin: PathBuf, config: SupervisorConfig) -> Result<Self> {
        let proc = PluginProcess::spawn(bin.clone()).await?;

        // Apply CPU limit if configured; failure is non-fatal (plugin still starts).
        if config.cpu_nice_value > 0 {
            if let Some(pid) = proc.pid {
                let _ = apply_nice_priority(pid, config.cpu_nice_value);
            }
        }

        let info = proc.info.clone();

        let process = Arc::new(RwLock::new(Some(Arc::new(proc))));
        let info_arc = Arc::new(RwLock::new(info));
        let stats = Arc::new(Mutex::new(SupervisorStats {
            is_alive: true,
            ..Default::default()
        }));
        let failed = Arc::new(AtomicBool::new(false));

        let s = PluginSupervisor {
            bin,
            config,
            process: Arc::clone(&process),
            info: Arc::clone(&info_arc),
            stats: Arc::clone(&stats),
            failed: Arc::clone(&failed),
        };

        // Start the watchdog background task.
        s.start_watchdog();

        Ok(s)
    }

    /// Snapshot of the current health metrics.
    #[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
    pub async fn stats(&self) -> SupervisorStats {
        self.stats.lock().await.clone()
    }

    /// `true` if the supervisor has permanently given up on this plugin.
    #[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// `true` if the plugin advertises the given capability.
    #[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
    pub async fn has_capability(&self, cap: &str) -> bool {
        self.info.read().await.capabilities.iter().any(|c| c == cap)
    }

    /// Gracefully shut down the plugin and stop the supervisor.
    #[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
    pub async fn shutdown(&self) {
        self.failed.store(true, Ordering::Relaxed); // prevent restart after shutdown
        if let Some(proc) = self.process.read().await.as_ref().cloned() {
            proc.shutdown().await;
        }
    }

    // ── Delegating RPC methods ────────────────────────────────────────────

    pub async fn catalog_search(
        &self,
        query: &str,
        tab: &str,
        page: u32,
    ) -> Result<Vec<RpcMediaItem>> {
        let timeout_duration = Duration::from_millis(self.config.request_timeout_ms);
        let result = timeout(
            timeout_duration,
            self.with_process(|p| async move { p.catalog_search(query, tab, page).await }),
        )
        .await;

        match result {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let mut s = self.stats.lock().await;
                s.timeout_count += 1;
                Err(anyhow::anyhow!(
                    "catalog_search timed out after {}ms",
                    self.config.request_timeout_ms
                ))
            }
        }
    }

    pub async fn streams_resolve(&self, id: &str) -> Result<Vec<RpcStream>> {
        let timeout_duration = Duration::from_millis(self.config.request_timeout_ms);
        let result = timeout(
            timeout_duration,
            self.with_process(|p| async move { p.streams_resolve(id).await }),
        )
        .await;

        match result {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let mut s = self.stats.lock().await;
                s.timeout_count += 1;
                Err(anyhow::anyhow!(
                    "streams_resolve timed out after {}ms",
                    self.config.request_timeout_ms
                ))
            }
        }
    }

    pub async fn subtitles_fetch(&self, id: &str) -> Result<Vec<RpcSubtitleTrack>> {
        let timeout_duration = Duration::from_millis(self.config.request_timeout_ms);
        let result = timeout(
            timeout_duration,
            self.with_process(|p| async move { p.subtitles_fetch(id).await }),
        )
        .await;

        match result {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let mut s = self.stats.lock().await;
                s.timeout_count += 1;
                Err(anyhow::anyhow!(
                    "subtitles_fetch timed out after {}ms",
                    self.config.request_timeout_ms
                ))
            }
        }
    }

    // ── Internals ─────────────────────────────────────────────────────────

    async fn with_process<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(Arc<PluginProcess>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let proc = self.process.read().await.as_ref().cloned();
        match proc {
            Some(p) => f(p).await,
            None => anyhow::bail!("plugin '{}' is restarting", self.bin.display()),
        }
    }

    /// Spawn the watchdog task that monitors the process for death and
    /// optionally for memory overuse.
    fn start_watchdog(&self) {
        let process = Arc::clone(&self.process);
        let info_arc = Arc::clone(&self.info);
        let stats = Arc::clone(&self.stats);
        let failed = Arc::clone(&self.failed);
        let bin = self.bin.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            // Track crash timestamps within the sliding window.
            let mut crash_times: Vec<Instant> = vec![];
            let mut backoff_ms = config.backoff_base_ms;

            loop {
                // Wait for either the process to die or a memory overuse event.
                let death_notify = {
                    let guard = process.read().await;
                    match guard.as_ref() {
                        Some(p) => Arc::clone(&p.death_notify),
                        None => break, // supervisor shut down
                    }
                };

                // Concurrently: wait for death OR poll memory every 10 s.
                let memory_killed = tokio::select! {
                    _ = death_notify.notified() => false,
                    killed = poll_memory_loop(&process, &stats, config.max_memory_mb) => killed,
                };

                if failed.load(Ordering::Relaxed) {
                    // Supervisor shut down intentionally — do not restart.
                    break;
                }

                // ── Crash accounting ──────────────────────────────────────
                let now = Instant::now();
                let window = Duration::from_secs(config.crash_window_secs);
                crash_times.retain(|t| now.duration_since(*t) < window);
                crash_times.push(now);

                {
                    let mut s = stats.lock().await;
                    s.crash_count += 1;
                    s.is_alive = false;
                }

                if crash_times.len() > config.max_restarts as usize {
                    error!(
                        plugin = %bin.display(),
                        crashes = crash_times.len(),
                        window  = config.crash_window_secs,
                        "plugin crash loop detected — giving up"
                    );
                    failed.store(true, Ordering::Relaxed);
                    stats.lock().await.permanently_failed = true;
                    *process.write().await = None;
                    break;
                }

                let reason = if memory_killed {
                    "memory limit exceeded"
                } else {
                    "unexpected exit"
                };
                warn!(plugin = %bin.display(), reason, backoff_ms, "plugin died — restarting");

                // Clear the dead process slot while we respawn.
                *process.write().await = None;

                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(config.backoff_max_ms);

                // Re-check after sleeping: shutdown() may have been called while
                // we were waiting. Without this check we would spawn a new process
                // after a graceful shutdown and then immediately orphan it.
                if failed.load(Ordering::Relaxed) {
                    break;
                }

                match PluginProcess::spawn(bin.clone()).await {
                    Ok(proc) => {
                        // Re-apply CPU limit after restart
                        if config.cpu_nice_value > 0 {
                            if let Some(pid) = proc.pid {
                                let _ = apply_nice_priority(pid, config.cpu_nice_value);
                            }
                        }
                        info!(plugin = %proc.info.name, version = %proc.info.version, "plugin restarted");
                        *info_arc.write().await = proc.info.clone();
                        let mut s = stats.lock().await;
                        s.restart_count += 1;
                        s.is_alive = true;
                        *process.write().await = Some(Arc::new(proc));
                        // Reset backoff on a successful start.
                        backoff_ms = config.backoff_base_ms;
                    }
                    Err(e) => {
                        error!(plugin = %bin.display(), err = %e, "plugin respawn failed");
                        // Will loop back and try again after another backoff.
                    }
                }
            }
        });
    }
}

// ── Memory monitoring ─────────────────────────────────────────────────────────

/// Poll the process's resident memory every 10 seconds.
/// Returns `true` if the process was killed for exceeding `max_memory_mb`.
/// Updates `memory_mb` in stats on each poll.
/// Returns `false` if the memory limit is not set (future: process died normally).
#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
async fn poll_memory_loop(
    process: &Arc<RwLock<Option<Arc<PluginProcess>>>>,
    stats: &Arc<Mutex<SupervisorStats>>,
    max_memory_mb: Option<u64>,
) -> bool {
    let Some(limit_mb) = max_memory_mb else {
        std::future::pending::<()>().await;
        return false;
    };

    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;

        let pid = {
            let guard = process.read().await;
            guard.as_ref().and_then(|p| p.pid)
        };

        if let Some(pid) = pid {
            if let Some(rss_mb) = read_proc_rss_mb(pid) {
                {
                    let mut s = stats.lock().await;
                    s.memory_mb = rss_mb;
                }

                if rss_mb > limit_mb {
                    warn!(
                        pid,
                        rss_mb, limit_mb, "plugin exceeded memory limit — killing"
                    );
                    let _ = nix_kill(pid);
                    return true;
                }
            }
        }
    }
}

/// Read the resident set size in MB from `/proc/{pid}/status` (Linux only).
#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
#[cfg(target_os = "linux")]
fn read_proc_rss_mb(pid: u32) -> Option<u64> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in content.lines() {
        // VmRSS:    12345 kB
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
#[cfg(not(target_os = "linux"))]
fn read_proc_rss_mb(_pid: u32) -> Option<u64> {
    None
}

/// Send SIGKILL to a process by PID.
#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
fn nix_kill(pid: u32) -> std::io::Result<()> {
    // Safety: we only send SIGKILL (9) which is always safe.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

// ── Scheduling priority ────────────────────────────────────────────────────────
// Adjusts the OS scheduling nice value — lowers priority so the plugin yields
// CPU time to other processes. Does NOT enforce a hard CPU cap.

#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
#[cfg(target_os = "linux")]
fn apply_nice_priority(pid: u32, cpu_nice_value: u32) -> Result<()> {
    use std::process::Command;

    if cpu_nice_value == 0 || cpu_nice_value > 100 {
        return Ok(());
    }

    let nice_value: i32 = if cpu_nice_value >= 80 {
        0
    } else if cpu_nice_value >= 60 {
        5
    } else if cpu_nice_value >= 40 {
        10
    } else if cpu_nice_value >= 20 {
        15
    } else {
        19
    };

    let output = Command::new("renice")
        .args(["-n", &nice_value.to_string(), "-p", &pid.to_string()])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            info!(pid, nice_value, "applied CPU limit via nice");
            Ok(())
        }
        Ok(out) => {
            warn!(pid, stderr = ?String::from_utf8_lossy(&out.stderr), "renice failed");
            Err(anyhow::anyhow!("renice failed"))
        }
        Err(e) => {
            warn!(pid, err = %e, "failed to execute renice");
            Err(anyhow::anyhow!("failed to execute renice: {}", e))
        }
    }
}

#[allow(dead_code)] // planned: plugin RPC supervisor pub API, called via PluginRpcManager
#[cfg(not(target_os = "linux"))]
fn apply_nice_priority(_pid: u32, _cpu_nice_value: u32) -> Result<()> {
    Ok(())
}
