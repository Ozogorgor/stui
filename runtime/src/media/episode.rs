//! Episode-specific metadata, attached to `MediaItem` when `media_type == Episode`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub season:        u32,
    pub episode:       u32,
    pub series_title:  String,
    pub series_id:     Option<String>,  // parent series MediaId string
    pub runtime_mins:  Option<u32>,
    pub air_date:      Option<String>,  // ISO 8601
}
