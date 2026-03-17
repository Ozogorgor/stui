//! Track/music-specific metadata, attached to `MediaItem` when `media_type == Track`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    pub artist:       String,
    pub album:        Option<String>,
    pub album_id:     Option<String>,   // parent album MediaId string
    pub track_number: Option<u32>,
    pub duration_secs: Option<u32>,
    pub isrc:         Option<String>,   // International Standard Recording Code
}
