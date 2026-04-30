package ipc

import (
	"encoding/json"
	"fmt"
	"time"

	"github.com/stui/stui/pkg/log"
)

// IPC request ceiling. Sized to cover stream-resolution responses
// (Jackett/Prowlarr Torznab fan-outs across many indexers legitimately
// take 20-30 s), with a few seconds of headroom so the user sees the
// runtime's "no streams found" response rather than an IPC timeout.
const defaultRequestTimeout = 60 * time.Second

// ping sends a versioned handshake ping and validates the response.
func (c *Client) ping() (bool, error) {
	id := c.nextID()
	ch := c.sendWithID(id, map[string]any{
		"type":        "ping",
		"ipc_version": IPCVersion,
	})
	resp := <-ch
	if resp.Err != nil {
		return false, resp.Err
	}
	if resp.Type != "pong" {
		return false, fmt.Errorf("expected pong, got %q", resp.Type)
	}

	var pong struct {
		IPCVersion     uint32 `json:"ipc_version"`
		RuntimeVersion string `json:"runtime_version"`
		VersionOK      bool   `json:"version_ok"`
	}
	if err := resp.decodeData(&pong); err != nil {
		return false, fmt.Errorf("ipc: failed to decode pong data: %w", err)
	}
	c.NegotiatedIPCVersion = pong.IPCVersion
	c.RuntimeVersion = pong.RuntimeVersion
	return pong.VersionOK, nil
}

func (c *Client) nextID() string {
	n := c.reqIDSeq.Add(1)
	return fmt.Sprintf("req-%d", n)
}

// sendWithID registers a pending channel keyed by id and sends the payload.
// The id is automatically added to the payload so the runtime can echo it back.
// Returns a channel that will receive the response.
func (c *Client) sendWithID(id string, payload map[string]any) <-chan RawResponse {
	ch := make(chan RawResponse, 1)
	c.mu.Lock()
	c.pending[id] = ch
	c.mu.Unlock()

	// Ensure id is in payload for runtime correlation
	payload["id"] = id

	if err := c.sendRaw(payload); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		ch <- RawResponse{Err: err}
		close(ch)
	}

	return ch
}

func (c *Client) sendRaw(payload map[string]any) error {
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	data = append(data, '\n')
	c.mu.Lock()
	defer c.mu.Unlock()
	_, err = c.stdin.Write(data)
	if err != nil {
		c.logger.Error("failed to write to runtime stdin", "error", err)
	}
	return err
}

// readLoop continuously reads response lines from the runtime's stdout.
//
// Wrapped in a panic recovery: a panic here (e.g. from a malformed
// payload that slips past json.Unmarshal into a downstream handler)
// would otherwise kill the goroutine silently and freeze the entire
// IPC pipeline — pending requests would block forever and no future
// runtime messages would reach the TUI. Instead we log the panic,
// surface it as a RuntimeErrorMsg, and drain pending channels so
// callers unblock with an explicit error rather than hanging.
func (c *Client) readLoop() {
	logger := log.NewIPCLogger().With("component", "ipc.readLoop")
	defer func() {
		if r := recover(); r != nil {
			logger.Error("read loop panicked", "panic", fmt.Sprintf("%v", r))
			c.send(RuntimeErrorMsg{Err: fmt.Errorf("ipc: read loop panic: %v", r)})
			c.drainPending(fmt.Errorf("ipc: read loop panic: %v", r))
		}
	}()
	for c.stdout.Scan() {
		line := c.stdout.Bytes()
		if len(line) == 0 {
			continue
		}

		var env struct {
			Type string  `json:"type"`
			ID   *string `json:"id"`
		}
		if err := json.Unmarshal(line, &env); err != nil {
			logger.Warn("failed to parse response envelope", "error", err)
			c.send(StatusMsg{Text: fmt.Sprintf("ipc: bad response: %v", err)})
			continue
		}

		logger.Debug("received response", "type", env.Type, "id", env.ID)

		raw := RawResponse{Type: env.Type, Raw: json.RawMessage(append([]byte{}, line...))}

		routed := false
		if env.ID != nil {
			c.mu.Lock()
			ch, ok := c.pending[*env.ID]
			if ok {
				delete(c.pending, *env.ID)
				c.mu.Unlock()
				ch <- raw
				close(ch)
				routed = true
			} else {
				c.mu.Unlock()
			}
		}

		if !routed {
			if env.ID != nil {
				// Has an ID but no matching pending request — log as unexpected.
				c.mu.Lock()
				pendingCount := len(c.pending)
				c.mu.Unlock()
				logger.Warn("response ID has no matching pending request, dispatching as unsolicited",
					"type", env.Type,
					"id", *env.ID,
					"pending_count", pendingCount)
			}
			c.dispatchUnsolicited(raw)
		}
	}

	if err := c.stdout.Err(); err != nil && c.ctx.Err() == nil {
		logger.Error("runtime stdout closed unexpectedly", "error", err)
		c.send(RuntimeErrorMsg{Err: fmt.Errorf("ipc: runtime stdout closed: %w", err)})
	}
	logger.Info("read loop terminated")

	// Drain all pending response channels so goroutines waiting on them
	// don't block forever now that the runtime is gone.
	c.drainPending(fmt.Errorf("ipc: runtime process exited"))
}

// drainPending unblocks all callers that are waiting for a response by
// sending them an error and then clearing the pending map.
// Must be called after the readLoop exits.
func (c *Client) drainPending(err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for id, ch := range c.pending {
		ch <- RawResponse{Err: err}
		close(ch)
		delete(c.pending, id)
	}
}

func (c *Client) dispatchUnsolicited(raw RawResponse) {
	switch raw.Type {
	case "grid_update":
		var msg GridUpdateMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse grid_update", "error", err)
		} else {
			c.send(msg)
		}
	case "catalog_stale":
		var msg CatalogStaleMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse catalog_stale", "error", err)
		} else {
			c.send(msg)
		}
	case "plugin_toast":
		var msg PluginToastMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse plugin_toast", "error", err)
		} else {
			c.send(msg)
		}
	case "subtitle_fetched":
		var msg SubtitleFetchedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse subtitle_fetched", "error", err)
		} else {
			c.send(msg)
		}
	case "subtitle_search_failed":
		var msg SubtitleSearchFailedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse subtitle_search_failed", "error", err)
		} else {
			c.send(msg)
		}
	case "theme_update":
		var msg ThemeUpdateMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse theme_update", "error", err)
		} else {
			c.send(msg)
		}
	case "player_started":
		var msg PlayerStartedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_started", "error", err)
		} else {
			c.send(msg)
		}
	case "player_progress":
		var msg PlayerProgressMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_progress", "error", err)
		} else {
			c.send(msg)
		}
	case "player_ended":
		var msg PlayerEndedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_ended", "error", err)
		} else {
			c.send(msg)
		}
	case "player_terminal_takeover":
		var msg PlayerTerminalTakeoverMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_terminal_takeover", "error", err)
		} else {
			c.send(msg)
		}
	case "player_tracks_updated":
		var msg PlayerTracksUpdatedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_tracks_updated", "error", err)
		} else {
			c.send(msg)
		}
	case "download_started":
		var msg DownloadStartedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse download_started", "error", err)
		} else {
			c.send(msg)
		}
	case "download_progress":
		var msg DownloadProgressMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse download_progress", "error", err)
		} else {
			c.send(msg)
		}
	case "download_complete":
		var msg DownloadCompleteMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse download_complete", "error", err)
		} else {
			c.send(msg)
		}
	case "download_error":
		var msg DownloadErrorMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse download_error", "error", err)
		} else {
			c.send(msg)
		}
	case "player_buffering":
		var msg PlayerBufferingMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_buffering", "error", err)
		} else {
			c.send(msg)
		}
	case "player_buffer_ready":
		var msg PlayerBufferReadyMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse player_buffer_ready", "error", err)
		} else {
			c.send(msg)
		}
	case "queue_update":
		var msg QueueUpdateMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse queue_update", "error", err)
		} else {
			c.send(msg)
		}
	case "catalog_loaded":
		var msg CatalogLoadedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse catalog_loaded", "error", err)
		} else {
			c.send(msg)
		}
	case "skip_segment":
		var msg SkipSegmentMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse skip_segment", "error", err)
		} else {
			c.send(msg)
		}
	case "mpd_queue_changed":
		c.send(MpdQueueChangedMsg{})
	case "mpd_status":
		var msg MpdStatusMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse mpd_status", "error", err)
		} else {
			c.send(msg)
		}
	case "mpd_outputs_result":
		var msg MpdOutputsResultMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse mpd_outputs_result", "error", err)
		} else {
			c.send(msg)
		}
	case "dsp_status":
		var msg DspStatusMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse dsp_status", "error", err)
		} else {
			c.send(msg)
		}
	case "dsp_bound_to_mpd":
		var msg DspBoundToMpdMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse dsp_bound_to_mpd", "error", err)
		} else {
			c.send(msg)
		}
	case "scope_results":
		var msg ScopeResultsMsg
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse scope_results", "error", err)
		} else {
			c.dispatchScopeResults(msg)
		}
	case "detail_metadata_partial":
		// Streamed per-verb partial from a pending GetDetailMetadata
		// request. Arrives without a request ID — matches by EntryID
		// inside the Model's Update handler.
		var msg DetailMetadataPartial
		if err := json.Unmarshal(raw.Raw, &msg); err != nil {
			c.logger.Warn("failed to parse detail_metadata_partial", "error", err)
		} else {
			c.send(msg)
		}
	case "streams_partial":
		// Per-provider chunk from an in-flight find_streams. The TUI
		// appends to its per-(season, episode) cache; the spinner
		// clears on `streams_complete`.
		var wire struct {
			EntryID  string       `json:"entry_id"`
			Season   uint32       `json:"season"`
			Episode  uint32       `json:"episode"`
			Provider string       `json:"provider"`
			Streams  []StreamInfo `json:"streams"`
		}
		if err := json.Unmarshal(raw.Raw, &wire); err != nil {
			c.logger.Warn("failed to parse streams_partial", "error", err)
		} else {
			c.send(EpisodeStreamsPartialMsg{
				EntryID:  wire.EntryID,
				Season:   int(wire.Season),
				Episode:  int(wire.Episode),
				Provider: wire.Provider,
				Streams:  wire.Streams,
			})
		}
	case "streams_complete":
		var wire struct {
			EntryID string  `json:"entry_id"`
			Season  uint32  `json:"season"`
			Episode uint32  `json:"episode"`
			Error   *string `json:"error"`
		}
		if err := json.Unmarshal(raw.Raw, &wire); err != nil {
			c.logger.Warn("failed to parse streams_complete", "error", err)
		} else {
			errStr := ""
			if wire.Error != nil {
				errStr = *wire.Error
			}
			c.send(EpisodeStreamsCompleteMsg{
				EntryID: wire.EntryID,
				Season:  int(wire.Season),
				Episode: int(wire.Episode),
				Err:     errStr,
			})
		}
	}
}
