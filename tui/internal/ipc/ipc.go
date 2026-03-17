// Package ipc implements the Go side of the stui IPC bridge.
//
// Transport: newline-delimited JSON over the stdin/stdout of a
// stui-runtime child process.
//
//	Go TUI  ──(Request \n)──▶  stui-runtime
//	Go TUI  ◀──(Response \n)── stui-runtime
//
// Usage:
//
//	client, err := ipc.Start("/usr/local/bin/stui-runtime")
//	defer client.Stop()
//
//	// Send a request and get a response channel
//	ch := client.Send(ipc.SearchRequest{...})
//	resp := <-ch
package ipc

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
	"sync/atomic"

	"github.com/charmbracelet/bubbletea"
)

// ── Protocol versioning ───────────────────────────────────────────────────────

// IPCVersion is the TUI's protocol version number. Must be bumped together
// with CURRENT_VERSION in runtime/src/ipc/mod.rs when introducing breaking
// changes to the wire protocol.
const IPCVersion = 1

// ── Wire types (mirror Rust ipc.rs) ─────────────────────────────────────────

// MediaTab matches the Rust MediaTab enum
type MediaTab string

const (
	TabMovies  MediaTab = "movies"
	TabSeries  MediaTab = "series"
	TabMusic   MediaTab = "music"
	TabLibrary MediaTab = "library"
)

// Request types ───────────────────────────────────────────────────

type requestEnvelope struct {
	Type string `json:"type"`
	// Fields are inlined via embedding or map merge at send time
	Data map[string]any `json:"-"`
}

// Response types ──────────────────────────────────────────────────

// RawResponse is the partially-decoded response from the runtime.
// The UI layer further decodes the payload based on Type.
type RawResponse struct {
	Type string          `json:"type"`
	Raw  json.RawMessage // the full original JSON, for further decoding
	Err  error           // set if the transport itself failed
}

func (r RawResponse) IsError() bool {
	return r.Type == "error" || r.Err != nil
}

// decodeData unmarshals the raw response body into v.
func (r RawResponse) decodeData(v any) error {
	if r.Err != nil {
		return r.Err
	}
	return json.Unmarshal(r.Raw, v)
}

// Concrete response payload types

type SearchResult struct {
	ID     string       `json:"id"`
	Items  []MediaEntry `json:"items"`
	Total  int          `json:"total"`
	Offset int          `json:"offset"`
}

type MediaEntry struct {
	ID          string   `json:"id"`
	Title       string   `json:"title"`
	Year        *string  `json:"year"`
	Genre       *string  `json:"genre"`
	Rating      *string  `json:"rating"`
	Description *string  `json:"description"`
	PosterURL   *string  `json:"poster_url"`
	Provider    string   `json:"provider"`
	Tab         MediaTab `json:"tab"`
}

type PluginInfo struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	Version    string `json:"version"`
	PluginType string `json:"plugin_type"`
	Status     string `json:"status"`
}

type PluginListResult struct {
	Plugins []PluginInfo `json:"plugins"`
}

type ErrorPayload struct {
	ID      *string `json:"id"`
	Code    string  `json:"code"`
	Message string  `json:"message"`
}

// ── Bubble Tea messages ──────────────────────────────────────────────────────
// These are dispatched into the Bubble Tea event loop so the UI can react
// to async runtime responses without polling.

// RuntimeReadyMsg is sent once the runtime process has started and
// responded to the initial ping.
type RuntimeReadyMsg struct {
	// RuntimeVersion is the semver string from the runtime binary (e.g. "0.8.1").
	RuntimeVersion string
	// IPCVersion is the protocol version the runtime reported.
	IPCVersion uint32
}

// IPCVersionMismatchMsg is dispatched when the runtime's ipc_version differs
// from IPCVersion. The TUI continues to run but should display a warning.
type IPCVersionMismatchMsg struct {
	TUIVersion     uint32
	RuntimeVersion uint32
	RuntimeSemver  string
}

// RuntimeErrorMsg wraps a fatal IPC or runtime error.
type RuntimeErrorMsg struct{ Err error }

// SearchResultMsg carries the result of a search request.
type SearchResultMsg struct {
	ReqID  string
	Result SearchResult
	Err    error
}

// PluginListMsg carries the current plugin list.
type PluginListMsg struct {
	Plugins []PluginInfo
	Err     error
}

// PluginLoadedMsg signals a plugin was loaded.
type PluginLoadedMsg struct {
	PluginID string
	Name     string
	Err      error
}

// StatusMsg carries a generic status string for display in the status bar.
type StatusMsg struct{ Text string }

// ── Client ───────────────────────────────────────────────────────────────────

// Client manages the stui-runtime child process and all IPC with it.
type Client struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	stdout *bufio.Scanner

	mu       sync.Mutex
	pending  map[string]chan RawResponse // req id → response channel
	reqIDSeq atomic.Uint64

	program *tea.Program // for dispatching BubbleTea msgs
	ctx     context.Context
	cancel  context.CancelFunc
	once    sync.Once // ensures Stop is idempotent

	// Populated after a successful handshake.
	RuntimeVersion     string // semver from the runtime binary, e.g. "0.8.1"
	NegotiatedIPCVersion uint32 // ipc_version echoed back in the pong
}

// Start spawns the stui-runtime binary and performs a handshake ping.
// runtimePath is the path to the stui-runtime binary.
// program is the active Bubble Tea program (used to dispatch async messages).
func Start(runtimePath string, program *tea.Program) (*Client, error) {
	ctx, cancel := context.WithCancel(context.Background())

	cmd := exec.CommandContext(ctx, runtimePath)
	cmd.Stderr = nil // runtime writes logs to its own stderr — let them through

	stdin, err := cmd.StdinPipe()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("ipc: stdin pipe: %w", err)
	}

	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("ipc: stdout pipe: %w", err)
	}

	if err := cmd.Start(); err != nil {
		cancel()
		return nil, fmt.Errorf("ipc: start runtime: %w", err)
	}

	c := &Client{
		cmd:     cmd,
		stdin:   stdin,
		stdout:  bufio.NewScanner(stdoutPipe),
		pending: make(map[string]chan RawResponse),
		program: program,
		ctx:     ctx,
		cancel:  cancel,
	}

	// Start the read loop in a goroutine
	go c.readLoop()

	// Handshake — send a versioned ping, confirm the runtime is alive
	versionOK, err := c.ping()
	if err != nil {
		c.Stop()
		return nil, fmt.Errorf("ipc: handshake ping failed: %w", err)
	}
	if !versionOK && program != nil {
		program.Send(IPCVersionMismatchMsg{
			TUIVersion:     IPCVersion,
			RuntimeVersion: c.NegotiatedIPCVersion,
			RuntimeSemver:  c.RuntimeVersion,
		})
	}

	return c, nil
}

// Stop shuts down the runtime process gracefully.
func (c *Client) Stop() {
	c.once.Do(func() {
		// Best-effort graceful shutdown request
		_ = c.sendRaw(map[string]any{"type": "shutdown"})
		c.cancel()
		_ = c.stdin.Close()
		_ = c.cmd.Wait()
	})
}

// ── Public request methods ───────────────────────────────────────────────────

// Search sends a search request and dispatches a SearchResultMsg to the
// Bubble Tea program when the response arrives.
func (c *Client) Search(reqID, query string, tab MediaTab, limit, offset int) {
	go func() {
		ch := c.sendWithID(reqID, map[string]any{
			"type":   "search",
			"id":     reqID,
			"query":  query,
			"tab":    string(tab),
			"limit":  limit,
			"offset": offset,
		})
		raw := <-ch
		msg := decodeSearchResult(reqID, raw)
		c.program.Send(msg)
	}()
}

// UnloadPlugin removes a plugin from the running engine.
// The plugin files remain on disk; the engine hot-reloads on the next scan.
// A PluginListMsg refresh is sent to the program when the call completes.
func (c *Client) UnloadPlugin(pluginID string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":      "unload_plugin",
			"plugin_id": pluginID,
		})
		<-ch // discard response; hot-reload watcher handles the state change
		// Trigger a list refresh so the UI shows the updated state.
		c.ListPlugins()
	}()
}

// LoadPlugin sends a load_plugin request.
func (c *Client) LoadPlugin(path string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "load_plugin",
			"path": path,
		})
		raw := <-ch
		var msg PluginLoadedMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var p struct {
				PluginID string `json:"plugin_id"`
				Name     string `json:"name"`
			}
			_ = json.Unmarshal(raw.Raw, &p)
			msg.PluginID = p.PluginID
			msg.Name = p.Name
		}
		c.program.Send(msg)
	}()
}

// ListPlugins requests the current plugin list.
func (c *Client) ListPlugins() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "list_plugins"})
		raw := <-ch
		var msg PluginListMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			_ = json.Unmarshal(raw.Raw, &msg)
		}
		c.program.Send(msg)
	}()
}

// ── Internal ─────────────────────────────────────────────────────────────────

// ping sends a versioned handshake ping and validates the response.
// Returns (versionOK, error). versionOK is false when the runtime's
// ipc_version differs from IPCVersion (non-fatal; caller dispatches a warning).
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

	// Decode version metadata from the pong body.
	var pong struct {
		IPCVersion     uint32 `json:"ipc_version"`
		RuntimeVersion string `json:"runtime_version"`
		VersionOK      bool   `json:"version_ok"`
	}
	if err := resp.decodeData(&pong); err == nil {
		c.NegotiatedIPCVersion = pong.IPCVersion
		c.RuntimeVersion = pong.RuntimeVersion
		return pong.VersionOK, nil
	}

	// Old runtime with unit Pong — no version info, assume compatible.
	c.NegotiatedIPCVersion = IPCVersion
	return true, nil
}

func (c *Client) nextID() string {
	n := c.reqIDSeq.Add(1)
	return fmt.Sprintf("req-%d", n)
}

// sendWithID registers a pending channel keyed by id and sends the payload.
// For requests without a correlation id (ping, list_plugins), we use a
// generated id that the runtime won't echo — we match by arrival order.
func (c *Client) sendWithID(id string, payload map[string]any) <-chan RawResponse {
	ch := make(chan RawResponse, 1)
	c.mu.Lock()
	c.pending[id] = ch
	c.mu.Unlock()

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
	return err
}

// readLoop continuously reads response lines from the runtime's stdout.
func (c *Client) readLoop() {
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
			c.program.Send(StatusMsg{Text: fmt.Sprintf("ipc: bad response: %v", err)})
			continue
		}

		raw := RawResponse{Type: env.Type, Raw: json.RawMessage(append([]byte{}, line...))}

		// Try to route to a pending channel by correlation id
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

		// For responses without a correlation id (pong, ok, plugin_list, etc.)
		// route to the oldest pending channel (FIFO).
		if !routed {
			c.mu.Lock()
			for k, ch := range c.pending {
				delete(c.pending, k)
				c.mu.Unlock()
				ch <- raw
				close(ch)
				routed = true
				break
			}
			if !routed {
				c.mu.Unlock()
			}
		}

		// If nothing was pending, broadcast as a BubbleTea message directly
		if !routed {
			c.dispatchUnsolicited(raw)
		}
	}

	if err := c.stdout.Err(); err != nil && c.ctx.Err() == nil {
		c.program.Send(RuntimeErrorMsg{Err: fmt.Errorf("ipc: runtime stdout closed: %w", err)})
	}
}

func (c *Client) dispatchUnsolicited(raw RawResponse) {
	switch raw.Type {
	case "grid_update":
		var msg GridUpdateMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "plugin_toast":
		var msg PluginToastMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "theme_update":
		var msg ThemeUpdateMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_started":
		var msg PlayerStartedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_progress":
		var msg PlayerProgressMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_ended":
		var msg PlayerEndedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_terminal_takeover":
		var msg PlayerTerminalTakeoverMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_tracks_updated":
		var msg PlayerTracksUpdatedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "download_started":
		var msg DownloadStartedMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "download_progress":
		var msg DownloadProgressMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "download_complete":
		var msg DownloadCompleteMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "download_error":
		var msg DownloadErrorMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_buffering":
		var msg PlayerBufferingMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "player_buffer_ready":
		var msg PlayerBufferReadyMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}
	case "queue_update":
		var m QueueUpdateMsg
		if err := json.Unmarshal(raw.Raw, &m); err == nil {
			c.program.Send(m)
		}
	case "streams_resolved":
		// Runtime pushed resolved stream candidates (e.g. after a resolve request)
		var resp struct {
			EntryID string       `json:"entry_id"`
			Streams []StreamInfo `json:"streams"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err == nil {
			c.program.Send(StreamsResolvedMsg{EntryID: resp.EntryID, Streams: resp.Streams})
		}

	case "config_updated":
		// Acknowledge that a SetConfig request was applied
		var resp struct {
			Key string `json:"key"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err == nil {
			c.program.Send(StatusMsg{Text: "config updated: " + resp.Key})
		}

	case "provider_rate_limited":
		// A provider hit its rate limit — show a toast
		var resp struct {
			Provider        string `json:"provider"`
			RetryAfterSecs  int    `json:"retry_after_secs"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err == nil {
			msg := fmt.Sprintf("%s rate limited — retry in %ds", resp.Provider, resp.RetryAfterSecs)
			c.program.Send(StatusMsg{Text: msg})
		}

	case "skip_segment":
		var msg SkipSegmentMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}

	case "mpd_status":
		var msg MpdStatusMsg
		if err := json.Unmarshal(raw.Raw, &msg); err == nil {
			c.program.Send(msg)
		}

	case "mpd_outputs":
		var payload struct {
			Outputs []MpdOutput `json:"outputs"`
		}
		if err := json.Unmarshal(raw.Raw, &payload); err == nil {
			c.program.Send(MpdOutputsResultMsg{Outputs: payload.Outputs})
		}

	case "mpd_queue_changed":
		c.program.Send(MpdQueueChangedMsg{})

	case "error":
		var ep ErrorPayload
		_ = json.Unmarshal(raw.Raw, &ep)
		c.program.Send(StatusMsg{Text: fmt.Sprintf("runtime error: %s — %s", ep.Code, ep.Message)})
	}
}

// ── Decode helpers ────────────────────────────────────────────────────────────

func decodeSearchResult(reqID string, raw RawResponse) SearchResultMsg {
	if raw.Err != nil {
		return SearchResultMsg{ReqID: reqID, Err: raw.Err}
	}
	if raw.Type == "error" {
		var ep ErrorPayload
		_ = json.Unmarshal(raw.Raw, &ep)
		return SearchResultMsg{ReqID: reqID, Err: fmt.Errorf("%s: %s", ep.Code, ep.Message)}
	}
	var result SearchResult
	if err := json.Unmarshal(raw.Raw, &result); err != nil {
		return SearchResultMsg{ReqID: reqID, Err: err}
	}
	return SearchResultMsg{ReqID: reqID, Result: result}
}

// ── Grid / catalog types ─────────────────────────────────────────────────────

// CatalogEntry is a richer media item with poster data, pushed by the catalog.
type CatalogEntry struct {
	ID          string  `json:"id"`
	Title       string  `json:"title"`
	Year        *string `json:"year"`
	Genre       *string `json:"genre"`
	Rating      *string `json:"rating"`
	Description *string `json:"description"`
	PosterURL   *string `json:"poster_url"`
	PosterArt   *string `json:"poster_art"`
	Provider    string  `json:"provider"`
	Tab         string  `json:"tab"`
	ImdbID      *string `json:"imdb_id"`
}

// GridUpdateMsg is pushed by the runtime whenever catalog data changes.
// Source is "cache" (instant, on launch) or "live" (fresh from providers).
type GridUpdateMsg struct {
	Tab     string         `json:"tab"`
	Entries []CatalogEntry `json:"entries"`
	Source  string         `json:"source"` // "cache" | "live"
}

// ── Plugin toast ─────────────────────────────────────────────────────────────

// PluginToastMsg is pushed by the runtime when a plugin is hot-loaded or
// fails to load. Displayed as a transient notification in the TUI.
type PluginToastMsg struct {
	PluginName string `json:"plugin_name"`
	Version    string `json:"version"`
	PluginType string `json:"plugin_type"`
	Message    string `json:"message"`
	IsError    bool   `json:"is_error"`
}

// ── Detail / metadata types ───────────────────────────────────────────────────

// DetailEntry is the rich metadata for a single title — returned by the
// runtime's metadata endpoint and also assembled client-side from catalog data.
type DetailEntry struct {
	ID          string       `json:"id"`
	Title       string       `json:"title"`
	Year        string       `json:"year"`
	Genre       string       `json:"genre"`
	Rating      string       `json:"rating"`
	Runtime     string       `json:"runtime"`      // e.g. "2h 46m"
	Description string       `json:"description"`
	PosterURL   string       `json:"poster_url"`
	PosterArt   string       `json:"poster_art"`   // pre-rendered block art
	Cast        []CastMember `json:"cast"`
	Provider    string       `json:"provider"`
	Providers   []string     `json:"providers"`
	ImdbID      string       `json:"imdb_id"`
	Tab         string       `json:"tab"`
}

// CastMember is a single person in the cast/crew list.
// Name is "hyperlink-like" — pressing enter triggers a person search.
type CastMember struct {
	Name      string `json:"name"`
	Role      string `json:"role"`       // e.g. "Paul Atreides" or "Director"
	RoleType  string `json:"role_type"`  // "cast" | "crew"
}

// PersonSearchMsg is dispatched when the user activates a cast member link.
// The UI handles it by firing a new search and entering ViewPersonResults.
type PersonSearchMsg struct {
	PersonName string
	FromID     string // detail entry we navigated from (for breadcrumb back)
}

// DetailReadyMsg carries fetched/assembled detail data into the UI.
type DetailReadyMsg struct {
	Entry DetailEntry
	Err   error
}

// SimilarReadyMsg carries similar title results for the bottom row.
type SimilarReadyMsg struct {
	ForID   string
	Entries []CatalogEntry
	Err     error
}

// ── Theme update (from matugen watcher in runtime) ────────────────────────────

// ThemeUpdateMsg is pushed by the Rust runtime whenever matugen rewrites
// its colors.json. Colors is the raw "dark" map from the JSON file —
// keys are Material You role names like "primary", "background", etc.,
// values are hex strings like "#adc6ff".
type ThemeUpdateMsg struct {
	Colors map[string]string `json:"colors"` // M3 role → hex
	Mode   string            `json:"mode"`   // "dark" | "light"
}

// ── Play / player messages ────────────────────────────────────────────────────

// Play sends a play request to the runtime.
// tab is the active media tab — when it's "music", "radio", or "podcasts"
// the runtime routes playback through MPD instead of MPV.
func (c *Client) Play(entryID, provider, imdbID string, tab MediaTab) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":     "play",
			"id":       id,
			"entry_id": entryID,
			"provider": provider,
			"imdb_id":  imdbID,
			"tab":      string(tab),
		}
		_ = c.sendRaw(payload)
	}()
}

// PlayFrom is like Play but resumes from startPos seconds into the stream.
// Pass startPos=0 to start from the beginning.
func (c *Client) PlayFrom(entryID, provider, imdbID string, tab MediaTab, startPos float64) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":           "play",
			"id":             id,
			"entry_id":       entryID,
			"provider":       provider,
			"imdb_id":        imdbID,
			"tab":            string(tab),
			"start_position": startPos,
		}
		_ = c.sendRaw(payload)
	}()
}

// PlayerStop sends a stop command to the runtime (kills mpv + aria2 GID).
func (c *Client) PlayerStop() {
	go func() {
		_ = c.sendRaw(map[string]any{"type": "player_stop"})
	}()
}

// DeleteStream asks the runtime to remove the cached stream files for the
// given catalog entry ID.  Called after natural ("eof") playback completion
// when the user has enabled auto-delete for that media type.
func (c *Client) DeleteStream(entryID string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":     "delete_stream",
			"entry_id": entryID,
		})
	}()
}

// PlayerCommand sends an mpv IPC command (pause, seek, etc.)
func (c *Client) PlayerCommand(cmd string, args ...any) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "player_command",
			"cmd":  cmd,
			"args": args,
		})
	}()
}

// Resolve sends a stream resolution request for entryID.
// When results arrive they are dispatched as StreamsResolvedMsg.
func (c *Client) Resolve(entryID, provider string) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":     "get_streams",
			"id":       id,
			"entry_id": entryID,
		}
		ch := c.sendWithID(id, payload)
		raw := <-ch
		if raw.Err != nil {
			c.program.Send(StatusMsg{Text: "stream resolve failed: " + raw.Err.Error()})
			return
		}
		var resp struct {
			Streams []StreamInfo `json:"streams"`
		}
		if err := raw.decodeData(&resp); err != nil {
			c.program.Send(StatusMsg{Text: "stream decode failed: " + err.Error()})
			return
		}
		c.program.Send(StreamsResolvedMsg{EntryID: entryID, Streams: resp.Streams})
	}()
}

// DownloadStream starts an aria2 download for the given URL without launching
// mpv.  Progress events (DownloadStartedMsg, DownloadProgressMsg, etc.) will
// arrive as unsolicited messages.
func (c *Client) DownloadStream(url, title string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "download_stream",
			"url":   url,
			"title": title,
		})
	}()
}

// CancelDownload asks aria2 to abort the download identified by gid.
func (c *Client) CancelDownload(gid string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "download_cancel",
			"gid":  gid,
		})
	}()
}

// PlayFile launches mpv on a local file path (e.g. a completed download).
func (c *Client) PlayFile(path, title string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "play_file",
			"path":  path,
			"title": title,
		})
	}()
}

// SwitchStream sends a stream-switch command to mpv (loadfile replace).
func (c *Client) SwitchStream(url string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "cmd",
			"cmd":  "switch_stream",
			"url":  url,
		})
	}()
}

// LoadEpisodes requests episode metadata for a series season.
// Results arrive as EpisodesLoadedMsg dispatched to the BubbleTea program.
func (c *Client) LoadEpisodes(seriesID string, season int) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":      "metadata",
			"id":        id,
			"entry_id":  seriesID,
			"kind":      "episodes",
			"season":    season,
		}
		ch := c.sendWithID(id, payload)
		raw := <-ch
		if raw.Err != nil {
			return
		}
		var resp struct {
			Episodes []EpisodeEntry `json:"episodes"`
		}
		if err := raw.decodeData(&resp); err != nil {
			return
		}
		c.program.Send(EpisodesLoadedMsg{
			SeriesID: seriesID,
			Season:   season,
			Episodes: resp.Episodes,
		})
	}()
}

// ── Provider settings ────────────────────────────────────────────────────────

// ProviderField describes one configurable field for a provider (e.g. an API key).
type ProviderField struct {
	Key        string `json:"key"`
	Label      string `json:"label"`
	Hint       string `json:"hint"`
	Masked     bool   `json:"masked"`
	Configured bool   `json:"configured"`
}

// ProviderSchema describes one provider's configuration requirements.
type ProviderSchema struct {
	ID          string          `json:"id"`
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Active      bool            `json:"active"`
	Fields      []ProviderField `json:"fields"`
}

// ProviderSettingsResultMsg is dispatched when GetProviderSettings completes.
type ProviderSettingsResultMsg struct {
	Providers []ProviderSchema
	Err       error
}

// GetProviderSettings requests the full provider configuration schema from
// the runtime. The result arrives as a ProviderSettingsResultMsg.
func (c *Client) GetProviderSettings() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_provider_settings"})
		raw := <-ch
		var msg ProviderSettingsResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Providers []ProviderSchema `json:"providers"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Providers = payload.Providers
			}
		}
		c.program.Send(msg)
	}()
}

// ── Plugin repos ─────────────────────────────────────────────────────────────

// PluginReposResultMsg is dispatched when GetPluginRepos completes.
type PluginReposResultMsg struct {
	Repos []string // ordered list; first entry is always the built-in repo
	Err   error
}

// GetPluginRepos requests the current plugin repository list from the runtime.
// The result arrives as a PluginReposResultMsg.
func (c *Client) GetPluginRepos() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_plugin_repos"})
		raw := <-ch
		var msg PluginReposResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Repos []string `json:"repos"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Repos = payload.Repos
			}
		}
		c.program.Send(msg)
	}()
}

// SetPluginRepos sends an updated plugin repository list to the runtime.
// The built-in repo must always be included; the UI enforces this.
func (c *Client) SetPluginRepos(repos []string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "set_plugin_repos",
			"repos": repos,
		})
	}()
}

// ── Plugin registry ───────────────────────────────────────────────────────────

// RegistryEntry is one plugin listed in a registry index.
type RegistryEntry struct {
	Name        string  `json:"name"`
	Version     string  `json:"version"`
	PluginType  string  `json:"plugin_type"`
	Description string  `json:"description"`
	Author      string  `json:"author"`
	Homepage    *string `json:"homepage"`
	BinaryURL   string  `json:"binary_url"`
	Checksum    string  `json:"checksum"`
	Installed   bool    `json:"installed"`
}

// RegistryBrowseResultMsg is dispatched when BrowseRegistry completes.
type RegistryBrowseResultMsg struct {
	Entries     []RegistryEntry
	FailedRepos []string
	Err         error
}

// PluginInstallResultMsg is dispatched when InstallPlugin completes.
type PluginInstallResultMsg struct {
	Name    string
	Version string
	Path    string
	Err     error
}

// BrowseRegistry requests the merged plugin index from all configured registries.
// The result arrives as a RegistryBrowseResultMsg.
func (c *Client) BrowseRegistry() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "browse_registry", "id": id})
		raw := <-ch
		var msg RegistryBrowseResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Entries     []RegistryEntry `json:"entries"`
				FailedRepos []string        `json:"failed_repos"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Entries = payload.Entries
				msg.FailedRepos = payload.FailedRepos
			}
		}
		c.program.Send(msg)
	}()
}

// InstallPlugin downloads and installs a plugin from a registry entry.
// The result arrives as a PluginInstallResultMsg.
func (c *Client) InstallPlugin(name, version, binaryURL, checksum string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":       "install_plugin",
			"id":         id,
			"name":       name,
			"version":    version,
			"binary_url": binaryURL,
			"checksum":   checksum,
		})
		raw := <-ch
		var msg PluginInstallResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Name    string `json:"name"`
				Version string `json:"version"`
				Path    string `json:"path"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Name = payload.Name
				msg.Version = payload.Version
				msg.Path = payload.Path
			}
		}
		c.program.Send(msg)
	}()
}

// SetConfig sends a live config update to the runtime.
func (c *Client) SetConfig(key string, value any) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "set_config",
			"key":   key,
			"value": value,
		})
	}()
}

// StreamInfo describes a single resolved stream candidate.
type StreamInfo struct {
	URL       string `json:"url"`
	Label     string `json:"name"`
	Quality   string `json:"quality"`
	Protocol  string `json:"protocol"`
	Seeders   int    `json:"seeders"`
	Score     int    `json:"score"`
	Provider  string `json:"provider"`
	SizeBytes int64  `json:"size_bytes,omitempty"`
	Badge     string `json:"badge"`
	Codec     string `json:"codec,omitempty"`
	Source    string `json:"source,omitempty"`
	HDR       bool   `json:"hdr,omitempty"`
}

// StreamsResolvedMsg is delivered when the runtime has resolved stream candidates.
type StreamsResolvedMsg struct {
	EntryID string
	Streams []StreamInfo
}

// StreamBenchmarkResultMsg is dispatched (TUI-side only, no runtime IPC) when
// a single stream probe finishes.  SpeedMbps is 0 and Err is non-nil on failure.
type StreamBenchmarkResultMsg struct {
	EntryID   string
	URL       string
	SpeedMbps float64
	LatencyMs int
	Err       error
}

// StreamBenchmarkDoneMsg is dispatched after all probes for an entry complete.
type StreamBenchmarkDoneMsg struct {
	EntryID string
}

// SearchResultSelectedMsg is dispatched when the user picks a result from SearchScreen.
// The RootModel's LegacyScreen handles it by opening the detail overlay.
type SearchResultSelectedMsg struct {
	Entry MediaEntry
}

// EpisodesLoadedMsg carries episode data for a season, dispatched by the metadata layer.
type EpisodesLoadedMsg struct {
	SeriesID string
	Season   int
	Episodes []EpisodeEntry
}

// BingeContextMsg is fired by EpisodeScreen when the user plays an episode with
// binge mode enabled.  It carries the full season episode list so Model can
// automatically queue the next episode when the current one ends.
type BingeContextMsg struct {
	SeriesTitle  string
	SeriesID     string
	Tab          MediaTab
	Episodes     []EpisodeEntry
	CurrentIdx   int  // index of the episode that just started playing
	BingeEnabled bool // false → context stored but countdown won't fire
}

// EpisodeEntry is one episode in a series season.
type EpisodeEntry struct {
	Season   int    `json:"season"`
	Episode  int    `json:"episode"`
	Title    string `json:"title"`
	AirDate  string `json:"air_date,omitempty"`
	Runtime  int    `json:"runtime_mins,omitempty"`
	Provider string `json:"provider"`
	EntryID  string `json:"entry_id"`
}

// PlayerStartedMsg is pushed when mpv has launched and is playing.
type PlayerStartedMsg struct {
	Title    string `json:"title"`
	Path     string `json:"path"`  // local file path or URL
	Duration float64 `json:"duration"` // total seconds, 0 if unknown
}

// TrackInfo describes a single audio, subtitle, or video track reported by mpv.
type TrackInfo struct {
	ID        int64  `json:"id"`
	TrackType string `json:"track_type"` // "audio" | "sub" | "video"
	Lang      string `json:"lang"`       // BCP-47 tag, e.g. "en", "ja" — may be empty
	Title     string `json:"title"`      // human-readable label from container — may be empty
	Selected  bool   `json:"selected"`
	External  bool   `json:"external"` // true for tracks loaded via --sub-file
}

// Label returns the best human-readable string for the track (title > lang > "Track N").
func (t TrackInfo) Label() string {
	if t.Title != "" {
		return t.Title
	}
	if t.Lang != "" {
		return strings.ToUpper(t.Lang)
	}
	return fmt.Sprintf("Track %d", t.ID)
}

// PlayerTracksUpdatedMsg is pushed once per file load when mpv reports its track list.
type PlayerTracksUpdatedMsg struct {
	Tracks []TrackInfo `json:"tracks"`
}

// PlayerProgressMsg is pushed ~1/s while mpv is playing.
// Extended fields are populated once mpv reports them (may be zero/empty initially).
type PlayerProgressMsg struct {
	Position     float64 `json:"position"`      // elapsed seconds
	Duration     float64 `json:"duration"`
	Paused       bool    `json:"paused"`
	CachePercent float64 `json:"cache_percent"` // buffering progress (torrent streams)

	// Extended playback state (added in v5)
	Volume          float64 `json:"volume,omitempty"`
	Muted           bool    `json:"muted,omitempty"`
	SubtitleDelay   float64 `json:"subtitle_delay,omitempty"`
	AudioDelay      float64 `json:"audio_delay,omitempty"`
	AudioLabel      string  `json:"audio_label,omitempty"`
	SubLabel        string  `json:"sub_label,omitempty"`
	Quality         string  `json:"quality,omitempty"`
	Protocol        string  `json:"protocol,omitempty"`
	ActiveCandidate int     `json:"active_candidate,omitempty"`
	CandidateCount  int     `json:"candidate_count,omitempty"`
}

// PlayerEndedMsg is pushed when playback finishes or mpv exits.
type PlayerEndedMsg struct {
	Reason string `json:"reason"` // "eof" | "quit" | "error"
	Error  string `json:"error,omitempty"`
}

// PlayerTerminalTakeoverMsg is pushed just before mpv is launched in terminal
// VO mode (kitty/sixel/tct/chafa).  The TUI must release the terminal so mpv
// can write to it directly.  When PlayerEndedMsg arrives afterward, the TUI
// should restore the terminal.
type PlayerTerminalTakeoverMsg struct {
	VO string `json:"vo"` // the terminal VO driver in use
}

// PlayerBufferingMsg is pushed while waiting for pre-roll or during a stall-guard pause.
type PlayerBufferingMsg struct {
	Reason       string  `json:"reason"`        // "initial" | "stall_guard"
	FillPercent  float64 `json:"fill_percent"`  // 0–100
	SpeedMbps    float64 `json:"speed_mbps"`
	PreRollSecs  float64 `json:"pre_roll_secs"` // target buffer in seconds of video
	EtaSecs      float64 `json:"eta_secs"`      // seconds until buffer ready
}

// PlayerBufferReadyMsg is pushed when the pre-roll or stall-guard recovery finishes.
type PlayerBufferReadyMsg struct {
	PreRollSecs float64 `json:"pre_roll_secs"`
	SpeedMbps   float64 `json:"speed_mbps"`
	Slack       float64 `json:"slack"` // download_speed / video_bitrate
}

// CatalogLoadedMsg signals the initial catalog population is complete for a tab.
type CatalogLoadedMsg struct {
	Tab string `json:"tab"`
}

// ── Torrent / aria2 download types ────────────────────────────────────────────

// DownloadEntry tracks the live state of a single aria2 managed download.
// It is updated in-place as progress, complete, and error messages arrive.
type DownloadEntry struct {
	GID      string
	Title    string
	Progress float64  // 0.0 – 1.0
	Speed    string   // human-readable, e.g. "3.2 MiB/s"
	ETA      string   // human-readable, e.g. "34s"
	Seeders  uint64
	Status   string   // "active" | "complete" | "error"
	Files    []string // populated on completion
	Error    string   // populated on error
}

// DownloadStartedMsg is pushed by the runtime when aria2 begins a new download.
type DownloadStartedMsg struct {
	GID   string `json:"gid"`
	Title string `json:"title"`
	URI   string `json:"uri"`
	Dir   string `json:"dir"`
}

// DownloadProgressMsg is pushed ~2/s while a download is in progress.
type DownloadProgressMsg struct {
	GID      string  `json:"gid"`
	Progress float64 `json:"progress"` // 0.0 – 1.0
	Speed    string  `json:"speed"`
	ETA      string  `json:"eta"`
	Seeders  uint64  `json:"seeders"`
}

// DownloadCompleteMsg is pushed when a download finishes successfully.
type DownloadCompleteMsg struct {
	GID   string   `json:"gid"`
	Files []string `json:"files"`
}

// DownloadErrorMsg is pushed when an aria2 download fails.
type DownloadErrorMsg struct {
	GID     string `json:"gid"`
	Message string `json:"message"`
}

// QueueUpdateMsg is pushed whenever the playback queue length changes.
type QueueUpdateMsg struct {
	QueueLen int `json:"queue_len"`
}

// ── MPD / audio types ─────────────────────────────────────────────────────────

// MpdOutput describes one MPD audio output device.
type MpdOutput struct {
	ID      uint32 `json:"id"`
	Name    string `json:"name"`
	Plugin  string `json:"plugin"`
	Enabled bool   `json:"enabled"`
}

// MpdStatusMsg is pushed by the runtime's MPD idle loop whenever
// player/mixer/options state changes in MPD.
type MpdStatusMsg struct {
	State       string  `json:"state"`        // "play" | "pause" | "stop"
	SongTitle   string  `json:"song_title"`
	SongArtist  string  `json:"song_artist"`
	SongAlbum   string  `json:"song_album"`
	Elapsed     float64 `json:"elapsed"`
	Duration    float64 `json:"duration"`
	Volume      uint32  `json:"volume"`       // 0–100
	Bitrate     uint32  `json:"bitrate"`      // kbps, 0 if unknown
	AudioFormat string  `json:"audio_format"` // "192000:24:2"
	ReplayGain  string  `json:"replay_gain"`  // off|track|album|auto
	Crossfade   uint32  `json:"crossfade"`    // seconds
	Consume     bool    `json:"consume"`
	Random      bool    `json:"random"`
	QueueLength uint32  `json:"queue_length"`
	SongPos     int32   `json:"song_pos"`     // 0-based queue position of current song; -1 if stopped
	SongID      int32   `json:"song_id"`      // MPD song ID of current song; 0 if unknown
}

// MpdOutputsResultMsg is dispatched when GetMpdOutputs completes.
type MpdOutputsResultMsg struct {
	Outputs []MpdOutput
	Err     error
}

// ── Skip detection ────────────────────────────────────────────────────────────

// SkipSegmentMsg is pushed when the runtime detects an intro or credits segment.
// For intro: Start/End are absolute timestamps (seconds from start).
// For credits: Start/End are seconds-from-end of video (FromEnd = true).
type SkipSegmentMsg struct {
	SegmentType string  `json:"segment_type"` // "intro" | "credits"
	Start       float64 `json:"start"`
	End         float64 `json:"end"`
	FromEnd     bool    `json:"from_end"`
}

// GetMpdOutputs requests the list of MPD audio outputs.
// Results arrive as MpdOutputsResultMsg.
func (c *Client) GetMpdOutputs() {
	go func() {
		_ = c.sendRaw(map[string]any{"type": "get_mpd_outputs"})
	}()
}

// MpdCmd sends a typed MPD player command.
func (c *Client) MpdCmd(cmd string, params map[string]any) {
	go func() {
		payload := map[string]any{"type": "cmd", "cmd": cmd}
		for k, v := range params {
			payload[k] = v
		}
		_ = c.sendRaw(payload)
	}()
}
