//! Detection heuristic for stylized casing that should NOT be auto-title-cased.
//!
//! Flags single-token strings like `deadmau5`, `AC/DC`, `MGMT`, `iamamiwhoami`
//! so the algorithmic rules skip them. Exception list further overrides this
//! both ways.
//!
//! DESIGN DECISION: only SINGLE-TOKEN strings (no spaces) are eligible to be
//! flagged. Multi-word all-caps ("DARK SIDE OF THE MOON") is almost always a
//! bad tag — we title-case it. Users with legitimate multi-word stylized
//! names use the exception list.

/// Returns true if the string's casing looks stylized/deliberate (single
/// token only) and should be left alone by the title-case pass.
pub fn is_unusual_case(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) { return false; }

    let mut has_upper = false;
    let mut has_lower = false;
    let mut has_digit_in_letters = false;
    let mut has_separator = false;
    let mut last_was_alpha = false;

    for ch in trimmed.chars() {
        if ch.is_ascii_uppercase() { has_upper = true; last_was_alpha = true; }
        else if ch.is_ascii_lowercase() { has_lower = true; last_was_alpha = true; }
        else if ch.is_ascii_digit() {
            if last_was_alpha { has_digit_in_letters = true; }
            last_was_alpha = false;
        } else {
            if ch == '/' || ch == '-' || ch == '.' || ch == '!' { has_separator = true; }
            last_was_alpha = false;
        }
    }

    // Digits embedded mid-word: always stylized (deadmau5, 3OH!3).
    if has_digit_in_letters { return true; }

    // All-uppercase single token with a non-alpha separator, length >= 2: AC/DC, MGMT-Z.
    let alpha_count = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if has_upper && !has_lower && has_separator && alpha_count >= 2 {
        return true;
    }

    // All-uppercase single-token acronyms of length >= 3: MGMT, LCD.
    // Note: 2-letter tokens like "UK" are deliberately NOT flagged to avoid
    // false positives on country codes in titles.
    if has_upper && !has_lower && alpha_count >= 3 {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn acdc() { assert!(is_unusual_case("AC/DC")); }
    #[test] fn mgmt() { assert!(is_unusual_case("MGMT")); }
    #[test] fn deadmau5() { assert!(is_unusual_case("deadmau5")); }
    #[test] fn threeohthree() { assert!(is_unusual_case("3OH!3")); }
    #[test] fn empty() { assert!(!is_unusual_case("")); }
    #[test] fn whitespace() { assert!(!is_unusual_case("   ")); }
    #[test] fn all_lower() { assert!(!is_unusual_case("the beatles")); }
    #[test] fn title_case() { assert!(!is_unusual_case("The Beatles")); }
    #[test] fn sentence_case() { assert!(!is_unusual_case("Pink floyd")); }
    #[test] fn two_letter_caps() { assert!(!is_unusual_case("UK")); }
    #[test] fn mixed_with_apostrophe() { assert!(!is_unusual_case("don't stop")); }
    #[test] fn multi_word_screaming_caps_ignored() { assert!(!is_unusual_case("DARK SIDE OF THE MOON")); }
    #[test] fn single_token_screaming() { assert!(is_unusual_case("GENESIS")); }
}
