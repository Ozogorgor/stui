//! URL helpers for librqbit's HTTP streaming API.
//!
//! Confirmed against librqbit 8.1.1 (`src/http_api/handlers/mod.rs`): the
//! per-file stream route is `/torrents/{id}/stream/{file_id}`, with a
//! `/{*filename}` suffix variant that some players prefer for filename
//! sniffing. We use the bare form; mpv resolves codecs from Content-Type
//! headers librqbit already sets.

pub fn stream_url_for(base: &str, torrent_id: usize, file_idx: usize) -> String {
    format!("{base}/torrents/{torrent_id}/stream/{file_idx}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_format_is_stable() {
        assert_eq!(
            stream_url_for("http://127.0.0.1:1234", 7, 0),
            "http://127.0.0.1:1234/torrents/7/stream/0"
        );
    }
}
