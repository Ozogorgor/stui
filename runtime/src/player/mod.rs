//! Player module — mpv subprocess management and playback orchestration.
//!
//! # Structure
//!
//! ```text
//! player/
//!   mod.rs      - this file, public re-exports
//!   state.rs    - PlaybackState: authoritative model of what mpv is doing
//!   commands.rs - PlayerCommand: typed control API (pause/seek/sub-delay/...)
//!   mpv.rs      - MpvPlayer: spawn, IPC socket, event broadcast
//!   bridge.rs   - PlayerBridge: route stream_url to the torrent engine or mpv
//!   manager.rs  - PlayerManager: queue, candidates, command dispatch
//! ```
//!
//! # Playback pipeline
//!
//! ```text
//! PlayerManager.handle_command(cmd)
//!   → dispatches to MpvPlayer typed methods
//!       → mpv IPC socket → mpv
//!
//! PlayerManager.play_item(item)
//!   → PlayerBridge.play(url, title, sub)
//!       → streamer::wait_for_preroll()   (torrent only)
//!       → MpvPlayer.play(file_or_url)
//!           → mpv IPC → events → EventBus → Go TUI
//! ```
//!
//! # State flow
//!
//! ```text
//! mpv property-change events
//!   → MpvEvent::Progress
//!   → PlayerManager updates PlaybackState
//!   → EventBus::PlaybackProgress
//!   → IPC serialises PlaybackState snapshot
//!   → Go TUI renders HUD
//! ```

pub mod bridge;
pub mod commands;
pub mod manager;
pub mod mpv;
pub mod state;

pub use bridge::PlayerBridge;
#[allow(unused_imports)]
pub use commands::PlayerCommand;
#[allow(unused_imports)]
pub use manager::{PlaybackRecord, PlayerManager, QueueEntry};
#[allow(unused_imports)]
pub use mpv::{MpvPlayer, PlayerStartedEvent};
#[allow(unused_imports)]
pub use state::PlaybackState;
