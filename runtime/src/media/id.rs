//! Namespaced media identifier.
//!
//! A `MediaId` encodes both the provider that owns this item and the
//! provider-local key, separated by a colon:
//!
//!   `tmdb:movie:tt0816692`
//!   `imdb:tt0816692`
//!   `prowlarr:1234abcd`
//!   `local:/home/user/Movies/Interstellar.mkv`
//!
//! This avoids collisions when items from different providers are merged into
//! a single catalog and lets the engine route resolution requests to the
//! correct provider without extra metadata.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaId {
    /// The provider namespace (e.g. "tmdb", "imdb", "prowlarr", "local").
    pub namespace: String,
    /// The provider-local key (e.g. IMDB ID, internal numeric ID, file path).
    pub key: String,
}

impl MediaId {
    pub fn new(namespace: impl Into<String>, key: impl Into<String>) -> Self {
        MediaId {
            namespace: namespace.into(),
            key: key.into(),
        }
    }

    /// Parse a `"namespace:key"` string. If there is no colon, the whole
    /// string is treated as the key with namespace `"unknown"`.
    pub fn parse(s: &str) -> Self {
        match s.find(':') {
            Some(i) => MediaId {
                namespace: s[..i].to_string(),
                key: s[i + 1..].to_string(),
            },
            None => MediaId {
                namespace: "unknown".to_string(),
                key: s.to_string(),
            },
        }
    }

    #[allow(dead_code)] // pub API: used by IPC layer
    /// Serialize to the canonical `"namespace:key"` wire form.
    pub fn to_string_id(&self) -> String {
        format!("{}:{}", self.namespace, self.key)
    }
}

impl fmt::Display for MediaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.key)
    }
}

impl From<&str> for MediaId {
    fn from(s: &str) -> Self {
        MediaId::parse(s)
    }
}

impl From<String> for MediaId {
    fn from(s: String) -> Self {
        MediaId::parse(&s)
    }
}
