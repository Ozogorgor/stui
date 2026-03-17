# Universal Provider Protocol (UPP)

stui's plugin system is built around a single, content-agnostic provider
interface.  Any plugin that speaks UPP can deliver **any media type** —
movies, series, music, radio, anime, podcasts — through the same pipeline
that drives the entire application.

```
Search query
    │
    ▼
Provider fan-out (parallel)
    │   torrentio-rpc → StreamCandidates
    │   tmdb           → MediaItems
    │   soundcloud-rpc → MediaItems + StreamCandidates
    │   radio-rpc      → MediaItems + StreamCandidates
    ▼
Ranking engine (quality + reliability + latency)
    │
    ▼
Player (mpv handles HTTP, HLS, DASH, torrents, radio streams identically)
```

## Why One Interface?

Most streaming platforms are specialised:

| Platform  | Handles          |
|-----------|------------------|
| Stremio   | movies / TV only |
| Kodi      | local media + plugins |
| MPD       | music only |
| Jellyfin  | local library |

stui's architecture already supports a universal model because:

- `MediaItem` carries a `MediaSource` discriminant (Movie, Series, Track, Radio, Podcast…)
- `StreamCandidate` carries a `StreamProtocol` discriminant (Http, Torrent, Hls, Dash)
- mpv plays all of these natively
- The ranking engine is content-type-agnostic

## The Interface

Every UPP plugin implements three optional methods:

```json
{
  "method": "catalog",
  "params": { "tab": "movies", "query": "dune", "page": 1 }
}
→
{
  "items": [
    { "id": "tt0816692", "title": "Interstellar", "type": "movie", "year": 2014, ... }
  ]
}
```

```json
{
  "method": "streams.resolve",
  "params": { "id": "tt0816692", "type": "movie" }
}
→
{
  "streams": [
    { "url": "magnet:?xt=...", "protocol": "torrent", "quality": "1080p", "seeders": 142 }
  ]
}
```

```json
{
  "method": "subtitles.resolve",
  "params": { "id": "tt0816692", "lang": "eng" }
}
→
{
  "subtitles": [
    { "url": "https://...", "lang": "eng", "format": "srt" }
  ]
}
```

## Capability Declaration

Each plugin declares its capabilities at handshake time.  The runtime
uses this to route requests only to plugins that can handle them:

```json
{
  "method": "handshake",
  "params": { "protocol": "stui-rpc/1" }
}
→
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "1.0.0",
  "capabilities": {
    "catalog":   true,
    "search":    true,
    "streams":   true,
    "subtitles": false,
    "metadata":  false
  },
  "supported_media": ["movie", "series"]
}
```

A `null` / absent `supported_media` means "handles all types".

## MediaItem Structure

```json
{
  "id":          "tt0816692",
  "title":       "Interstellar",
  "type":        "movie",
  "year":        2014,
  "description": "...",
  "poster":      "https://image.tmdb.org/...",
  "imdb_id":     "tt0816692",
  "rating":      8.7,
  "duration":    169
}
```

`type` is one of:
`movie` | `series` | `episode` | `track` | `album` | `radio` | `podcast` | `video` | `local_file`

## StreamCandidate Structure

```json
{
  "url":        "magnet:?xt=urn:btih:...",
  "protocol":   "torrent",
  "quality":    "1080p",
  "bitrate":    8000,
  "size_bytes": 4831838208,
  "seeders":    142,
  "provider":   "torrentio",
  "audio_lang": "eng",
  "hdr":        false,
  "codec":      "h264"
}
```

`protocol` is one of: `http` | `torrent` | `hls` | `dash` | `magnet`

## Example Plugins

### Movie / Series (Torrentio)

```python
def handle_streams_resolve(params):
    id = params["id"]
    resp = requests.get(f"https://torrentio.strem.fun/stream/movie/{id}.json")
    return {"streams": [parse_stream(s) for s in resp["streams"]]}
```

### Music (SoundCloud-style)

```python
def handle_catalog(params):
    results = soundcloud.search(params["query"])
    return {"items": [
        {"id": t.id, "title": t.title, "type": "track", "duration": t.duration}
        for t in results
    ]}

def handle_streams_resolve(params):
    url = soundcloud.stream_url(params["id"])
    return {"streams": [{"url": url, "protocol": "http", "quality": "128kbps"}]}
```

### Radio

```python
def handle_catalog(params):
    stations = radio_browser.search(params.get("query", ""))
    return {"items": [
        {"id": s.stationuuid, "title": s.name, "type": "radio"}
        for s in stations
    ]}

def handle_streams_resolve(params):
    station = radio_browser.get(params["id"])
    return {"streams": [{"url": station.url, "protocol": "http"}]}
```

### Anime (AniList-style)

```python
def handle_catalog(params):
    results = anilist.search(params["query"], type="ANIME")
    return {"items": [
        {"id": str(a.id), "title": a.title.romaji, "type": "series"}
        for a in results
    ]}
```

## Installing Plugins

```bash
# RPC plugins (Python, Go, Node, any language)
mkdir -p ~/.stui/plugins/my-plugin
cp my-plugin.py ~/.stui/plugins/my-plugin/
cp plugin.json  ~/.stui/plugins/my-plugin/

# plugin.json minimal manifest:
{
  "id":      "my-plugin",
  "name":    "My Plugin",
  "version": "1.0.0",
  "entry":   "python3 my-plugin.py"
}
```

stui discovers plugins on startup and on directory changes (hot-reload).

## Future: Plugin Registry

```bash
# Conceptual — not yet implemented
stui plugin install torrentio
stui plugin install soundcloud
stui plugin install anime
stui plugin list
stui plugin update torrentio
```

Similar to `brew`, `npm`, or Kodi's addon repository — a community-driven
ecosystem of UPP-compatible plugins.

## Why This Architecture Is Powerful

| Feature                        | Stremio | Kodi | stui |
|--------------------------------|---------|------|------|
| Movies & TV                    | ✓       | ✓    | ✓    |
| Music                          | ✗       | ✓    | ✓    |
| Radio                          | ✗       | ✓    | ✓    |
| Podcasts                       | ✗       | ✓    | ✓    |
| Anime                          | plugin  | plugin| ✓   |
| Terminal-native                | ✗       | ✗    | ✓    |
| Plugin in any language         | ✗       | ✗    | ✓    |
| Unified pipeline (one ranker)  | ✗       | ✗    | ✓    |
| No Electron / no browser       | ✗       | ✗    | ✓    |

The UPP turns stui into a **terminal-native universal media hub**.
