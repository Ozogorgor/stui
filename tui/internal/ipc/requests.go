package ipc

import (
	"context"
	"encoding/json"
	"fmt"
	"time"
)

// receiveWithTimeout waits for a response from the channel with a timeout.
// Returns the response or a timeout error.
func receiveWithTimeout(ch <-chan RawResponse) RawResponse {
	select {
	case resp := <-ch:
		return resp
	case <-time.After(defaultRequestTimeout):
		return RawResponse{Err: fmt.Errorf("ipc: request timed out after %v", defaultRequestTimeout)}
	}
}

// Public request methods for the IPC client

// Search dispatches a scoped search request and returns the query id plus a
// channel that receives scope-results as they stream back. The channel is
// closed when every scope has emitted partial=false. On error, neither qid
// nor channel is returned.
//
// Caller is responsible for draining the channel. A pending search will retain
// its subscriber entry in scopeSubs until finalization or channel GC.
func (c *Client) Search(ctx context.Context, query string, scopes []SearchScope) (uint64, <-chan ScopeResultsMsg, error) {
	qid := c.NextQueryID()
	ch := c.SubscribeScopeResults(qid, scopes)

	reqID := c.nextID()
	req := SearchReq{
		ID:      reqID,
		Query:   query,
		Scopes:  scopes,
		Limit:   50,
		Offset:  0,
		QueryID: qid,
	}

	// Marshal req as a map so sendWithID can add the id field and correlate the ack.
	payload := map[string]any{
		"type":     "search",
		"id":       reqID,
		"query":    req.Query,
		"scopes":   req.Scopes,
		"limit":    req.Limit,
		"offset":   req.Offset,
		"query_id": req.QueryID,
	}
	respCh := c.sendWithID(reqID, payload)

	// Wait for the ack in the background; the channel carry the real payload.
	// If the ack itself carries an error (transport failure), clean up the subscription.
	go func() {
		select {
		case raw := <-respCh:
			if raw.Err != nil {
				c.scopeSubs.Delete(qid)
			}
			// ack received (ok or error) — streaming events are the real payload.
		case <-ctx.Done():
			c.scopeSubs.Delete(qid)
		}
	}()

	return qid, ch, nil
}

// MpdSearch performs a synchronous local MPD search. MPD is fast enough that
// streaming adds complexity without benefit — a single typed response carries
// all three buckets.
func (c *Client) MpdSearch(ctx context.Context, query string, scopes []MpdScope) (*MpdSearchResult, error) {
	qid := c.NextQueryID()
	reqID := c.nextID()
	payload := map[string]any{
		"type":     "mpd_search",
		"id":       reqID,
		"query":    query,
		"scopes":   scopes,
		"limit":    uint32(200),
		"query_id": qid,
	}

	respCh := c.sendWithID(reqID, payload)

	var raw RawResponse
	select {
	case raw = <-respCh:
	case <-ctx.Done():
		return nil, ctx.Err()
	}

	if raw.Err != nil {
		return nil, raw.Err
	}
	if raw.Type == "error" {
		var ep ErrorPayload
		_ = json.Unmarshal(raw.Raw, &ep)
		return nil, fmt.Errorf("%s: %s", ep.Code, ep.Message)
	}

	var result MpdSearchResult
	if err := json.Unmarshal(raw.Raw, &result); err != nil {
		return nil, fmt.Errorf("ipc: mpd_search decode: %w", err)
	}
	return &result, nil
}

// UnloadPlugin removes a plugin from the running engine.
func (c *Client) UnloadPlugin(pluginID string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":      "unload_plugin",
			"plugin_id": pluginID,
		})
		receiveWithTimeout(ch)
		c.ListPlugins()
	}()
}

// SetPluginEnabled toggles whether a loaded plugin participates in
// dispatch. The plugin stays in the runtime registry either way —
// this is a soft enable/disable, not an uninstall. After the runtime
// responds we re-fetch the plugin list so the TUI reflects the new
// state immediately.
func (c *Client) SetPluginEnabled(pluginID string, enabled bool) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":      "set_plugin_enabled",
			"plugin_id": pluginID,
			"enabled":   enabled,
		})
		receiveWithTimeout(ch)
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
		raw := receiveWithTimeout(ch)
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
		c.send(msg)
	}()
}

// LastfmAlbumGetTracks requests the tracklist for a lastfm album
// via the runtime's `lastfm_album_tracks` IPC verb. Result is
// dispatched as a `LastFMAlbumTracksMsg` on the message channel.
// Used by the AlbumDetailScreen to populate its track list when the
// user opens an album in Music Browse.
func (c *Client) LastfmAlbumGetTracks(artist, album string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":   "lastfm_album_tracks",
			"id":     id,
			"artist": artist,
			"album":  album,
		})
		raw := receiveWithTimeout(ch)
		msg := LastFMAlbumTracksMsg{Artist: artist, Album: album}
		if raw.Err != nil {
			c.send(msg)
			return
		}
		if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(msg)
			return
		}
		// Wire shape from runtime/src/ipc/v1/mod.rs: tracks[]
		// entries carry `number` (1-based), `title`, optional
		// `duration_secs` (u32), optional `mbid`. We project into
		// the TUI's `AlbumTrack` shape, formatting duration as
		// "m:ss" so the renderer can show it inline without
		// re-parsing.
		var payload struct {
			Tracks []struct {
				Number       uint32  `json:"number"`
				Title        string  `json:"title"`
				DurationSecs *uint32 `json:"duration_secs,omitempty"`
				Mbid         string  `json:"mbid,omitempty"`
			} `json:"tracks"`
		}
		if err := json.Unmarshal(raw.Raw, &payload); err != nil {
			c.send(msg)
			return
		}
		out := make([]AlbumTrack, 0, len(payload.Tracks))
		for _, t := range payload.Tracks {
			dur := ""
			if t.DurationSecs != nil && *t.DurationSecs > 0 {
				secs := *t.DurationSecs
				dur = fmt.Sprintf("%d:%02d", secs/60, secs%60)
			}
			out = append(out, AlbumTrack{
				Number:   int(t.Number),
				Title:    t.Title,
				Duration: dur,
			})
		}
		msg.Tracks = out
		c.send(msg)
	}()
}

// MetadataPluginsForKind queries the runtime for the metadata-source
// plugins that contribute to `kind`'s detail-card fan-out. Used by the
// Settings → Metadata Sources screen to render the editable list.
//
// `kind` is the lowercase TUI tab label: "movies"/"series"/"anime"/
// "music". Result is dispatched as a `MetadataPluginsForKindMsg` on
// the message channel, with priority/discovered/disabled lists
// snapshotted from the runtime config + live plugin registry.
func (c *Client) MetadataPluginsForKind(kind string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "metadata_plugins_for_kind",
			"id":   id,
			"kind": kind,
		})
		raw := receiveWithTimeout(ch)
		msg := MetadataPluginsForKindMsg{Kind: kind}
		if raw.Err != nil {
			msg.Err = raw.Err
			c.send(msg)
			return
		}
		if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
			c.send(msg)
			return
		}
		var payload struct {
			Kind       string   `json:"kind"`
			Priority   []string `json:"priority"`
			Discovered []string `json:"discovered"`
			Disabled   []string `json:"disabled"`
		}
		if err := json.Unmarshal(raw.Raw, &payload); err != nil {
			msg.Err = err
			c.send(msg)
			return
		}
		msg.Priority = payload.Priority
		msg.Discovered = payload.Discovered
		msg.Disabled = payload.Disabled
		c.send(msg)
	}()
}

// ListPlugins requests the current plugin list.
func (c *Client) ListPlugins() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "list_plugins"})
		raw := receiveWithTimeout(ch)
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
		c.send(msg)
	}()
}

// Play sends a play request to the runtime.
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

// PlayerStop sends a stop command to the runtime (kills mpv + active torrent).
func (c *Client) PlayerStop() {
	go func() {
		_ = c.sendRaw(map[string]any{"type": "player_stop"})
	}()
}

// DeleteStream asks the runtime to remove the cached stream files for the given entry ID.
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
func (c *Client) Resolve(entryID, provider string) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":     "get_streams",
			"id":       id,
			"entry_id": entryID,
			"provider": provider,
		}
		ch := c.sendWithID(id, payload)
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "stream resolve failed: " + raw.Err.Error()})
			return
		}
		var resp struct {
			Streams []StreamInfo `json:"streams"`
		}
		if err := raw.decodeData(&resp); err != nil {
			c.send(StatusMsg{Text: "stream decode failed: " + err.Error()})
			return
		}
		c.send(StreamsResolvedMsg{EntryID: entryID, Streams: resp.Streams})
	}()
}

// FindStreamsRequest carries the rich media-reference fields the new
// `StreamProvider::find_streams` flow expects. Used by the detail
// card's Episodes tab streams column to ask jackett/prowlarr (and
// any future stream provider) for torrents matching a specific
// episode.
type FindStreamsRequest struct {
	Title       string
	Year        *uint32
	Kind        string
	Season      *uint32
	Episode     *uint32
	ExternalIDs map[string]string
	ImdbID      string
	TmdbID      string
}

// FindStreams dispatches a find_streams query (rich shape) to every
// loaded stream provider. Result is delivered as
// `EpisodeStreamsLoadedMsg{Season, Episode, Streams}`. Same `get_streams`
// IPC verb under the hood — runtime branches on whether `title` is
// populated to pick the new path vs the legacy `entry_id`-only one.
func (c *Client) FindStreams(req FindStreamsRequest) {
	season := 0
	if req.Season != nil {
		season = int(*req.Season)
	}
	episode := 0
	if req.Episode != nil {
		episode = int(*req.Episode)
	}
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":     "get_streams",
			"id":       id,
			"entry_id": "", // not used in the find_streams path
			"title":    req.Title,
		}
		if req.Year != nil {
			payload["year"] = *req.Year
		}
		if req.Kind != "" {
			payload["kind"] = req.Kind
		}
		if req.Season != nil {
			payload["season"] = *req.Season
		}
		if req.Episode != nil {
			payload["episode"] = *req.Episode
		}
		if len(req.ExternalIDs) > 0 {
			payload["external_ids"] = req.ExternalIDs
		}
		if req.ImdbID != "" {
			payload["imdb_id"] = req.ImdbID
		}
		if req.TmdbID != "" {
			payload["tmdb_id"] = req.TmdbID
		}
		ch := c.sendWithID(id, payload)
		raw := receiveWithTimeout(ch)
		msg := EpisodeStreamsLoadedMsg{Season: season, Episode: episode}
		if raw.Err != nil {
			msg.Err = raw.Err
			c.send(msg)
			return
		}
		if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
			c.send(msg)
			return
		}
		var resp struct {
			Streams []StreamInfo `json:"streams"`
		}
		if err := raw.decodeData(&resp); err != nil {
			msg.Err = err
			c.send(msg)
			return
		}
		msg.Streams = resp.Streams
		c.send(msg)
	}()
}

// DownloadStream starts a torrent download without launching mpv.
func (c *Client) DownloadStream(url, title string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "download_stream",
			"url":   url,
			"title": title,
		})
	}()
}

// CancelDownload asks the torrent engine to abort the download identified by id.
func (c *Client) CancelDownload(gid string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "download_cancel",
			"gid":  gid,
		})
	}()
}

// PlayFile launches mpv on a local file path.
func (c *Client) PlayFile(path, title string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "play_file",
			"path":  path,
			"title": title,
		})
	}()
}

// SwitchStream sends a stream-switch command to mpv.
func (c *Client) SwitchStream(url string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "cmd",
			"cmd":  "switch_stream",
			"url":  url,
		})
	}()
}

// GetDetailMetadata fires the four-verb metadata fan-out for a single
// detail entry. The runtime streams back one detail_metadata_partial
// message per verb as its merge finishes; these are dispatched through
// the unsolicited channel and handled by Model.Update via
// DetailState.ApplyMetadataPartial.
//
// Fire-and-forget — the per-verb partials don't share a request id
// with this send; the TUI correlates them by EntryID instead.
func (c *Client) GetDetailMetadata(entryID, idSource, kind, title string, year *uint16, externalIDs map[string]string) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":      "get_detail_metadata",
			"id":        id,
			"entry_id":  entryID,
			"id_source": idSource,
			"kind":      kind,
		}
		if title != "" {
			payload["title"] = title
		}
		if year != nil {
			payload["year"] = *year
		}
		if len(externalIDs) > 0 {
			payload["external_ids"] = externalIDs
		}
		_ = c.sendRaw(payload)
	}()
}

// LoadEpisodes requests episode metadata for a series season.
//
// `idSource` tells the runtime which plugin owns this entry (today only
// "tmdb" returns real data; other id_sources resolve to NOT_IMPLEMENTED
// at the plugin layer). When empty the runtime falls back to peeling a
// "<provider>-<id>" prefix off the id, with a final default of "tmdb".
//
// `externalIDs` carries the catalog entry's cross-provider id map (e.g.
// `{"imdb": "tt12345", "tvdb": "67890"}`) so the runtime can fall back
// to TVDB when the primary plugin (TMDB) errors out.
func (c *Client) LoadEpisodes(seriesID, idSource string, season int, externalIDs map[string]string) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":     "metadata",
			"id":       id,
			"entry_id": seriesID,
			"kind":     "episodes",
			"season":   season,
		}
		if idSource != "" {
			payload["id_source"] = idSource
		}
		if len(externalIDs) > 0 {
			payload["external_ids"] = externalIDs
		}
		ch := c.sendWithID(id, payload)
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "episodes load failed: " + raw.Err.Error()})
			c.send(EpisodesLoadFailedMsg{
				SeriesID: seriesID,
				Season:   season,
				Reason:   raw.Err.Error(),
			})
			return
		}
		// Runtime-side error envelopes arrive with type=="error" but no
		// transport error; surface the message so the screen exits the
		// loading state instead of silently rendering an empty list.
		if raw.Type == "error" {
			var env struct {
				Code    string `json:"code"`
				Message string `json:"message"`
			}
			_ = json.Unmarshal(raw.Raw, &env)
			reason := env.Message
			if env.Code != "" {
				reason = env.Code + ": " + env.Message
			}
			c.send(StatusMsg{Text: "episodes load failed: " + reason})
			c.send(EpisodesLoadFailedMsg{
				SeriesID: seriesID,
				Season:   season,
				Reason:   reason,
			})
			return
		}
		var resp struct {
			Episodes []EpisodeEntry `json:"episodes"`
		}
		if err := raw.decodeData(&resp); err != nil {
			c.send(StatusMsg{Text: "episodes load failed: " + err.Error()})
			c.send(EpisodesLoadFailedMsg{
				SeriesID: seriesID,
				Season:   season,
				Reason:   err.Error(),
			})
			return
		}
		c.send(EpisodesLoadedMsg{
			SeriesID: seriesID,
			Season:   season,
			Episodes: resp.Episodes,
		})
	}()
}

// GetProviderSettings requests the full provider configuration schema.
func (c *Client) GetProviderSettings() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_provider_settings"})
		raw := receiveWithTimeout(ch)
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
		c.send(msg)
	}()
}

// GetPluginRepos requests the current plugin repository list.
func (c *Client) GetPluginRepos() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_plugin_repos"})
		raw := receiveWithTimeout(ch)
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
		c.send(msg)
	}()
}

// SetPluginRepos sends an updated plugin repository list to the runtime.
func (c *Client) SetPluginRepos(repos []string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "set_plugin_repos",
			"repos": repos,
		})
	}()
}

// BrowseRegistry requests the merged plugin index from all configured registries.
func (c *Client) BrowseRegistry() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "browse_registry", "id": id})
		raw := receiveWithTimeout(ch)
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
		c.send(msg)
	}()
}

// InstallPlugin downloads and installs a plugin from a registry entry.
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
		raw := receiveWithTimeout(ch)
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
		c.send(msg)
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

// GetMpdOutputs requests the list of MPD audio outputs.
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
			// Skip reserved keys to prevent payload corruption
			if k == "type" || k == "cmd" {
				continue
			}
			payload[k] = v
		}
		_ = c.sendRaw(payload)
	}()
}

// ProviderField describes one configurable field for a provider.
type ProviderField struct {
	Key        string `json:"key"`
	Label      string `json:"label"`
	Hint       string `json:"hint"`
	Masked     bool   `json:"masked"`
	Configured bool   `json:"configured"`
	Required   bool   `json:"required"`
	Value      string `json:"value"`
}

// ProviderSchema describes one provider's configuration requirements.
type ProviderSchema struct {
	ID          string          `json:"id"`
	Name        string          `json:"name"`
	Description string          `json:"description"`
	PluginType  string          `json:"plugin_type"`
	Active      bool            `json:"active"`
	Fields      []ProviderField `json:"fields"`
}

// ProviderSettingsResultMsg is dispatched when GetProviderSettings completes.
type ProviderSettingsResultMsg struct {
	Providers []ProviderSchema
	Err       error
}

// PluginReposResultMsg is dispatched when GetPluginRepos completes.
type PluginReposResultMsg struct {
	Repos []string
	Err   error
}

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

// RankStreams sends a policy-based ranking request to the runtime.
func (c *Client) RankStreams(streams []StreamInfo, prefs StreamPreferences) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":        "rank_streams",
			"id":          id,
			"streams":     streams,
			"preferences": prefs,
		})
		raw := receiveWithTimeout(ch)
		var msg StreamsRankedMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var resp struct {
				Ranked []RankedStream `json:"ranked"`
			}
			if err := json.Unmarshal(raw.Raw, &resp); err != nil {
				msg.Err = err
			} else {
				msg.Ranked = resp.Ranked
			}
		}
		c.send(msg)
	}()
}

// GetStreamPolicy fetches the persisted stream selection policy from the runtime.
// The result is dispatched as a StreamPolicyLoadedMsg to the Bubble Tea program.
func (c *Client) GetStreamPolicy() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_stream_policy", "id": id})
		raw := receiveWithTimeout(ch)
		var msg StreamPolicyLoadedMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var resp struct {
				Policy StreamPreferences `json:"policy"`
			}
			if err := json.Unmarshal(raw.Raw, &resp); err != nil {
				msg.Err = err
			} else {
				msg.Policy = resp.Policy
			}
		}
		c.send(msg)
	}()
}

// SetStreamPolicy persists the stream selection policy via the runtime (fire-and-forget).
func (c *Client) SetStreamPolicy(prefs StreamPreferences) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":   "set_stream_policy",
			"id":     id,
			"policy": prefs,
		})
		receiveWithTimeout(ch) // wait for ack, ignore response content
	}()
}

// WatchHistoryEntry represents a watch history entry.
type WatchHistoryEntry struct {
	ID          string  `json:"id"`
	Title       string  `json:"title"`
	Year        *string `json:"year,omitempty"`
	Tab         string  `json:"tab"`
	Provider    string  `json:"provider"`
	ImdbID      *string `json:"imdb_id,omitempty"`
	Position    float64 `json:"position"`
	Duration    float64 `json:"duration"`
	Completed   bool    `json:"completed"`
	LastWatched int64   `json:"last_watched"`
	Season      uint    `json:"season,omitempty"`
	Episode     uint    `json:"episode,omitempty"`
	FilePath    *string `json:"file_path,omitempty"`
}

// Progress returns Position/Duration as a 0-1 fraction.
func (e WatchHistoryEntry) Progress() float64 {
	if e.Duration <= 0 {
		return 0
	}
	f := e.Position / e.Duration
	if f > 1 {
		f = 1
	}
	return f
}

// GetWatchHistoryEntry requests a single watch history entry by ID.
func (c *Client) GetWatchHistoryEntry(id string) <-chan WatchHistoryEntry {
	ch := make(chan WatchHistoryEntry, 1)
	go func() {
		reqID := c.nextID()
		respCh := c.sendWithID(reqID, map[string]any{
			"type":     "get_watch_history_entry",
			"entry_id": id,
		})
		raw := <-respCh
		if raw.Err != nil {
			close(ch)
			return
		}
		if raw.Type == "error" {
			close(ch)
			return
		}
		var resp struct {
			Entry *WatchHistoryEntry `json:"entry"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err != nil {
			close(ch)
			return
		}
		if resp.Entry != nil {
			ch <- *resp.Entry
		}
		close(ch)
	}()
	return ch
}

// GetWatchHistoryInProgress requests all in-progress entries for a tab.
func (c *Client) GetWatchHistoryInProgress(tab string) <-chan []WatchHistoryEntry {
	ch := make(chan []WatchHistoryEntry, 1)
	go func() {
		reqID := c.nextID()
		respCh := c.sendWithID(reqID, map[string]any{
			"type": "get_watch_history_in_progress",
			"tab":  tab,
		})
		raw := <-respCh
		if raw.Err != nil {
			close(ch)
			return
		}
		if raw.Type == "error" {
			close(ch)
			return
		}
		var resp struct {
			Entries []WatchHistoryEntry `json:"entries"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err != nil {
			close(ch)
			return
		}
		ch <- resp.Entries
		close(ch)
	}()
	return ch
}

// UpsertWatchHistoryEntry creates or updates a watch history entry.
func (c *Client) UpsertWatchHistoryEntry(entry WatchHistoryEntry) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":  "upsert_watch_history_entry",
			"entry": entry,
		})
	}()
}

// UpdateWatchHistoryPosition updates the position for an entry.
func (c *Client) UpdateWatchHistoryPosition(id string, position, duration float64) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type":     "update_watch_history_position",
			"id":       id,
			"position": position,
			"duration": duration,
		})
	}()
}

// MarkWatchHistoryCompleted marks an entry as completed.
func (c *Client) MarkWatchHistoryCompleted(id string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "mark_watch_history_completed",
			"id":   id,
		})
	}()
}

// RemoveWatchHistoryEntry removes a watch history entry.
func (c *Client) RemoveWatchHistoryEntry(id string) {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "remove_watch_history_entry",
			"id":   id,
		})
	}()
}

// CachedTab represents a cached tab's worth of catalog data.
type CachedTab struct {
	Tab       string         `json:"tab"`
	Entries   []CatalogEntry `json:"entries"`
	UpdatedAt int64          `json:"updated_at"`
}

// MediaCacheStats holds media cache statistics.
type MediaCacheStats struct {
	TotalCount  int64 `json:"total_count"`
	LastUpdated int64 `json:"last_updated"`
}

// CatalogRefresh asks the runtime to discard its in-mem SearchCache and
// re-dispatch the provider fan-out for the given tab. Fire-and-forget —
// the actual refreshed entries arrive via the existing GridUpdate stream.
func (c *Client) CatalogRefresh(tab string) {
	go func() {
		reqID := c.nextID()
		respCh := c.sendWithID(reqID, map[string]any{
			"type": "catalog_refresh",
			"tab":  tab,
		})
		// Drain the ack so pending-map entries don't leak, but ignore the
		// payload: the grid update broadcast is what actually matters.
		<-respCh
	}()
}

// GetMediaCacheTab requests cached entries for a specific tab.
func (c *Client) GetMediaCacheTab(tab string) <-chan CachedTab {
	ch := make(chan CachedTab, 1)
	go func() {
		reqID := c.nextID()
		respCh := c.sendWithID(reqID, map[string]any{
			"type": "get_media_cache_tab",
			"tab":  tab,
		})
		raw := <-respCh
		if raw.Err != nil {
			close(ch)
			return
		}
		if raw.Type == "error" {
			close(ch)
			return
		}
		var resp struct {
			Tab       string         `json:"tab"`
			Entries   []CatalogEntry `json:"entries"`
			UpdatedAt int64          `json:"updated_at"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err != nil {
			close(ch)
			return
		}
		ch <- CachedTab{
			Tab:       resp.Tab,
			Entries:   resp.Entries,
			UpdatedAt: resp.UpdatedAt,
		}
		close(ch)
	}()
	return ch
}

// GetMediaCacheAll requests all cached entries across all tabs.
func (c *Client) GetMediaCacheAll() <-chan []CatalogEntry {
	ch := make(chan []CatalogEntry, 1)
	go func() {
		reqID := c.nextID()
		respCh := c.sendWithID(reqID, map[string]any{
			"type": "get_media_cache_all",
		})
		raw := <-respCh
		if raw.Err != nil {
			close(ch)
			return
		}
		if raw.Type == "error" {
			close(ch)
			return
		}
		var resp struct {
			Entries []CatalogEntry `json:"entries"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err != nil {
			close(ch)
			return
		}
		ch <- resp.Entries
		close(ch)
	}()
	return ch
}

// GetMediaCacheStats requests media cache statistics.
func (c *Client) GetMediaCacheStats() <-chan MediaCacheStats {
	ch := make(chan MediaCacheStats, 1)
	go func() {
		reqID := c.nextID()
		respCh := c.sendWithID(reqID, map[string]any{
			"type": "get_media_cache_stats",
		})
		raw := <-respCh
		if raw.Err != nil {
			close(ch)
			return
		}
		if raw.Type == "error" {
			close(ch)
			return
		}
		var resp struct {
			TotalCount  int64 `json:"total_count"`
			LastUpdated int64 `json:"last_updated"`
		}
		if err := json.Unmarshal(raw.Raw, &resp); err != nil {
			close(ch)
			return
		}
		ch <- MediaCacheStats{
			TotalCount:  resp.TotalCount,
			LastUpdated: resp.LastUpdated,
		}
		close(ch)
	}()
	return ch
}

// ClearMediaCache clears the entire media cache.
func (c *Client) ClearMediaCache() {
	go func() {
		_ = c.sendRaw(map[string]any{
			"type": "clear_media_cache",
		})
	}()
}

// StoragePathsResponse represents the storage directory configuration.
type StoragePathsResponse struct {
	Movies   string `json:"movies"`
	Series   string `json:"series"`
	Music    string `json:"music"`
	Anime    string `json:"anime"`
	Podcasts string `json:"podcasts"`
}

// GetStoragePaths returns the current storage directory paths.
func (c *Client) GetStoragePaths() <-chan StoragePathsResponse {
	ch := make(chan StoragePathsResponse, 1)
	id := c.nextID()
	go func() {
		defer close(ch)
		respCh := c.sendWithID(id, map[string]any{"type": "get_storage_paths", "id": id})
		resp := <-respCh
		if resp.Err != nil {
			return
		}
		var data struct {
			Type     string `json:"type"`
			Movies   string `json:"movies"`
			Series   string `json:"series"`
			Music    string `json:"music"`
			Anime    string `json:"anime"`
			Podcasts string `json:"podcasts"`
		}
		if err := json.Unmarshal(resp.Raw, &data); err != nil {
			return
		}
		ch <- StoragePathsResponse{
			Movies:   data.Movies,
			Series:   data.Series,
			Music:    data.Music,
			Anime:    data.Anime,
			Podcasts: data.Podcasts,
		}
	}()
	return ch
}

// SetStoragePathsRequest contains storage directory paths to update.
type SetStoragePathsRequest struct {
	Movies   *string
	Series   *string
	Music    *string
	Anime    *string
	Podcasts *string
}

// SetTrace enables or disables the runtime's pipeline trace output (stderr).
// Call immediately after the handshake when -v is passed.
func (c *Client) SetTrace(enabled bool) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":    "set_trace",
			"enabled": enabled,
		})
		receiveWithTimeout(ch) // wait for Ok response; ignore it
	}()
}

// SetStoragePaths updates storage directory paths.
func (c *Client) SetStoragePaths(req SetStoragePathsRequest) <-chan bool {
	ch := make(chan bool, 1)
	id := c.nextID()
	go func() {
		defer close(ch)
		payload := map[string]any{"type": "set_storage_paths", "id": id}
		if req.Movies != nil {
			payload["movies"] = *req.Movies
		}
		if req.Series != nil {
			payload["series"] = *req.Series
		}
		if req.Music != nil {
			payload["music"] = *req.Music
		}
		if req.Anime != nil {
			payload["anime"] = *req.Anime
		}
		if req.Podcasts != nil {
			payload["podcasts"] = *req.Podcasts
		}
		respCh := c.sendWithID(id, payload)
		resp := <-respCh
		if resp.Err != nil {
			ch <- false
			return
		}
		var data struct {
			Type    string `json:"type"`
			Success bool   `json:"success"`
		}
		if err := json.Unmarshal(resp.Raw, &data); err != nil {
			ch <- false
			return
		}
		ch <- data.Success
	}()
	return ch
}

// ── DSP requests ─────────────────────────────────────────────────────────────

// DspStatusMsg is dispatched when GetDspStatus completes.
type DspStatusMsg struct {
	Enabled            bool   `json:"enabled"`
	OutputSampleRate   uint32 `json:"output_sample_rate"`
	ResampleEnabled    bool   `json:"resample_enabled"`
	DsdToPcmEnabled    bool   `json:"dsd_to_pcm_enabled"`
	ConvolutionEnabled bool   `json:"convolution_enabled"`
	ConvolutionBypass  bool   `json:"convolution_bypass"`
	Active             bool   `json:"active"`
	Err                error
}

// GetDspStatus requests the current DSP pipeline status.
func (c *Client) GetDspStatus() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_dsp_status", "id": id})
		raw := receiveWithTimeout(ch)
		var msg DspStatusMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			if err := json.Unmarshal(raw.Raw, &msg); err != nil {
				msg.Err = err
			}
		}
		c.send(msg)
	}()
}

// SetDspConfig updates DSP configuration.
func (c *Client) SetDspConfig(enabled *bool, outputSampleRate *uint32, upsampleRatio *uint32, filterType *string, resampleEnabled *bool, dsdToPcmEnabled *bool, outputMode *string, convolutionEnabled *bool, convolutionBypass *bool) {
	go func() {
		id := c.nextID()
		payload := map[string]any{"type": "set_dsp_config", "id": id}
		if enabled != nil {
			payload["enabled"] = *enabled
		}
		if outputSampleRate != nil {
			payload["output_sample_rate"] = *outputSampleRate
		}
		if upsampleRatio != nil {
			payload["upsample_ratio"] = *upsampleRatio
		}
		if filterType != nil {
			payload["filter_type"] = *filterType
		}
		if resampleEnabled != nil {
			payload["resample_enabled"] = *resampleEnabled
		}
		if dsdToPcmEnabled != nil {
			payload["dsd_to_pcm_enabled"] = *dsdToPcmEnabled
		}
		if outputMode != nil {
			payload["output_mode"] = *outputMode
		}
		if convolutionEnabled != nil {
			payload["convolution_enabled"] = *convolutionEnabled
		}
		if convolutionBypass != nil {
			payload["convolution_bypass"] = *convolutionBypass
		}
		ch := c.sendWithID(id, payload)
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "DSP config failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("DSP config failed: %s %s", ep.Code, ep.Message)})
		} else {
			c.send(StatusMsg{Text: "DSP config updated"})
		}
	}()
}

// LoadConvolutionFilter loads a convolution filter from file.
func (c *Client) LoadConvolutionFilter(path string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "load_convolution_filter",
			"id":   id,
			"path": path,
		})
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "Load filter failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("Load filter failed: %s %s", ep.Code, ep.Message)})
		} else {
			c.send(StatusMsg{Text: "Convolution filter loaded"})
		}
	}()
}

// BindDspToMpd binds DSP to MPD audio output.
func (c *Client) BindDspToMpd() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "bind_dsp_to_mpd", "id": id})
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "Bind DSP to MPD failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("Bind DSP to MPD failed: %s %s", ep.Code, ep.Message)})
		} else {
			var resp struct {
				Success bool   `json:"success"`
				Config  string `json:"config"`
			}
			if err := json.Unmarshal(raw.Raw, &resp); err != nil {
				c.send(StatusMsg{Text: "Bind DSP to MPD: parse error"})
			} else if resp.Success {
				c.send(StatusMsg{Text: "DSP bound to MPD successfully"})
			} else {
				c.send(StatusMsg{Text: "DSP bind to MPD failed"})
			}
		}
	}()
}

// DspBoundToMpdMsg is dispatched when BindDspToMpd completes.
type DspBoundToMpdMsg struct {
	Success bool
	Config  string
}

// ListDspProfiles lists all saved DSP profiles.
func (c *Client) ListDspProfiles() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "list_dsp_profiles", "id": id})
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "List profiles failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("List profiles failed: %s %s", ep.Code, ep.Message)})
		} else {
			var resp struct {
				Profiles []string `json:"profiles"`
			}
			if err := json.Unmarshal(raw.Raw, &resp); err != nil {
				c.send(StatusMsg{Text: "List profiles: parse error"})
			} else {
				c.send(DspProfilesListedMsg{Profiles: resp.Profiles})
			}
		}
	}()
}

// DspProfilesListedMsg is dispatched when ListDspProfiles completes.
type DspProfilesListedMsg struct {
	Profiles []string
}

// SaveDspProfile saves a DSP profile with the given name.
func (c *Client) SaveDspProfile(name string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "save_dsp_profile",
			"id":   id,
			"name": name,
		})
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "Save profile failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("Save profile failed: %s %s", ep.Code, ep.Message)})
		} else {
			c.send(StatusMsg{Text: "Profile saved: " + name})
		}
	}()
}

// LoadDspProfile loads a DSP profile with the given name.
func (c *Client) LoadDspProfile(name string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "load_dsp_profile",
			"id":   id,
			"name": name,
		})
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "Load profile failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("Load profile failed: %s %s", ep.Code, ep.Message)})
		} else {
			c.send(DspProfileLoadedMsg{Name: name})
		}
	}()
}

// DspProfileLoadedMsg is dispatched when LoadDspProfile completes.
type DspProfileLoadedMsg struct {
	Name string
}

// DeleteDspProfile deletes a DSP profile with the given name.
func (c *Client) DeleteDspProfile(name string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "delete_dsp_profile",
			"id":   id,
			"name": name,
		})
		raw := receiveWithTimeout(ch)
		if raw.Err != nil {
			c.send(StatusMsg{Text: "Delete profile failed: " + raw.Err.Error()})
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			c.send(StatusMsg{Text: fmt.Sprintf("Delete profile failed: %s %s", ep.Code, ep.Message)})
		} else {
			c.send(StatusMsg{Text: "Profile deleted: " + name})
		}
	}()
}
