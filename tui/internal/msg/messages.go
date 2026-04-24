// Package msg provides the canonical BubbleTea message types for stui.
//
// # Architecture
//
// Message types are defined once in the [ipc] package, where they are decoded
// from the Rust runtime's NDJSON wire protocol. This package re-exports every
// type as a Go type alias so that non-IPC code (state management, tests,
// future UI components) can import messages without pulling in the full IPC
// client.
//
// Because these are true type aliases (using =), msg.FooMsg and ipc.FooMsg
// are identical types — a value of one satisfies a switch case for the other.
// No conversion or wrapping is needed.
//
// # Usage
//
//   import "github.com/stui/stui/internal/msg"
//
//   case msg.PlayerStartedMsg:     // identical to ipc.PlayerStartedMsg
//   case msg.GridUpdateMsg:
//
// # Adding a new message
//
//  1. Add the struct to ipc/ipc.go (where it will be decoded from JSON).
//  2. Add a type alias here so the msg package stays in sync.
//  3. Handle it in ui/ui.go's Update() switch.
package msg

import "github.com/stui/stui/internal/ipc"

// ── Runtime lifecycle ─────────────────────────────────────────────────────────

// RuntimeReadyMsg is sent once the runtime process has started and the IPC
// connection is established.
type RuntimeReadyMsg = ipc.RuntimeReadyMsg

// RuntimeErrorMsg wraps a fatal error during runtime startup or IPC operation.
type RuntimeErrorMsg = ipc.RuntimeErrorMsg

// ── Catalog / search ──────────────────────────────────────────────────────────

// SearchResultMsg carries the result of a single-response search request.
// Retained for the person-mode search path; see ipc.SearchResultMsg.
type SearchResultMsg = ipc.SearchResultMsg

// GridUpdateMsg is pushed by the runtime whenever catalog data changes
// (cache hit on startup, live provider refresh, or search results).
type GridUpdateMsg = ipc.GridUpdateMsg

// CatalogLoadedMsg signals that the initial catalog population is complete
// for a given tab — useful for hiding loading spinners.
type CatalogLoadedMsg = ipc.CatalogLoadedMsg

// ── Detail / metadata ─────────────────────────────────────────────────────────

// DetailReadyMsg carries fully-enriched metadata for the detail overlay.
type DetailReadyMsg = ipc.DetailReadyMsg

// PersonSearchMsg is dispatched internally when the user activates a cast link.
type PersonSearchMsg = ipc.PersonSearchMsg

// ── Plugin management ─────────────────────────────────────────────────────────

// PluginListMsg carries the current snapshot of all loaded plugins.
type PluginListMsg = ipc.PluginListMsg

// PluginLoadedMsg signals a plugin was successfully loaded or hot-reloaded.
type PluginLoadedMsg = ipc.PluginLoadedMsg

// PluginToastMsg is pushed by the runtime on hot-load / hot-unload events.
type PluginToastMsg = ipc.PluginToastMsg

// ── Player ────────────────────────────────────────────────────────────────────

// PlayerStartedMsg is pushed when mpv has launched and begun playing.
type PlayerStartedMsg = ipc.PlayerStartedMsg

// PlayerProgressMsg is pushed approximately once per second during playback.
type PlayerProgressMsg = ipc.PlayerProgressMsg

// PlayerEndedMsg is pushed when playback finishes or mpv exits.
type PlayerEndedMsg = ipc.PlayerEndedMsg

// PlayerBufferingMsg is pushed during initial pre-roll or a stall-guard pause.
// The TUI renders a progress bar from FillPercent and EtaSecs.
type PlayerBufferingMsg = ipc.PlayerBufferingMsg

// PlayerBufferReadyMsg is pushed when pre-roll or stall-guard recovery finishes
// and playback is about to resume.
type PlayerBufferReadyMsg = ipc.PlayerBufferReadyMsg

// QueueUpdateMsg is pushed whenever the player queue length changes.
type QueueUpdateMsg = ipc.QueueUpdateMsg

// ── Theme ─────────────────────────────────────────────────────────────────────

// ThemeUpdateMsg is pushed by the Rust runtime whenever matugen rewrites its
// colors.json, triggering a live palette hot-swap in the TUI.
type ThemeUpdateMsg = ipc.ThemeUpdateMsg

// ── UI signals ────────────────────────────────────────────────────────────────

// StatusMsg carries a short status string for the status bar.
// Used for transient feedback that doesn't warrant a full toast notification.
type StatusMsg = ipc.StatusMsg

// ── Stream resolution ─────────────────────────────────────────────────────────

// StreamsResolvedMsg carries resolved stream candidates.
type StreamsResolvedMsg = ipc.StreamsResolvedMsg

// StreamInfo describes a single resolved stream candidate.
type StreamInfo = ipc.StreamInfo

// EpisodesLoadedMsg carries episode metadata for a season.
type EpisodesLoadedMsg = ipc.EpisodesLoadedMsg
