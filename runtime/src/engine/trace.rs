//! Live-to-stderr structured pipeline trace.
//!
//! `TraceEmitter` writes `[trace] stage: detail` lines to stderr (or an
//! injected writer in tests) as each pipeline stage completes.
//! All methods are no-ops when the emitter is disabled.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct TraceEmitter {
    enabled: AtomicBool,
    // Mutex required: Write::write takes &mut self; Mutex gives interior mutability.
    writer: Mutex<Box<dyn Write + Send>>,
}

impl Default for TraceEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceEmitter {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            writer: Mutex::new(Box::new(std::io::stderr())),
        }
    }

    /// Construct with an injected writer — used in tests to capture output.
    /// Starts disabled; caller must call `.enable()` separately.
    pub fn with_writer(w: Box<dyn Write + Send>) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            writer: Mutex::new(w),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn emit(&self, line: &str) {
        if !self.is_enabled() {
            return;
        }
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "[trace] {}", line);
        }
    }

    // ── Stage helpers ──────────────────────────────────────────────────────

    pub fn search(&self, n_providers: usize, elapsed_ms: u64) {
        self.emit(&format!("search: {} providers ({}ms)", n_providers, elapsed_ms));
    }

    pub fn resolve(&self, n_streams: usize) {
        self.emit(&format!("resolve: {} streams", n_streams));
    }

    pub fn bench(&self, n_tested: usize) {
        self.emit(&format!("bench: {} tested", n_tested));
    }

    pub fn rank(&self, position: usize, score: f64) {
        self.emit(&format!("rank: picked #{} (score {:.2})", position, score));
    }

    pub fn fallback(&self, reason: &str) {
        self.emit(&format!("fallback: {}", reason));
    }

    pub fn provider_error(&self, name: &str, reason: &str) {
        self.emit(&format!("provider: {} failed ({})", name, reason));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buf_emitter() -> (TraceEmitter, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let buf_clone = std::sync::Arc::clone(&buf);
        struct BufWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for BufWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let emitter = TraceEmitter::with_writer(Box::new(BufWriter(buf_clone)));
        (emitter, buf)
    }

    fn read_buf(buf: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn disabled_by_default_no_output() {
        let (emitter, buf) = make_buf_emitter();
        emitter.search(3, 100);
        emitter.resolve(12);
        emitter.bench(8);
        emitter.rank(1, 0.82);
        emitter.fallback("timeout");
        emitter.provider_error("prov", "http 503");
        assert_eq!(read_buf(&buf), "", "disabled emitter must produce no output");
    }

    #[test]
    fn enable_sets_flag() {
        let emitter = TraceEmitter::new();
        assert!(!emitter.is_enabled());
        emitter.enable();
        assert!(emitter.is_enabled());
    }

    #[test]
    fn search_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.search(3, 120);
        assert_eq!(read_buf(&buf).trim(), "[trace] search: 3 providers (120ms)");
    }

    #[test]
    fn resolve_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.resolve(12);
        assert_eq!(read_buf(&buf).trim(), "[trace] resolve: 12 streams");
    }

    #[test]
    fn bench_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.bench(8);
        assert_eq!(read_buf(&buf).trim(), "[trace] bench: 8 tested");
    }

    #[test]
    fn rank_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.rank(4, 0.82);
        assert_eq!(read_buf(&buf).trim(), "[trace] rank: picked #4 (score 0.82)");
    }

    #[test]
    fn fallback_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.fallback("timeout");
        assert_eq!(read_buf(&buf).trim(), "[trace] fallback: timeout");
    }

    #[test]
    fn provider_error_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.provider_error("yts", "http 503");
        assert_eq!(read_buf(&buf).trim(), "[trace] provider: yts failed (http 503)");
    }

    #[test]
    fn no_streams_fallback_format() {
        let (emitter, buf) = make_buf_emitter();
        emitter.enable();
        emitter.fallback("no streams after bench");
        assert_eq!(read_buf(&buf).trim(), "[trace] fallback: no streams after bench");
    }
}
