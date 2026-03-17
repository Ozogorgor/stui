//! Central media domain module.
//!
//! Every part of the runtime — providers, catalog, resolver, player, plugins —
//! speaks in terms of these types.  Nothing should invent its own media struct.
//!
//! Hierarchy:
//!   MediaItem      — the atom returned by any catalog/search
//!   MediaId        — opaque, namespaced identifier
//!   MediaType      — coarse classification
//!   EpisodeInfo    — attaches to MediaType::Episode
//!   TrackInfo      — attaches to MediaType::Track / Album

pub mod id;
pub mod item;
pub mod episode;
pub mod source;
pub mod stream;
pub mod track;

pub use id::MediaId;
pub use item::MediaItem;
pub use episode::EpisodeInfo;
pub use track::TrackInfo;
pub use source::MediaSource;
pub use stream::{StreamCandidate, StreamProtocol, BundledSubtitle};

// Re-export MediaType here so the rest of the codebase imports from one place.
// ipc.rs keeps its own copy for wire-format stability; media/ is the canonical
// in-memory version.
pub use crate::ipc::MediaType;
