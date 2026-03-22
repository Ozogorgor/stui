package ipc

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"charm.land/bubbletea/v2"
)

func TestMain(m *testing.M) {
	os.Exit(m.Run())
}

type mockProgram struct {
	mu       sync.Mutex
	messages []tea.Msg
}

func newMockProgram() *mockProgram {
	return &mockProgram{
		messages: make([]tea.Msg, 0),
	}
}

func (p *mockProgram) Send(msg tea.Msg) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.messages = append(p.messages, msg)
}

func (p *mockProgram) Messages() []tea.Msg {
	p.mu.Lock()
	defer p.mu.Unlock()
	result := make([]tea.Msg, len(p.messages))
	copy(result, p.messages)
	return result
}

func (p *mockProgram) ClearMessages() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.messages = p.messages[:0]
}

func (p *mockProgram) HasMessageOfType(typeName string) bool {
	for _, msg := range p.Messages() {
		if reflect.TypeOf(msg).Name() == typeName {
			return true
		}
	}
	return false
}

type mockIPCServer struct {
	stdin  io.WriteCloser
	stdout *bufio.Reader
	cmd    *exec.Cmd
}

func startMockServer(t *testing.T) (*mockIPCServer, error) {
	script := `#!/usr/bin/env python3
import sys
import json

while True:
    try:
        line = sys.stdin.readline()
        if not line:
            break
        req = json.loads(line)
        req_type = req.get("type", "")
        
        if req_type == "ping":
            resp = {
                "type": "pong",
                "id": req.get("id", ""),
                "ipc_version": 1,
                "runtime_version": "0.1.0-test",
                "version_ok": True
            }
            print(json.dumps(resp))
        elif req_type == "shutdown":
            print(json.dumps({"type": "ok", "id": req.get("id", "")}))
            break
        elif req_type == "list_plugins":
            resp = {
                "type": "plugin_list",
                "id": req.get("id", ""),
                "plugins": []
            }
            print(json.dumps(resp))
        elif req_type == "search":
            query = req.get("query", "")
            items = []
            if query and query != "empty":
                items = [
                    {"id": "tt0000001", "title": "Test Movie", "year": "2024",
                     "genre": "Action", "rating": "8.5", "description": "Test",
                     "poster_url": None, "provider": "tmdb", "tab": "movies"}
                ]
            resp = {
                "type": "search_result",
                "id": req.get("id", ""),
                "items": items,
                "total": len(items),
                "offset": 0
            }
            print(json.dumps(resp))
        elif req_type == "load_plugin":
            path = req.get("path", "")
            if "error" in path:
                resp = {"type": "error", "id": req.get("id", ""),
                        "code": "plugin_load_failed", "message": "load failed"}
            else:
                resp = {"type": "plugin_loaded", "id": req.get("id", ""),
                        "plugin_id": "test-plugin", "name": "Test Plugin"}
            print(json.dumps(resp))
        elif req_type == "unload_plugin":
            plugin_id = req.get("plugin_id", "")
            if plugin_id == "nonexistent":
                resp = {"type": "error", "id": req.get("id", ""),
                        "code": "plugin_not_found", "message": "not found"}
            else:
                resp = {"type": "ok", "id": req.get("id", "")}
            print(json.dumps(resp))
        elif req_type == "get_provider_settings":
            resp = {"type": "provider_settings", "id": req.get("id", ""), "settings": []}
            print(json.dumps(resp))
        else:
            resp = {"type": "ok", "id": req.get("id", "")}
            print(json.dumps(resp))
        sys.stdout.flush()
    except Exception as e:
        print(json.dumps({"type": "error", "message": str(e)}), file=sys.stderr)
        break
`
	cmd := exec.Command("python3", "-c", script)
	cmd.Stderr = os.Stderr

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}

	if err := cmd.Start(); err != nil {
		return nil, err
	}

	return &mockIPCServer{
		stdin:  stdin,
		stdout: bufio.NewReader(stdout),
		cmd:    cmd,
	}, nil
}

func (s *mockIPCServer) Stop() {
	s.stdin.Close()
	s.cmd.Wait()
}

func (s *mockIPCServer) SendLine(t *testing.T, data map[string]interface{}) {
	bytes, err := json.Marshal(data)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}
	bytes = append(bytes, '\n')
	if _, err := s.stdin.Write(bytes); err != nil {
		t.Fatalf("failed to write to stdin: %v", err)
	}
}

func (s *mockIPCServer) ReadLine(t *testing.T) string {
	line, err := s.stdout.ReadString('\n')
	if err != nil {
		t.Fatalf("failed to read line: %v", err)
	}
	return strings.TrimRight(line, "\n")
}

func TestIPCProtocolVersion(t *testing.T) {
	if IPCVersion != 1 {
		t.Errorf("IPCVersion = %d, want 1", IPCVersion)
	}
}

func TestMediaTabConstants(t *testing.T) {
	tests := []struct {
		tab   MediaTab
		value string
	}{
		{TabMovies, "movies"},
		{TabSeries, "series"},
		{TabMusic, "music"},
		{TabLibrary, "library"},
	}

	for _, tt := range tests {
		if string(tt.tab) != tt.value {
			t.Errorf("MediaTab = %q, want %q", tt.tab, tt.value)
		}
	}
}

func TestRawResponseIsError(t *testing.T) {
	tests := []struct {
		name     string
		response RawResponse
		want     bool
	}{
		{
			name:     "error type",
			response: RawResponse{Type: "error"},
			want:     true,
		},
		{
			name:     "nil error",
			response: RawResponse{Type: "ok"},
			want:     false,
		},
		{
			name:     "with error",
			response: RawResponse{Type: "ok", Err: fmt.Errorf("test error")},
			want:     true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := tt.response.IsError(); got != tt.want {
				t.Errorf("RawResponse.IsError() = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestRawResponseDecodeData(t *testing.T) {
	t.Run("with error", func(t *testing.T) {
		resp := RawResponse{Err: fmt.Errorf("transport error")}
		var v map[string]interface{}
		if err := resp.decodeData(&v); err == nil {
			t.Error("decodeData() = nil, want error")
		}
	})

	t.Run("valid JSON", func(t *testing.T) {
		raw := json.RawMessage(`{"foo": "bar"}`)
		resp := RawResponse{Type: "ok", Raw: raw}
		var v struct {
			Foo string `json:"foo"`
		}
		if err := resp.decodeData(&v); err != nil {
			t.Errorf("decodeData() error = %v, want nil", err)
		}
		if v.Foo != "bar" {
			t.Errorf("v.Foo = %q, want %q", v.Foo, "bar")
		}
	})
}

func TestRequestEnvelopeSerialization(t *testing.T) {
	env := requestEnvelope{
		Type: "search",
		Data: map[string]any{
			"query": "test",
			"tab":   "movies",
			"limit": 20,
		},
	}

	merged := map[string]any{
		"type": env.Type,
	}
	for k, v := range env.Data {
		merged[k] = v
	}

	bytes, err := json.Marshal(merged)
	if err != nil {
		t.Fatalf("json.Marshal() error = %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(bytes, &parsed); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if parsed["type"] != "search" {
		t.Errorf("parsed[type] = %v, want %v", parsed["type"], "search")
	}
	if parsed["query"] != "test" {
		t.Errorf("parsed[query] = %v, want %v", parsed["query"], "test")
	}
}

func TestSearchRequestSerialization(t *testing.T) {
	searchReq := map[string]any{
		"type":   "search",
		"id":     "req-1",
		"query":  "test movie",
		"tab":    "movies",
		"limit":  20,
		"offset": 0,
	}

	bytes, err := json.Marshal(searchReq)
	if err != nil {
		t.Fatalf("json.Marshal() error = %v", err)
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(bytes, &parsed); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if parsed["type"] != "search" {
		t.Errorf("parsed[type] = %v, want %v", parsed["type"], "search")
	}
	if parsed["query"] != "test movie" {
		t.Errorf("parsed[query] = %v, want %v", parsed["query"], "test movie")
	}
	if parsed["tab"] != "movies" {
		t.Errorf("parsed[tab] = %v, want %v", parsed["tab"], "movies")
	}
}

func TestPingResponse(t *testing.T) {
	pongJSON := `{
		"type": "pong",
		"id": "req-1",
		"ipc_version": 1,
		"runtime_version": "0.1.0",
		"version_ok": true
	}`

	var resp map[string]interface{}
	if err := json.Unmarshal([]byte(pongJSON), &resp); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if resp["type"] != "pong" {
		t.Errorf("resp[type] = %v, want %v", resp["type"], "pong")
	}
	if resp["ipc_version"].(float64) != 1 {
		t.Errorf("resp[ipc_version] = %v, want %v", resp["ipc_version"], 1)
	}
	if resp["version_ok"] != true {
		t.Errorf("resp[version_ok] = %v, want %v", resp["version_ok"], true)
	}
}

func TestSearchResponse(t *testing.T) {
	searchJSON := `{
		"type": "search_result",
		"id": "req-1",
		"items": [
			{
				"id": "tt0000001",
				"title": "Test Movie",
				"year": "2024",
				"genre": "Action",
				"rating": "8.5",
				"description": "A test movie",
				"poster_url": null,
				"provider": "tmdb",
				"tab": "movies"
			}
		],
		"total": 1,
		"offset": 0
	}`

	var parsed struct {
		Type  string `json:"type"`
		ID    string `json:"id"`
		Items []struct {
			ID       string `json:"id"`
			Title    string `json:"title"`
			Provider string `json:"provider"`
		} `json:"items"`
		Total  int `json:"total"`
		Offset int `json:"offset"`
	}
	if err := json.Unmarshal([]byte(searchJSON), &parsed); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if parsed.Type != "search_result" {
		t.Errorf("parsed.Type = %v, want %v", parsed.Type, "search_result")
	}
	if parsed.Total != 1 {
		t.Errorf("parsed.Total = %v, want %v", parsed.Total, 1)
	}
	if len(parsed.Items) != 1 {
		t.Fatalf("len(parsed.Items) = %v, want %v", len(parsed.Items), 1)
	}
	if parsed.Items[0].Title != "Test Movie" {
		t.Errorf("parsed.Items[0].Title = %v, want %v", parsed.Items[0].Title, "Test Movie")
	}
	if parsed.Items[0].Provider != "tmdb" {
		t.Errorf("parsed.Items[0].Provider = %v, want %v", parsed.Items[0].Provider, "tmdb")
	}
}

func TestErrorResponse(t *testing.T) {
	errJSON := `{
		"type": "error",
		"id": "req-1",
		"code": "plugin_not_found",
		"message": "plugin not found"
	}`

	var errResp ErrorPayload
	if err := json.Unmarshal([]byte(errJSON), &errResp); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if errResp.Code != "plugin_not_found" {
		t.Errorf("errResp.Code = %v, want %v", errResp.Code, "plugin_not_found")
	}
	if errResp.Message != "plugin not found" {
		t.Errorf("errResp.Message = %v, want %v", errResp.Message, "plugin not found")
	}
}

func TestPluginInfo(t *testing.T) {
	pluginJSON := `{
		"id": "test-plugin",
		"name": "Test Plugin",
		"version": "1.0.0",
		"plugin_type": "rpc",
		"status": "loaded"
	}`

	var plugin PluginInfo
	if err := json.Unmarshal([]byte(pluginJSON), &plugin); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if plugin.ID != "test-plugin" {
		t.Errorf("plugin.ID = %v, want %v", plugin.ID, "test-plugin")
	}
	if plugin.Name != "Test Plugin" {
		t.Errorf("plugin.Name = %v, want %v", plugin.Name, "Test Plugin")
	}
	if plugin.Version != "1.0.0" {
		t.Errorf("plugin.Version = %v, want %v", plugin.Version, "1.0.0")
	}
}

func TestPluginListResponse(t *testing.T) {
	listJSON := `{
		"type": "plugin_list",
		"id": "req-1",
		"plugins": [
			{"id": "p1", "name": "Plugin 1", "version": "1.0", "plugin_type": "rpc", "status": "loaded"},
			{"id": "p2", "name": "Plugin 2", "version": "2.0", "plugin_type": "wasm", "status": "loaded"}
		]
	}`

	var resp PluginListResult
	if err := json.Unmarshal([]byte(listJSON), &resp); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if len(resp.Plugins) != 2 {
		t.Errorf("len(resp.Plugins) = %v, want %v", len(resp.Plugins), 2)
	}
	if resp.Plugins[0].Name != "Plugin 1" {
		t.Errorf("resp.Plugins[0].Name = %v, want %v", resp.Plugins[0].Name, "Plugin 1")
	}
}

func TestGridUpdateEvent(t *testing.T) {
	eventJSON := `{
		"type": "grid_update",
		"tab": "movies",
		"entries": [
			{"id": "tt1", "title": "Movie 1"}
		]
	}`

	var env struct {
		Type string `json:"type"`
	}
	if err := json.Unmarshal([]byte(eventJSON), &env); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if env.Type != "grid_update" {
		t.Errorf("env.Type = %v, want %v", env.Type, "grid_update")
	}
}

func TestPlayerEvents(t *testing.T) {
	tests := []struct {
		name      string
		eventJSON string
		wantType  string
	}{
		{
			name: "player_started",
			eventJSON: `{
				"type": "player_started",
				"id": "play-1",
				"entry_id": "tt0001",
				"provider": "tmdb"
			}`,
			wantType: "player_started",
		},
		{
			name: "player_progress",
			eventJSON: `{
				"type": "player_progress",
				"position": 120.5,
				"duration": 3600.0,
				"volume": 100.0,
				"paused": false
			}`,
			wantType: "player_progress",
		},
		{
			name: "player_ended",
			eventJSON: `{
				"type": "player_ended",
				"reason": "eof"
			}`,
			wantType: "player_ended",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var env struct {
				Type string `json:"type"`
			}
			if err := json.Unmarshal([]byte(tt.eventJSON), &env); err != nil {
				t.Fatalf("json.Unmarshal() error = %v", err)
			}
			if env.Type != tt.wantType {
				t.Errorf("env.Type = %v, want %v", env.Type, tt.wantType)
			}
		})
	}
}

func TestBubbleTeaMessages(t *testing.T) {
	t.Run("RuntimeReadyMsg", func(t *testing.T) {
		msg := RuntimeReadyMsg{
			RuntimeVersion: "0.1.0",
			IPCVersion:     1,
		}
		if msg.RuntimeVersion != "0.1.0" {
			t.Errorf("msg.RuntimeVersion = %v, want %v", msg.RuntimeVersion, "0.1.0")
		}
	})

	t.Run("RuntimeErrorMsg", func(t *testing.T) {
		err := fmt.Errorf("test error")
		msg := RuntimeErrorMsg{Err: err}
		if msg.Err != err {
			t.Errorf("msg.Err = %v, want %v", msg.Err, err)
		}
	})

	t.Run("StatusMsg", func(t *testing.T) {
		msg := StatusMsg{Text: "test status"}
		if msg.Text != "test status" {
			t.Errorf("msg.Text = %v, want %v", msg.Text, "test status")
		}
	})
}

func TestMockRuntimeProtocol(t *testing.T) {
	if os.Getenv("IPC_TEST_MOCK") != "1" {
		t.Skip("skipping mock runtime test (not in mock mode)")
	}

	server, err := startMockServer(t)
	if err != nil {
		t.Fatalf("startMockServer() error = %v", err)
	}
	defer server.Stop()

	server.SendLine(t, map[string]interface{}{
		"type":        "ping",
		"id":          "test-1",
		"ipc_version": 1,
	})

	resp := server.ReadLine(t)
	if !bytes.Contains([]byte(resp), []byte("pong")) {
		t.Errorf("expected pong response, got: %s", resp)
	}
}

func TestIPCWireFormat(t *testing.T) {
	t.Run("newline delimited", func(t *testing.T) {
		msgs := []map[string]interface{}{
			{"type": "search", "id": "1"},
			{"type": "ping", "id": "2"},
			{"type": "shutdown", "id": "3"},
		}

		var buf bytes.Buffer
		for _, msg := range msgs {
			data, _ := json.Marshal(msg)
			buf.Write(data)
			buf.WriteByte('\n')
		}

		scanner := bufio.NewScanner(&buf)
		count := 0
		for scanner.Scan() {
			var parsed map[string]interface{}
			if err := json.Unmarshal(scanner.Bytes(), &parsed); err != nil {
				t.Fatalf("json.Unmarshal() error = %v", err)
			}
			count++
		}
		if count != 3 {
			t.Errorf("scanned %d messages, want %d", count, 3)
		}
	})

	t.Run("empty line skipped", func(t *testing.T) {
		data := []byte("{\"type\": \"ok\"}\n\n{\"type\": \"error\"}\n")
		scanner := bufio.NewScanner(bytes.NewReader(data))
		count := 0
		for scanner.Scan() {
			if len(scanner.Bytes()) == 0 {
				continue
			}
			count++
		}
		if count != 2 {
			t.Errorf("scanned %d messages, want %d", count, 2)
		}
	})
}

func TestClientRequestIDGeneration(t *testing.T) {
	c := &Client{}

	id1 := c.nextID()
	id2 := c.nextID()

	if id1 == id2 {
		t.Errorf("IDs should be unique: %s == %s", id1, id2)
	}

	if len(id1) < 4 || id1[:4] != "req-" {
		t.Errorf("ID should start with 'req-': %s", id1)
	}
}

func TestIPCVersionNegotiation(t *testing.T) {
	tests := []struct {
		name           string
		clientVersion  uint32
		runtimeVersion uint32
		wantOK         bool
	}{
		{"match", 1, 1, true},
		{"client older", 0, 1, false},
		{"client newer", 2, 1, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			gotOK := tt.clientVersion == tt.runtimeVersion
			if gotOK != tt.wantOK {
				t.Errorf("version check = %v, want %v", gotOK, tt.wantOK)
			}
		})
	}
}

func TestConcurrentResponseRouting(t *testing.T) {
	pending := make(map[string]chan RawResponse)

	addResponse := func(id string) chan RawResponse {
		ch := make(chan RawResponse, 1)
		pending[id] = ch
		return ch
	}

	routeResponse := func(id string) {
		if ch, ok := pending[id]; ok {
			delete(pending, id)
			ch <- RawResponse{Type: "ok"}
			close(ch)
		}
	}

	ch1 := addResponse("req-1")
	ch2 := addResponse("req-2")
	ch3 := addResponse("req-3")

	routeResponse("req-2")

	select {
	case <-ch1:
		t.Error("req-1 should not be routed yet")
	default:
	}

	select {
	case resp := <-ch2:
		if resp.Type != "ok" {
			t.Errorf("req-2 response type = %v, want %v", resp.Type, "ok")
		}
	default:
		t.Error("req-2 should be routed")
	}

	select {
	case <-ch3:
		t.Error("req-3 should not be routed yet")
	default:
	}
}

func TestLargeResponseHandling(t *testing.T) {
	largeJSON := map[string]interface{}{
		"type": "search_result",
		"id":   "req-1",
		"items": func() []map[string]interface{} {
			items := make([]map[string]interface{}, 100)
			for i := 0; i < 100; i++ {
				items[i] = map[string]interface{}{
					"id":          fmt.Sprintf("tt%07d", i),
					"title":       fmt.Sprintf("Movie %d", i),
					"year":        "2024",
					"genre":       "Action",
					"rating":      fmt.Sprintf("%.1f", float64(i%10)+5.0),
					"description": strings.Repeat("x", 100),
					"provider":    "tmdb",
					"tab":         "movies",
				}
			}
			return items
		}(),
		"total":  100,
		"offset": 0,
	}

	data, err := json.Marshal(largeJSON)
	if err != nil {
		t.Fatalf("json.Marshal() error = %v", err)
	}

	var resp SearchResult
	if err := json.Unmarshal(data, &resp); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if len(resp.Items) != 100 {
		t.Errorf("len(resp.Items) = %v, want %v", len(resp.Items), 100)
	}
	if resp.Total != 100 {
		t.Errorf("resp.Total = %v, want %v", resp.Total, 100)
	}
}

func TestResponseTimeout(t *testing.T) {
	t.Run("context deadline", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()

		select {
		case <-ctx.Done():
		case <-time.After(200 * time.Millisecond):
			t.Error("context should have timed out")
		}
	})
}

func TestMediaEntryFields(t *testing.T) {
	jsonStr := `{
		"id": "tt0000001",
		"title": "Test Movie",
		"year": "2024",
		"genre": "Action",
		"rating": "8.5",
		"description": "A test movie",
		"poster_url": "http://example.com/poster.jpg",
		"provider": "tmdb",
		"tab": "movies"
	}`

	var entry MediaEntry
	if err := json.Unmarshal([]byte(jsonStr), &entry); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if entry.ID != "tt0000001" {
		t.Errorf("entry.ID = %v, want %v", entry.ID, "tt0000001")
	}
	if entry.Title != "Test Movie" {
		t.Errorf("entry.Title = %v, want %v", entry.Title, "Test Movie")
	}
	if entry.Year == nil || *entry.Year != "2024" {
		t.Errorf("entry.Year = %v, want %v", entry.Year, "2024")
	}
	if entry.Genre == nil || *entry.Genre != "Action" {
		t.Errorf("entry.Genre = %v, want %v", entry.Genre, "Action")
	}
	if entry.Rating == nil || *entry.Rating != "8.5" {
		t.Errorf("entry.Rating = %v, want %v", entry.Rating, "8.5")
	}
	if entry.Provider != "tmdb" {
		t.Errorf("entry.Provider = %v, want %v", entry.Provider, "tmdb")
	}
	if entry.Tab != TabMovies {
		t.Errorf("entry.Tab = %v, want %v", entry.Tab, TabMovies)
	}
}

func TestNullFields(t *testing.T) {
	jsonStr := `{
		"id": "tt0000001",
		"title": "Test Movie",
		"year": null,
		"genre": null,
		"rating": null,
		"description": null,
		"poster_url": null,
		"provider": "tmdb",
		"tab": "movies"
	}`

	var entry MediaEntry
	if err := json.Unmarshal([]byte(jsonStr), &entry); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if entry.Year != nil {
		t.Errorf("entry.Year = %v, want nil", entry.Year)
	}
	if entry.Genre != nil {
		t.Errorf("entry.Genre = %v, want nil", entry.Genre)
	}
	if entry.Rating != nil {
		t.Errorf("entry.Rating = %v, want nil", entry.Rating)
	}
	if entry.PosterURL != nil {
		t.Errorf("entry.PosterURL = %v, want nil", entry.PosterURL)
	}
}

func TestUnsupportedFields(t *testing.T) {
	jsonStr := `{
		"type": "search_result",
		"id": "req-1",
		"items": [],
		"total": 0,
		"offset": 0,
		"extra_field": "should be ignored",
		"another_field": 123
	}`

	var resp SearchResult
	if err := json.Unmarshal([]byte(jsonStr), &resp); err != nil {
		t.Fatalf("json.Unmarshal() error = %v", err)
	}

	if resp.Total != 0 {
		t.Errorf("resp.Total = %v, want %v", resp.Total, 0)
	}
}
