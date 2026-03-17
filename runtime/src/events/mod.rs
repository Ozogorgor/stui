//! Runtime event system — decouples modules via a central broadcast bus.
//!
//! # Module structure
//!
//! ```
//! events/
//!   mod.rs    ← this file; re-exports EventBus and RuntimeEvent
//!   event.rs  ← RuntimeEvent enum (all variants)
//!   bus.rs    ← EventBus: emit(), subscribe(), spawn_logger()
//! ```
//!
//! # Architecture overview
//!
//! Instead of modules calling each other directly, they emit events onto
//! the bus and subscribe to events they care about:
//!
//! ```text
//! ┌─────────┐   emit     ┌──────────┐   subscribe  ┌────────┐
//! │ engine  │──────────▶ │ EventBus │ ◀────────────│ player │
//! └─────────┘            └──────────┘              └────────┘
//!                              │
//!                    subscribe │
//!                   ┌──────────┴──────────┐
//!                   ▼                     ▼
//!               ┌───────┐           ┌──────────┐
//!               │  ipc  │           │  cache   │
//!               └───────┘           └──────────┘
//! ```
//!
//! Benefits:
//! - No circular imports (every module only depends on `events`)
//! - Easy to add new listeners without touching emitters
//! - Trivial to add scrobbling, history, analytics, or plugin hooks
//!
//! # Wiring
//!
//! Create one `EventBus` in `main.rs` and inject it into subsystems:
//!
//! ```rust
//! let bus = Arc::new(EventBus::new());
//! bus.spawn_logger(); // enable with STUI_LOG=trace
//!
//! let pipeline = Pipeline::new(&cfg, providers, player, Arc::clone(&bus));
//! ```

pub mod bus;
pub mod event;

pub use bus::EventBus;
pub use event::RuntimeEvent;
