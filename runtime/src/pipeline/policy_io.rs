//! Persistent storage for the user's stream selection policy.
//!
//! Policy is read from and written to `~/.config/stui/stream_policy.json`.
//! Missing or invalid files are silently replaced with `StreamPreferences::default()`.

use crate::ipc::StreamPreferencesWire;
use crate::quality::StreamPreferences;

fn policy_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("stui")
        .join("stream_policy.json")
}

pub fn load_stream_policy() -> StreamPreferences {
    let path = policy_path();
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return StreamPreferences::default(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

pub fn save_stream_policy(prefs: &StreamPreferences) -> std::io::Result<()> {
    let path = policy_path();
    std::fs::create_dir_all(path.parent().expect("policy_io: output path has no parent directory"))?;
    let data = serde_json::to_vec_pretty(prefs).expect("serialize StreamPreferences");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, &path)
}

impl From<StreamPreferences> for StreamPreferencesWire {
    fn from(p: StreamPreferences) -> Self {
        StreamPreferencesWire {
            prefer_protocol: p.prefer_protocol,
            max_resolution:  p.max_resolution,
            max_size_mb:     p.max_size_mb,
            min_seeders:     p.min_seeders,
            avoid_labels:    p.avoid_labels,
            prefer_hdr:      p.prefer_hdr,
            prefer_codecs:   p.prefer_codecs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default_policy() {
        let prefs = StreamPreferences::default();
        let json = serde_json::to_string(&prefs).unwrap();
        let decoded: StreamPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.max_size_mb, prefs.max_size_mb);
        assert_eq!(decoded.min_seeders, prefs.min_seeders);
        assert_eq!(decoded.prefer_hdr, prefs.prefer_hdr);
    }

    #[test]
    fn wire_roundtrip() {
        let prefs = StreamPreferences {
            prefer_protocol: Some("torrent".to_string()),
            max_resolution:  Some("1080p".to_string()),
            max_size_mb:     4096,
            min_seeders:     5,
            avoid_labels:    vec!["cam".to_string()],
            prefer_hdr:      true,
            prefer_codecs:   vec!["hevc".to_string()],
        };
        let wire: StreamPreferencesWire = prefs.clone().into();
        let back: StreamPreferences = wire.into();
        assert_eq!(back.prefer_protocol, prefs.prefer_protocol);
        assert_eq!(back.max_size_mb,     prefs.max_size_mb);
        assert_eq!(back.avoid_labels,    prefs.avoid_labels);
    }
}
