package ipc

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"
	"time"
)

type MockRuntime struct {
	stdin     *bufio.Scanner
	stdout    *os.File
	stdinPipe io.Closer // write end of pipe; closing it unblocks the scanner
	done      chan struct{}
}

func NewMockRuntime() *MockRuntime {
	pr, pw, err := os.Pipe()
	if err != nil {
		// Fall back to os.Stdin if pipe creation fails (shouldn't happen).
		return &MockRuntime{
			stdin:  bufio.NewScanner(os.Stdin),
			stdout: os.Stdout,
			done:   make(chan struct{}),
		}
	}
	return &MockRuntime{
		stdin:     bufio.NewScanner(pr),
		stdout:    os.Stdout,
		stdinPipe: pw,
		done:      make(chan struct{}),
	}
}

// Stop signals the Run goroutine to exit cleanly by closing the write end of
// the input pipe, which causes Scanner.Scan() to return false.
func (m *MockRuntime) Stop() {
	if m.stdinPipe != nil {
		m.stdinPipe.Close()
	}
	<-m.done
}

func (m *MockRuntime) Run() {
	defer close(m.done)
	for m.stdin.Scan() {
		line := m.stdin.Text()
		if line == "" {
			continue
		}

		var req map[string]interface{}
		if err := json.Unmarshal([]byte(line), &req); err != nil {
			continue
		}

		reqType, _ := req["type"].(string)
		id, _ := req["id"].(string)

		var resp map[string]interface{}
		switch reqType {
		case "ping":
			resp = m.handlePing(req)
		case "shutdown":
			m.stdout.WriteString("{}\n")
			return
		case "list_plugins":
			resp = m.handleListPlugins(id)
		case "search":
			resp = m.handleSearch(id, req)
		case "load_plugin":
			resp = m.handleLoadPlugin(id, req)
		case "unload_plugin":
			resp = m.handleUnloadPlugin(id, req)
		case "get_provider_settings":
			resp = m.handleGetProviderSettings(id)
		default:
			resp = map[string]interface{}{
				"type": "ok",
				"id":   id,
			}
		}

		if resp != nil {
			data, _ := json.Marshal(resp)
			m.stdout.WriteString(string(data) + "\n")
		}
	}
}

func (m *MockRuntime) handlePing(req map[string]interface{}) map[string]interface{} {
	return map[string]interface{}{
		"type":            "pong",
		"id":              req["id"],
		"ipc_version":     1,
		"runtime_version": "0.1.0-test",
		"version_ok":      true,
	}
}

func (m *MockRuntime) handleListPlugins(id string) map[string]interface{} {
	return map[string]interface{}{
		"type":    "plugin_list",
		"id":      id,
		"plugins": []map[string]interface{}{},
	}
}

func (m *MockRuntime) handleSearch(id string, req map[string]interface{}) map[string]interface{} {
	query, _ := req["query"].(string)

	items := []map[string]interface{}{}
	if query != "" && query != "empty" {
		items = []map[string]interface{}{
			{
				"id":          "tt0000001",
				"title":       "Test Movie",
				"year":        "2024",
				"genre":       "Action",
				"rating":      "8.5",
				"description": "A test movie",
				"poster_url":  nil,
				"provider":    "tmdb",
				"tab":         "movies",
			},
			{
				"id":          "tt0000002",
				"title":       "Another Test",
				"year":        "2023",
				"genre":       "Drama",
				"rating":      "7.2",
				"description": "Another test movie",
				"poster_url":  nil,
				"provider":    "tmdb",
				"tab":         "movies",
			},
		}
	}

	return map[string]interface{}{
		"type":   "search_result",
		"id":     id,
		"items":  items,
		"total":  len(items),
		"offset": 0,
	}
}

func (m *MockRuntime) handleLoadPlugin(id string, req map[string]interface{}) map[string]interface{} {
	path, _ := req["path"].(string)
	if strings.Contains(path, "error") {
		return map[string]interface{}{
			"type":    "error",
			"id":      id,
			"code":    "plugin_load_failed",
			"message": "failed to load plugin",
		}
	}
	return map[string]interface{}{
		"type":        "plugin_loaded",
		"id":          id,
		"plugin_id":   "test-plugin",
		"name":        "Test Plugin",
		"version":     "1.0.0",
		"plugin_type": "rpc",
		"status":      "loaded",
	}
}

func (m *MockRuntime) handleUnloadPlugin(id string, req map[string]interface{}) map[string]interface{} {
	pluginID, _ := req["plugin_id"].(string)
	if pluginID == "nonexistent" {
		return map[string]interface{}{
			"type":    "error",
			"id":      id,
			"code":    "plugin_not_found",
			"message": "plugin not found",
		}
	}
	return map[string]interface{}{
		"type": "ok",
		"id":   id,
	}
}

func (m *MockRuntime) handleGetProviderSettings(id string) map[string]interface{} {
	return map[string]interface{}{
		"type":     "provider_settings",
		"id":       id,
		"settings": []map[string]interface{}{},
	}
}

func (m *MockRuntime) SendEvent(eventType string, data map[string]interface{}) {
	event := map[string]interface{}{
		"type": eventType,
	}
	for k, v := range data {
		event[k] = v
	}
	eventJSON, _ := json.Marshal(event)
	fmt.Fprintf(m.stdout, "%s\n", string(eventJSON))
}

func (m *MockRuntime) SendGridUpdate() {
	m.SendEvent("grid_update", map[string]interface{}{
		"tab":     "movies",
		"entries": []map[string]interface{}{},
	})
}

func (m *MockRuntime) SendPluginToast() {
	m.SendEvent("plugin_toast", map[string]interface{}{
		"plugin_id": "test-plugin",
		"message":   "Plugin loaded",
		"level":     "info",
	})
}

func (m *MockRuntime) SendPlayerStarted() {
	m.SendEvent("player_started", map[string]interface{}{
		"id":           "test-playback",
		"entry_id":     "tt0000001",
		"provider":     "tmdb",
		"stream_url":   "http://example.com/video.mkv",
		"subtitle_url": nil,
	})
}

func (m *MockRuntime) SendPlayerProgress() {
	m.SendEvent("player_progress", map[string]interface{}{
		"position":    120.5,
		"duration":    3600.0,
		"volume":      100.0,
		"paused":      false,
		"buffering":   false,
		"audio_track": 0,
		"sub_track":   -1,
	})
}

func (m *MockRuntime) SendPlayerEnded() {
	m.SendEvent("player_ended", map[string]interface{}{
		"reason": "eof",
	})
}

func (m *MockRuntime) RunWithTimeout(timeout time.Duration, events func(m *MockRuntime)) {
	go m.Run()
	time.Sleep(10 * time.Millisecond)
	if events != nil {
		events(m)
	}
	time.Sleep(timeout)
	m.Stop()
}
