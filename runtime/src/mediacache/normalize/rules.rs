//! Algorithmic normalization rules.

use super::unusual_case::is_unusual_case;

const SMALL_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "in", "on", "at", "to", "for", "by", "vs", "nor",
    "per", "via",
];

/// Smart title case: capitalize principal words, lowercase small words,
/// always capitalize first/last word. Respects the unusual-case heuristic.
pub fn smart_title_case(input: &str) -> String {
    let trimmed = collapse_whitespace(input.trim());
    if trimmed.is_empty() {
        return String::new();
    }
    if is_unusual_case(&trimmed) {
        return trimmed;
    }

    let words: Vec<&str> = trimmed.split(' ').collect();
    let last_idx = words.len().saturating_sub(1);

    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let lower = w.to_ascii_lowercase();
            let is_small = SMALL_WORDS.contains(&lower.as_str());
            if is_small && i != 0 && i != last_idx {
                lower
            } else {
                capitalize_word(w)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize_word(w: &str) -> String {
    if w.contains('/') {
        return w
            .split('/')
            .map(capitalize_simple)
            .collect::<Vec<_>>()
            .join("/");
    }
    capitalize_simple(w)
}

fn capitalize_simple(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = String::with_capacity(w.len());
            out.extend(first.to_uppercase());
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
            out
        }
    }
}

/// Collapse runs of whitespace to a single space. Does not trim ends.
pub fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out
}

/// Parse `N/M` forms to the integer N. Returns 0 on no parseable integer.
pub fn parse_track_or_disc(raw: &str) -> u32 {
    raw.split('/')
        .next()
        .unwrap_or("")
        .trim()
        .parse::<u32>()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titlecase_basic() {
        assert_eq!(smart_title_case("the beatles"), "The Beatles");
    }
    #[test]
    fn titlecase_small_words() {
        assert_eq!(smart_title_case("a hard day's night"), "A Hard Day's Night");
    }
    #[test]
    fn titlecase_small_word_last() {
        assert_eq!(
            smart_title_case("the long and winding road"),
            "The Long and Winding Road"
        );
    }
    #[test]
    fn titlecase_preserves_acronym() {
        assert_eq!(smart_title_case("AC/DC"), "AC/DC");
    }
    #[test]
    fn titlecase_preserves_stylized() {
        assert_eq!(smart_title_case("deadmau5"), "deadmau5");
    }
    #[test]
    fn titlecase_trims() {
        assert_eq!(smart_title_case("  pink floyd  "), "Pink Floyd");
    }
    #[test]
    fn titlecase_collapses() {
        assert_eq!(smart_title_case("the    wall"), "The Wall");
    }
    #[test]
    fn titlecase_fixes_screaming_multiword() {
        assert_eq!(
            smart_title_case("DARK SIDE OF THE MOON"),
            "Dark Side of the Moon"
        );
    }
    #[test]
    fn titlecase_empty() {
        assert_eq!(smart_title_case(""), "");
    }
    #[test]
    fn titlecase_slash_compound() {
        assert_eq!(smart_title_case("girl/boy song"), "Girl/Boy Song");
    }

    #[test]
    fn collapse_basic() {
        assert_eq!(collapse_whitespace("a  b   c"), "a b c");
    }
    #[test]
    fn collapse_tabs() {
        assert_eq!(collapse_whitespace("a\tb\nc"), "a b c");
    }

    #[test]
    fn track_plain() {
        assert_eq!(parse_track_or_disc("3"), 3);
    }
    #[test]
    fn track_slash() {
        assert_eq!(parse_track_or_disc("3/12"), 3);
    }
    #[test]
    fn track_leading_zero() {
        assert_eq!(parse_track_or_disc("003"), 3);
    }
    #[test]
    fn track_empty() {
        assert_eq!(parse_track_or_disc(""), 0);
    }
    #[test]
    fn track_junk() {
        assert_eq!(parse_track_or_disc("foo"), 0);
    }
}
