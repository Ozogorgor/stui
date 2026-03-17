//! `EventBus` — a broadcast channel that decouples runtime modules.
//!
//! # Usage
//!
//! ```rust
//! // In startup code:
//! let bus = Arc::new(EventBus::new());
//!
//! // Emitting (any module with a bus handle):
//! bus.emit(RuntimeEvent::SearchRequested {
//!     query: "Interstellar".into(),
//!     tab:   "movies".into(),
//! });
//!
//! // Subscribing (e.g. in a background task):
//! let mut rx = bus.subscribe();
//! tokio::spawn(async move {
//!     while let Ok(event) = rx.recv().await {
//!         match event {
//!             RuntimeEvent::PlaybackStarted { title, .. } => {
//!                 // scrobble to Trakt, update history, etc.
//!             }
//!             _ => {}
//!         }
//!     }
//! });
//! ```
//!
//! # Backpressure
//!
//! `tokio::sync::broadcast` drops messages for slow receivers rather than
//! blocking the sender.  Slow subscribers will see `RecvError::Lagged`.
//! The capacity (256) is intentionally generous but bounded — adjust via
//! `EventBus::with_capacity` for high-throughput scenarios.
//!
//! # Logging subscriber
//!
//! Call `EventBus::spawn_logger` to attach a background task that logs
//! every event at TRACE level.  Useful during development.

use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::trace;

use super::event::RuntimeEvent;

/// Default broadcast channel capacity.
const DEFAULT_CAPACITY: usize = 256;

/// A lightweight, cloneable handle to the runtime event bus.
///
/// Clone freely — all clones share the same underlying channel.
#[derive(Clone)]
pub struct EventBus {
    sender: Arc<broadcast::Sender<RuntimeEvent>>,
}

impl EventBus {
    /// Create a new bus with the default capacity (256 slots).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a bus with a custom channel capacity.
    pub fn with_capacity(cap: usize) -> Self {
        let (sender, _) = broadcast::channel(cap);
        EventBus { sender: Arc::new(sender) }
    }

    /// Emit an event to all current subscribers.
    ///
    /// Silently drops the send if there are no subscribers.
    /// Never blocks or returns an error to the caller.
    pub fn emit(&self, event: RuntimeEvent) {
        trace!(event = event.name(), "event emitted");
        let _ = self.sender.send(event);
    }

    /// Subscribe to the bus.  Each subscriber gets its own receiver.
    ///
    /// Subscribers created *after* an event is emitted will not receive
    /// that event (broadcast semantics).
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }

    /// Number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Spawn a background task that logs every event at TRACE level.
    ///
    /// Returns immediately; the task runs until the bus is dropped.
    /// Call this once during startup when `STUI_LOG=trace` is set.
    pub fn spawn_logger(&self) {
        let mut rx = self.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        trace!(event = event.name(), "runtime event");
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "event bus: subscriber lagged, events dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

// ── Convenience: watch a single event type ────────────────────────────────────

/// A guard returned by `EventBus::on_playback_started`.
/// Drop it to stop listening.
pub struct EventListener {
    _handle: tokio::task::JoinHandle<()>,
}

impl EventBus {
    /// Run `handler` in a background task for every `PlaybackStarted` event.
    ///
    /// Returns an `EventListener`; drop it to cancel.
    pub fn on_playback_started<F>(&self, mut handler: F) -> EventListener
    where
        F: FnMut(String, String, f64) + Send + 'static,
    {
        let mut rx = self.subscribe();
        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(RuntimeEvent::PlaybackStarted { title, url, duration }) => {
                        handler(title, url, duration);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        });
        EventListener { _handle: handle }
    }

    /// Run `handler` in a background task for every `ProviderError` event.
    pub fn on_provider_error<F>(&self, mut handler: F) -> EventListener
    where
        F: FnMut(String, String) + Send + 'static,
    {
        let mut rx = self.subscribe();
        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(RuntimeEvent::ProviderError { provider, message }) => {
                        handler(provider, message);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        });
        EventListener { _handle: handle }
    }
}
