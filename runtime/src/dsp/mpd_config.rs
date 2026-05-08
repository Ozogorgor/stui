//! MPD FIFO output configuration helpers.
//!
//! MPD routes audio to the stui DSP pipeline via a named FIFO (pipe).
//! This module generates the required `audio_output` stanza and can patch
//! the user's `mpd.conf` automatically.

use std::path::{Path, PathBuf};

/// Default FIFO path for the stui DSP loop.
pub const DEFAULT_FIFO_PATH: &str = "/tmp/stui-mpd-dsp.fifo";

/// Name used in MPD's `audio_output` block — matched by `ensure_dsp_output_enabled`.
pub const FIFO_OUTPUT_NAME: &str = "stui-dsp";

/// Generate an `audio_output` stanza for mpd.conf.
///
/// The format string `<sample_rate>:16:2` means 16-bit LE stereo at `sample_rate` Hz,
/// which matches what [`super::run_mpd_dsp_loop`] expects.
pub fn fifo_stanza(fifo_path: &str, sample_rate: u32) -> String {
    // Escape backslashes and double-quotes so the path cannot break the config syntax.
    let safe_path = fifo_path.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "\n# stui DSP FIFO output — added by stui-runtime\n\
         audio_output {{\n\
         \ttype\t\"fifo\"\n\
         \tname\t\"{FIFO_OUTPUT_NAME}\"\n\
         \tpath\t\"{safe_path}\"\n\
         \tformat\t\"{sample_rate}:16:2\"\n\
         }}\n"
    )
}

/// Scan `body` (the text after the opening `{` of an `audio_output` block) for
/// the matching closing `}`, returning its byte offset.
///
/// The scanner is aware of:
/// - double-quoted strings (`"…"` with `\"` escapes) — braces inside are ignored
/// - line comments (`#` to end of line) — braces inside are ignored
///
/// Returns 0 if no closing brace is found (malformed config).
fn find_block_end(body: &str) -> usize {
    #[derive(PartialEq)]
    enum State {
        Normal,
        InString,
        InComment,
    }

    let mut state = State::Normal;
    let mut depth = 1usize;
    let mut prev_was_backslash = false;
    let mut end = 0;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match state {
            State::InString => {
                if prev_was_backslash {
                    prev_was_backslash = false;
                } else if ch == '\\' {
                    prev_was_backslash = true;
                } else if ch == '"' {
                    state = State::Normal;
                }
            }
            State::InComment => {
                if ch == '\n' {
                    state = State::Normal;
                }
            }
            State::Normal => {
                prev_was_backslash = false;
                match ch {
                    '"' => state = State::InString,
                    '#' => state = State::InComment,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    end
}

/// Return `true` if `mpd_conf` text already contains the stui FIFO output block.
///
/// Uses comment/string-aware block parsing to avoid false positives from braces
/// inside comments or quoted strings, or from the name appearing in a comment.
pub fn conf_has_stui_output(mpd_conf: &str) -> bool {
    let mut search = mpd_conf;
    loop {
        let block_start = match search.find("audio_output") {
            Some(i) => i,
            None => return false,
        };
        let after = &search[block_start..];
        let open = match after.find('{') {
            Some(i) => i,
            None => return false,
        };
        let body = &after[open + 1..];
        let end = find_block_end(body);
        let block = &body[..end];
        // Check if any non-comment line in this block has name = "stui-dsp".
        let found = block.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#')
                && trimmed.starts_with("name")
                && trimmed.contains(FIFO_OUTPUT_NAME)
        });
        if found {
            return true;
        }
        let consumed = block_start + open + 1 + end + 1;
        if consumed >= search.len() {
            return false;
        }
        search = &search[consumed..];
    }
}

/// Append the stui FIFO stanza to the mpd.conf at `conf_path` if not already present.
///
/// Returns `Ok(true)` if the file was modified, `Ok(false)` if already present.
///
/// Uses a single file handle for both the read and the write to eliminate the
/// TOCTOU window that would exist if we read with one handle and opened a second
/// for appending.
pub fn ensure_mpd_conf(
    conf_path: &Path,
    fifo_path: &str,
    sample_rate: u32,
) -> std::io::Result<bool> {
    use std::io::{Read, Write};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(conf_path)?;
    let mut existing = String::new();
    file.read_to_string(&mut existing)?;
    if conf_has_stui_output(&existing) {
        return Ok(false);
    }
    write!(file, "{}", fifo_stanza(fifo_path, sample_rate))?;
    Ok(true)
}

/// Parse the sample rate from the `format` field of the stui-dsp `audio_output` stanza.
///
/// The format string written by [`fifo_stanza`] is `"<sample_rate>:16:2"`.
/// Returns `None` if the stanza is absent or the format string cannot be parsed.
pub fn parse_fifo_sample_rate(conf_path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(conf_path).ok()?;
    // Walk audio_output blocks and find the one that contains FIFO_OUTPUT_NAME.
    // This avoids picking up the name from a comment or another block.
    let mut search = text.as_str();
    loop {
        let block_start = search.find("audio_output")?;
        let after = &search[block_start..];
        // Find the matching closing brace via brace counting.
        let open = after.find('{')?;
        let body = &after[open + 1..];
        let end = find_block_end(body);
        let block = &body[..end];
        let has_name = block.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#')
                && trimmed.starts_with("name")
                && trimmed.contains(FIFO_OUTPUT_NAME)
        });
        if has_name {
            // Found the right block — extract the format line.
            // The format line looks like:  format  "44100:16:2"
            let fmt_line = block.lines().find(|l| {
                let t = l.trim();
                !t.starts_with('#') && t.starts_with("format")
            })?;
            let quoted = fmt_line.split('"').nth(1)?;
            let rate_str = quoted.split(':').next()?;
            return rate_str.parse().ok();
        }
        // Advance past this block and keep searching.
        let consumed = block_start + open + 1 + end + 1;
        if consumed >= search.len() {
            return None;
        }
        search = &search[consumed..];
    }
}

/// Try to locate the user's `mpd.conf` by checking common paths.
pub fn find_mpd_conf() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let xdg_config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));

    let candidates = [
        PathBuf::from(&xdg_config).join("mpd/mpd.conf"),
        PathBuf::from(&home).join(".mpd/mpd.conf"),
        PathBuf::from("/etc/mpd.conf"),
        PathBuf::from("/etc/mpd/mpd.conf"),
    ];

    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stanza_contains_name_and_path() {
        let s = fifo_stanza("/tmp/test.fifo", 44100);
        assert!(s.contains(FIFO_OUTPUT_NAME));
        assert!(s.contains("/tmp/test.fifo"));
        assert!(s.contains("44100:16:2"));
    }

    #[test]
    fn detect_existing_output() {
        let conf = "audio_output {\n\ttype \"fifo\"\n\tname \"stui-dsp\"\n}\n";
        assert!(conf_has_stui_output(conf));
        assert!(!conf_has_stui_output("audio_output { type \"pulse\" }"));
    }

    #[test]
    fn parse_sample_rate_from_stanza() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "{}", fifo_stanza("/tmp/test.fifo", 48000)).unwrap();
        let rate = parse_fifo_sample_rate(tmp.path());
        assert_eq!(rate, Some(48000));
    }
}
