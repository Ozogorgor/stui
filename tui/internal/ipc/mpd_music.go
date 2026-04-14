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
	Duration float64 `json:"duration,omitempty"`
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
type MpdAlbum struct {
	Title  string `json:"title"`
	Artist string `json:"artist"`
	Year   string `json:"year"`
}

// MpdSong is one entry in the library track list or a saved playlist.
type MpdSong struct {
	Title    string  `json:"title"`
	Artist   string  `json:"artist"`
	Album    string  `json:"album"`
	Duration float64 `json:"duration"` // seconds
	File     string  `json:"file"`
}

// MpdLibraryResultMsg is dispatched by MpdListArtists / MpdListAlbums / MpdListSongs.
// Exactly one of Artists/Albums/Songs is populated depending on the request.
type MpdLibraryResultMsg struct {
	Artists   []MpdArtist
	Albums    []MpdAlbum
	Songs     []MpdSong
	ForArtist string // the artist filter used; empty means "all artists"
	ForAlbum  string // the album filter used; empty means no album filter
	Err       error
}

// MpdListArtists requests all artists in the MPD database.
func (c *Client) MpdListArtists() {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type": "mpd_list",
			"id":   id,
			"what": "artists",
		})
		raw := receiveWithTimeout(ch)
		var msg MpdLibraryResultMsg
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Artists []MpdArtist `json:"artists"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Artists = payload.Artists
			}
		}
		c.send(msg)
	}()
}

// MpdListAlbums requests albums from the MPD database, filtered by artist.
// Pass artist="" to list all albums.
func (c *Client) MpdListAlbums(artist string) {
	go func() {
		id := c.nextID()
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
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Albums []MpdAlbum `json:"albums"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
				msg.Albums = payload.Albums
			}
		}
		c.send(msg)
	}()
}

// MpdListSongs requests tracks for a specific album.
// Pass artist="" to skip the artist filter.
func (c *Client) MpdListSongs(artist, album string) {
	go func() {
		id := c.nextID()
		ch := c.sendWithID(id, map[string]any{
			"type":   "mpd_list",
			"id":     id,
			"what":   "songs",
			"artist": artist,
			"album":  album,
		})
		raw := receiveWithTimeout(ch)
		var msg MpdLibraryResultMsg
		msg.ForArtist = artist
		msg.ForAlbum = album
		if raw.Err != nil {
			msg.Err = raw.Err
		} else if raw.Type == "error" {
			var ep ErrorPayload
			_ = json.Unmarshal(raw.Raw, &ep)
			msg.Err = fmt.Errorf("%s: %s", ep.Code, ep.Message)
		} else {
			var payload struct {
				Songs []MpdSong `json:"songs"`
			}
			if err := json.Unmarshal(raw.Raw, &payload); err != nil {
				msg.Err = err
			} else {
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
