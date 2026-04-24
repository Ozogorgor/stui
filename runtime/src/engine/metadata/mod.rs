//! Metadata enrichment pipeline sub-modules.
//!
//! * [`sources`] — resolves the ordered list of plugins that can serve a
//!   given (verb, kind) pair.
//! * [`merge`] — pure functions that merge per-verb responses from
//!   multiple plugins into a single canonical payload (Chunk 4.2 / 4.3).
//!
//! The top-level orchestrator (`fetch_detail_metadata`) lands in this
//! module once Task 4.3 implements it.

pub mod sources;
pub mod merge;
