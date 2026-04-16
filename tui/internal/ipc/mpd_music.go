package ipc

// mpd_music.go — MPD data types and client methods for the Music sub-tabs
// (Queue, Library, Playlists, Directories).
//
// Wire protocol additions:
//   mpd_get_queue          → MpdQueueResultMsg
//   mpd_browse             → MpdDirResultMsg
//   mpd_list (artists)     → MpdLibraryResultMsg
//   mpd_list (albums)      → MpdLibraryResultMsg
//   mpd_list (songs)       → MpdLibraryResultMsg
//   mpd_get_playlists      → MpdPlaylistsResultMsg
//   mpd_get_playlist       → MpdPlaylistTracksResultMsg
//   mpd_queue_changed      (unsolicited push) → MpdQueueChangedMsg

import (
	"encoding/json"
	"fmt"

	"github.com/stui/stui/pkg/log"
)

// ── Queue ─────────────────────────────────────────────────────────────────────

// MpdTrack is one song in the MPD playback queue.
type MpdTrack struct {
	ID       uint32  `json:"id"`               // MPD song ID (stable within a queue session)
	Pos      uint32  `json:"pos"`              // 0-based queue position
	Title    string  `json:"title"`
	Artist   string  `json:"artist"`
	Album    string  `json:"album"`
	Duration float64 `json:"duration"`         // seconds; 0 if unknown
	File     string  `json:"file"`             // path relative to MPD music root
}

// MpdQueueResultMsg is dispatched when MpdGetQueue completes.
type MpdQueueResultMsg struct {
	Tracks []MpdTrack
	Err    error
}

// MpdQueueChangedMsg is pushed (unsolicited) when the MPD queue changes.
type MpdQueueChangedMsg struct{}

// MpdGetQueue requests the full current MPD queue.
// Results arrive as MpdQueueResultMsg.
func (c *Client) MpdGetQueue() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "mpd_get_queue", "id": id})
		raw := receiveWithTimeout(ch)
		var msg MpdQueueResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Tracks []MpdTrack `json:"tracks"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Tracks = payload.Tracks
			}
		}
		c.send(msg)
	}()
}

// ── Directory browser ─────────────────────────────────────────────────────────

// MpdDirEntry is one item returned by an MPD directory browse request.
type MpdDirEntry struct {
	Name     string  `json:"name"`
	IsDir    bool    `json:"is_dir"`
	File     string  `json:"file,omitempty"`
	Title    string  `json:"title,omitempty"`
	Artist   string  `json:"artist,omitempty"`
	Album    string  `json:"album,omitempty"`
	Duration  float64 `json:"duration,omitempty"`
	RawArtist string  `json:"raw_artist,omitempty"`
	RawAlbum  string  `json:"raw_album,omitempty"`
	RawTitle  string  `json:"raw_title,omitempty"`
}

// MpdDirResultMsg is dispatched when MpdBrowseDir completes.
type MpdDirResultMsg struct {
	Path    string       // the path that was browsed
	Entries []MpdDirEntry
	Err     error
}

// MpdBrowseDir requests directory contents from the MPD music database.
// path is relative to the MPD music root; "" lists the root.
func (c *Client) MpdBrowseDir(path string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "mpd_browse",
			"id":   id,
			"path": path,
		})
		raw := receiveWithTimeout(ch)
		var msg MpdDirResultMsg
		msg.Path = path
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Entries []MpdDirEntry `json:"entries"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Entries = payload.Entries
			}
		}
		c.send(msg)
	}()
}

// ── Library ───────────────────────────────────────────────────────────────────

// MpdArtist is one entry in the MPD library artist list.
type MpdArtist struct {
	Name string `json:"name"`
}

// MpdAlbum is one entry in the MPD library album list.
// Year is the 4-digit display year (may be empty). Date holds the raw MPD
// `Date:` tag value (e.g. "1996-11-01") and is used to disambiguate
// multiple releases of the same album when listing that release's tracks.
type MpdAlbum struct {
	Title  string `json:"title"`
	Artist string `json:"artist"`
	Year      string `json:"year"`
	Date      string `json:"date"`
	RawArtist string `json:"raw_artist,omitempty"`
	RawTitle  string `json:"raw_title,omitempty"`
}

// MpdSong is one entry in the library track list or a saved playlist.
type MpdSong struct {
	Title    string  `json:"title"`
	Artist   string  `json:"artist"`
	Album    string  `json:"album"`
	Duration  float64 `json:"duration"` // seconds
	File      string  `json:"file"`
	RawArtist string  `json:"raw_artist,omitempty"`
	RawAlbum  string  `json:"raw_album,omitempty"`
	RawTitle  string  `json:"raw_title,omitempty"`
}

// MpdLibraryResultMsg is dispatched by MpdListArtists / MpdListAlbums / MpdListSongs.
// Exactly one of Artists/Albums/Songs is populated depending on the request.
type MpdLibraryResultMsg struct {
	Artists   []MpdArtist
	Albums    []MpdAlbum
	Songs     []MpdSong
	ForArtist string // the artist filter used; empty means "all artists"
	ForAlbum  string // the album filter used; empty means no album filter
	ForDate   string // the raw MPD Date filter used; empty means no date filter
	Err       error
}

// MpdListArtists requests all artists in the MPD database.
func (c *Client) MpdListArtists() {
	go func() {
		id := c.nextID()
		log.Info("ipc: MpdListArtists send", "id", id)
		ch := c.sendWithID(id, map[string]any{
			"type": "mpd_list",
			"id":   id,
			"what": "artists",
		})
		raw := receiveWithTimeout(ch)
		var msg MpdLibraryResultMsg
		if raw.Err != nil {
			log.Warn("ipc: MpdListArtists transport error", "id", id, "err", raw.Err)
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			log.Warn("ipc: MpdListArtists runtime error", "id", id, "code", ep.Code, "msg", ep.Message)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Artists []MpdArtist `json:"artists"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				log.Warn("ipc: MpdListArtists decode error", "id", id, "err", err,
					"rawType", raw.Type, "rawSnippet", snippet(raw.Raw))
				msg.Err = err
			} else {
				log.Info("ipc: MpdListArtists ok", "id", id, "artists", len(payload.Artists))
				msg.Artists = payload.Artists
			}
		}
		c.send(msg)
	}()
}

// snippet returns a short preview of a JSON payload for log lines.
func snippet(b []byte) string {
	const max = 200
	if len(b) <= max {
		return string(b)
	}
	return string(b[:max]) + "…"
}

// MpdListAlbums requests albums from the MPD database, filtered by artist.
// Pass artist="" to list all albums.
func (c *Client) MpdListAlbums(artist string) {
	go func() {
		id := c.nextID()
		log.Info("ipc: MpdListAlbums send", "id", id, "artist", artist)
		ch := c.sendWithID(id, map[string]any{
			"type":   "mpd_list",
			"id":     id,
			"what":   "albums",
			"artist": artist,
		})
		raw := receiveWithTimeout(ch)
		var msg MpdLibraryResultMsg
		msg.ForArtist = artist
		if raw.Err != nil {
			log.Warn("ipc: MpdListAlbums transport error", "id", id, "err", raw.Err)
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			log.Warn("ipc: MpdListAlbums runtime error", "id", id, "code", ep.Code, "msg", ep.Message)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Albums []MpdAlbum `json:"albums"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				log.Warn("ipc: MpdListAlbums decode error", "id", id, "err", err, "snippet", snippet(raw.Raw))
				msg.Err = err
			} else {
				log.Info("ipc: MpdListAlbums ok", "id", id, "albums", len(payload.Albums))
				msg.Albums = payload.Albums
			}
		}
		c.send(msg)
	}()
}

// MpdListSongs requests tracks for a specific album release.
// Pass artist="" to skip the artist filter. Pass date="" to skip the date
// filter (legacy / when the caller doesn't know the release date). Date
// must be the raw MPD `Date:` string from MpdAlbum.Date — it's used to
// pick one specific release when two share Album+Artist tags (e.g. a
// 1996 original and a 2007 remaster).
func (c *Client) MpdListSongs(artist, album, date string) {
	go func() {
		id := c.nextID()
		log.Info("ipc: MpdListSongs send", "id", id, "artist", artist, "album", album, "date", date)
		ch := c.sendWithID(id, map[string]any{
			"type":   "mpd_list",
			"id":     id,
			"what":   "songs",
			"artist": artist,
			"album":  album,
			"date":   date,
		})
		raw := receiveWithTimeout(ch)
		var msg MpdLibraryResultMsg
		msg.ForArtist = artist
		msg.ForAlbum = album
		msg.ForDate = date
		if raw.Err != nil {
			log.Warn("ipc: MpdListSongs transport error", "id", id, "err", raw.Err)
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			log.Warn("ipc: MpdListSongs runtime error", "id", id, "code", ep.Code, "msg", ep.Message)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Songs []MpdSong `json:"songs"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				log.Warn("ipc: MpdListSongs decode error", "id", id, "err", err, "snippet", snippet(raw.Raw))
				msg.Err = err
			} else {
				log.Info("ipc: MpdListSongs ok", "id", id, "songs", len(payload.Songs))
				msg.Songs = payload.Songs
			}
		}
		c.send(msg)
	}()
}

// ── Playlists ─────────────────────────────────────────────────────────────────

// MpdSavedPlaylist describes one named MPD saved playlist.
type MpdSavedPlaylist struct {
	Name     string `json:"name"`
	Modified string `json:"modified"` // ISO timestamp; may be empty
}

// MpdPlaylistsResultMsg is dispatched when MpdGetPlaylists completes.
type MpdPlaylistsResultMsg struct {
	Playlists []MpdSavedPlaylist
	Err       error
}

// MpdPlaylistTracksResultMsg carries the track list for one named playlist.
type MpdPlaylistTracksResultMsg struct {
	Name   string
	Tracks []MpdSong
	Err    error
}

// MpdGetPlaylists requests the list of saved MPD playlists.
func (c *Client) MpdGetPlaylists() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{"type": "mpd_get_playlists", "id": id})
		raw := receiveWithTimeout(ch)
		var msg MpdPlaylistsResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Playlists []MpdSavedPlaylist `json:"playlists"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Playlists = payload.Playlists
			}
		}
		c.send(msg)
	}()
}

// MpdGetPlaylistTracks requests the track list for one named saved playlist.
func (c *Client) MpdGetPlaylistTracks(name string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "mpd_get_playlist",
			"id":   id,
			"name": name,
		})
		raw := receiveWithTimeout(ch)
		var msg MpdPlaylistTracksResultMsg
		msg.Name = name
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Tracks []MpdSong `json:"tracks"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Tracks = payload.Tracks
			}
		}
		c.send(msg)
	}()
}

// ── Tag normalization ────────────────────────────────────────────────────────

// TagWriteScope defines what scope to normalize.
type TagWriteScope struct {
	Kind   string `json:"kind"`             // "album" | "artist" | "library"
	Artist string `json:"artist,omitempty"`
	Album  string `json:"album,omitempty"`
	Date   string `json:"date,omitempty"`
}

// TagDiffRow is one field-level change in a tag normalization preview.
type TagDiffRow struct {
	File     string `json:"file"`
	Field    string `json:"field"`
	OldValue string `json:"old_value"`
	NewValue string `json:"new_value"`
}

// MarkTagExceptionResultMsg is dispatched when MarkTagException completes.
type MarkTagExceptionResultMsg struct {
	Added bool
	Err   error
}

// MarkTagException adds a raw tag value to the user's exception list.
func (c *Client) MarkTagException(field, rawValue string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":      "mark_tag_exception",
			"id":        id,
			"field":     field,
			"raw_value": rawValue,
		})
		raw := receiveWithTimeout(ch)
		var msg MarkTagExceptionResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Added bool `json:"added"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Added = payload.Added
			}
		}
		c.send(msg)
	}()
}

// ActionAPreviewResultMsg is dispatched when ActionATagsPreview completes.
type ActionAPreviewResultMsg struct {
	JobID      string
	Rows       []TagDiffRow
	TotalFiles int
	Err        error
}

// ActionATagsPreview computes a normalize-vs-raw diff for a scope without writing.
func (c *Client) ActionATagsPreview(scope TagWriteScope) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":  "action_a_tags_preview",
			"id":    id,
			"scope": scope,
		})
		raw := receiveWithTimeout(ch)
		var msg ActionAPreviewResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				JobID      string       `json:"job_id"`
				Rows       []TagDiffRow `json:"rows"`
				TotalFiles int          `json:"total_files"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.JobID = payload.JobID
				msg.Rows = payload.Rows
				msg.TotalFiles = payload.TotalFiles
			}
		}
		c.send(msg)
	}()
}

// ActionAApplyResultMsg is dispatched when ActionATagsApply completes.
type ActionAApplyResultMsg struct {
	Succeeded        int
	Failed           int
	SkippedCancelled int
	Failures         []string
	RescanPath       string
	Err              error
}

// ActionATagsApply writes normalized tags to files for a previously-previewed job.
func (c *Client) ActionATagsApply(jobID string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":   "action_a_tags_apply",
			"id":     id,
			"job_id": jobID,
		})
		raw := receiveWithTimeout(ch)
		var msg ActionAApplyResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Succeeded        int      `json:"succeeded"`
				Failed           int      `json:"failed"`
				SkippedCancelled int      `json:"skipped_cancelled"`
				Failures         []string `json:"failures"`
				RescanPath       string   `json:"rescan_path"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Succeeded = payload.Succeeded
				msg.Failed = payload.Failed
				msg.SkippedCancelled = payload.SkippedCancelled
				msg.Failures = payload.Failures
				msg.RescanPath = payload.RescanPath
			}
		}
		c.send(msg)
	}()
}

// ActionACancelResultMsg is dispatched when ActionATagsCancel completes.
type ActionACancelResultMsg struct {
	Cancelled bool
	Err       error
}

// ActionATagsCancel cancels an in-progress Action A job.
func (c *Client) ActionATagsCancel(jobID string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":   "action_a_tags_cancel",
			"id":     id,
			"job_id": jobID,
		})
		raw := receiveWithTimeout(ch)
		var msg ActionACancelResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Cancelled bool `json:"cancelled"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Cancelled = payload.Cancelled
			}
		}
		c.send(msg)
	}()
}
