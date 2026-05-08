//! MPD (Music Player Daemon) integration for audio playback.
//!
//! stui routes `Music`, `Radio`, and `Podcasts` tabs through MPD while
//! `Movies`, `Series`, and `Videos` continue to use MPV.
//!
//! # Design
//!
//! Two TCP connections per runtime:
//! - **command connection** — lazy, reconnects on failure, used for all
//!   playback control (add, play, pause, seek, volume, replay gain, outputs)
//! - **idle connection**    — permanent; blocks on `idle player mixer options`;
//!   fires `mpd_status` push events to the TUI on every state change
//!
//! # Remote MPD servers
//!
//! Vanilla stui expects a localhost MPD instance.  Remote MPD servers
//! (NAS, Raspberry Pi, Volumio) are handled via RPC plugins that return
//! an `mpd+tcp://host:port` URL; the local bridge dials that host instead.

pub mod bridge;
pub mod client;
pub mod mpd_conf;
mod search;

pub use bridge::MpdBridge;
