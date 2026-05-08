//! Server-initiated events carried over the same IPC connection as
//! request/response. The client routes events to per-query subscribers
//! (or other targeted consumers).
//!
//! # Transport integration
//!
//! This codebase has *no* outer `Frame` envelope type — `Request` and
//! `Response` are serialized directly as top-level NDJSON objects
//! discriminated by a `"type"` field.  Server-initiated push messages
//! follow the same convention: each `Event` variant serializes to a
//! standalone JSON object (with `"type": "<variant>"`) that is written
//! to the shared `event_tx: mpsc::Sender<String>` channel and drained
//! by the `select!` loop in `main::run_ipc_loop`.
//!
//! # Adding a new event
//!
//! Create a typed payload struct (here or in a sibling module), add a
//! variant to `Event`, add a `"type"` alias in the Go client
//! (`tui/internal/ipc/ipc.go`), and push via [`emit`].

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::ScopeResultsMsg;

/// All server-initiated event payloads.
///
/// Serialized with `#[serde(tag = "type", rename_all = "snake_case")]`
/// so each variant produces a JSON object like
/// `{"type":"scope_results","query_id":7,...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Emitted by `Request::Search` as each scope finalizes or hits a
    /// partial deadline.  Task 2.3 defines the final `ScopeResultsMsg`
    /// shape; Task 2.7 produces the messages.
    ScopeResults(ScopeResultsMsg),
}

/// Sender handle given to handlers that need to stream events back to the
/// client outside the normal request/response flow.
///
/// The channel carries pre-serialized, newline-terminated NDJSON lines —
/// the same format used by all other push events (player progress,
/// grid updates, …).  The `run_ipc_loop` select! arm drains this channel
/// and writes to the wire unchanged, so no additional framing is needed.
pub type EventSender = mpsc::Sender<String>;

/// Serialize `event` to a wire line and send it via `tx`.
///
/// A dropped channel (client disconnected) is logged as a warning and
/// silently swallowed — callers must not treat it as fatal.
pub async fn emit(tx: &EventSender, event: Event) {
    match serde_json::to_string(&event) {
        Ok(mut wire) => {
            wire.push('\n');
            if let Err(e) = tx.send(wire).await {
                tracing::warn!(error = %e, "dropped IPC event (client channel closed)");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to serialize IPC event — event dropped");
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::ipc::v1::{MediaTab, MediaType, ScopeResultsMsg};
    use stui_plugin_sdk::SearchScope;

    /// Build a minimal `ScopeResultsMsg` for tests — avoids repeating all fields.
    fn minimal_scope_msg(query_id: u64) -> ScopeResultsMsg {
        ScopeResultsMsg {
            query_id,
            scope: SearchScope::Track,
            entries: vec![],
            partial: false,
            error: None,
        }
    }

    #[test]
    fn event_serializes_scope_results_type_tag() {
        let event = Event::ScopeResults(minimal_scope_msg(42));
        let s = serde_json::to_string(&event).unwrap();
        // Must carry the inner discriminant tag "scope_results"
        assert!(
            s.contains("\"type\":\"scope_results\""),
            "inner event tag missing: {s}"
        );
        // Must carry the payload field
        assert!(s.contains("\"query_id\":42"), "query_id missing: {s}");
    }

    #[test]
    fn event_round_trips_scope_results() {
        let event = Event::ScopeResults(minimal_scope_msg(7));
        let bytes = serde_json::to_vec(&event).unwrap();
        let back: Event = serde_json::from_slice(&bytes).unwrap();
        match back {
            Event::ScopeResults(m) => assert_eq!(m.query_id, 7),
        }
    }

    #[tokio::test]
    async fn emit_delivers_wire_line_to_channel() {
        let (tx, mut rx) = mpsc::channel::<String>(4);
        emit(&tx, Event::ScopeResults(minimal_scope_msg(1))).await;
        let line = rx.recv().await.expect("should receive a line");
        assert!(line.ends_with('\n'), "wire line must end with newline");
        assert!(
            line.contains("\"type\":\"scope_results\""),
            "wire line must contain event tag: {line}"
        );
        assert!(
            line.contains("\"query_id\":1"),
            "wire line must contain payload: {line}"
        );
    }

    #[tokio::test]
    async fn emit_on_closed_channel_does_not_panic() {
        let (tx, rx) = mpsc::channel::<String>(1);
        drop(rx); // close the receiver
                  // Must not panic; warning is logged internally
        emit(&tx, Event::ScopeResults(minimal_scope_msg(99))).await;
    }
}
