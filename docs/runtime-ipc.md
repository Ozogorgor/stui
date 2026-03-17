# stui-runtime IPC Protocol

## Transport

The Go TUI spawns `stui-runtime` as a child process and communicates via
**newline-delimited JSON** (NDJSON):

```
stdin  → one Request JSON object per line  (Go → Rust)
stdout → one Response JSON object per line (Rust → Go)
stderr → structured tracing logs (never parsed by Go)
```

## Request format

Every request has a `"type"` discriminant field:

```json
{ "type": "ping" }
{ "type": "list_plugins" }
{ "type": "load_plugin",   "path": "/home/user/.stui/plugins/example-provider" }
{ "type": "unload_plugin", "plugin_id": "<uuid>" }
{ "type": "search",  "id": "req-1", "query": "dune", "tab": "movies", "limit": 20, "offset": 0 }
{ "type": "resolve", "id": "req-2", "entry_id": "tt1234567", "provider": "example-provider" }
{ "type": "metadata","id": "req-3", "entry_id": "tt1234567", "provider": "example-provider" }
{ "type": "shutdown" }
```

## Response format

Every response has a `"type"` discriminant:

```json
{ "type": "pong" }
{ "type": "ok" }
{ "type": "plugin_list",     "plugins": [...] }
{ "type": "plugin_loaded",   "plugin_id": "<uuid>", "name": "example-provider" }
{ "type": "plugin_unloaded", "plugin_id": "<uuid>" }
{ "type": "search_result",   "id": "req-1", "items": [...], "total": 42, "offset": 0 }
{ "type": "resolve_result",  "id": "req-2", "stream_url": "https://...", "quality": "1080p", "subtitles": [] }
{ "type": "metadata_result", "id": "req-3", "entry": { ... } }
{ "type": "error",           "id": "req-1", "code": "SEARCH_FAILED", "message": "..." }
```

## Media tabs

The `tab` field in search requests must be one of:
`"movies"` `"series"` `"music"` `"library"`

## Error codes

| Code                | Meaning                              |
|---------------------|--------------------------------------|
| `PLUGIN_NOT_FOUND`  | No plugin with that id/name          |
| `PLUGIN_LOAD_FAILED`| Failed to parse manifest or entrypoint |
| `SEARCH_FAILED`     | Provider plugin returned an error    |
| `RESOLVE_FAILED`    | Resolver plugin returned an error    |
| `METADATA_FAILED`   | Metadata plugin returned an error    |
| `INVALID_REQUEST`   | Malformed JSON or missing fields     |
| `INTERNAL`          | Unexpected runtime error             |

## Logging

Set `STUI_LOG=debug` (or `info`, `warn`, `error`) to control log verbosity.
Logs are written to **stderr only** and never interfere with IPC on stdout.

## Example session

```
→ {"type":"ping"}
← {"type":"pong"}

→ {"type":"load_plugin","path":"/home/user/.stui/plugins/example-provider"}
← {"type":"plugin_loaded","plugin_id":"a1b2-...","name":"example-provider"}

→ {"type":"search","id":"s1","query":"dune","tab":"movies","limit":5,"offset":0}
← {"type":"search_result","id":"s1","items":[...],"total":12,"offset":0}

→ {"type":"shutdown"}
← {"type":"ok"}
```
