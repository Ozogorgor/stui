package ipc

import (
	"encoding/json"
	"fmt"
)

// Public request methods for the IPC client

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
func (c *Client) UnloadPlugin(pluginID string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":      "unload_plugin",
			"plugin_id": pluginID,
		})
		<-ch
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

// PlayerStop sends a stop command to the runtime (kills mpv + aria2 GID).
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

// DownloadStream starts an aria2 download without launching mpv.
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

// LoadEpisodes requests episode metadata for a series season.
func (c *Client) LoadEpisodes(seriesID string, season int) {
	go func() {
		id := c.nextID()
		payload := map[string]any{
			"type":     "metadata",
			"id":       id,
			"entry_id": seriesID,
			"kind":     "episodes",
			"season":   season,
		}
		ch := c.sendWithID(id, payload)
		raw := <-ch
		if raw.Err != nil {
			c.program.Send(StatusMsg{Text: "episodes load failed: " + raw.Err.Error()})
			return
		}
		var resp struct {
			Episodes []EpisodeEntry `json:"episodes"`
		}
		if err := raw.decodeData(&resp); err != nil {
			c.program.Send(StatusMsg{Text: "episodes load failed: " + err.Error()})
			return
		}
		c.program.Send(EpisodesLoadedMsg{
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

// GetPluginRepos requests the current plugin repository list.
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

// decodeSearchResult decodes a search response into a SearchResultMsg.
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
		raw := <-ch
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
		c.program.Send(msg)
	}()
}

// GetStreamPolicy fetches the persisted stream selection policy from the runtime.
// The result is dispatched as a StreamPolicyLoadedMsg to the Bubble Tea program.
func (c *Client) GetStreamPolicy() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "get_stream_policy", "id": id})
		raw := <-ch
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
		c.program.Send(msg)
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
		<-ch // wait for ack, ignore response content
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
