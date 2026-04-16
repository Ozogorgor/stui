//! Year extraction from messy MPD `Date:` values.

/// Extract a 4-digit year from a date string.
///
/// Returns the first run of four consecutive digits whose first digit is
/// `1` or `2` (i.e. a year in 1000–2999). Empty string on no match.
/// Handles `2017`, `2017-05-03`, `May 2017`, `03-05-2017`, etc.
pub fn extract_year(date: &str) -> String {
    if date.is_empty() { return String::new(); }
    let bytes = date.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        let slice = &bytes[i..i + 4];
        if slice.iter().all(|b| b.is_ascii_digit()) {
            let first = slice[0];
            if first == b'1' || first == b'2' {
                return std::str::from_utf8(slice).unwrap_or("").to_string();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn plain_year() { assert_eq!(extract_year("2017"), "2017"); }
    #[test] fn iso_date() { assert_eq!(extract_year("2017-05-03"), "2017"); }
    #[test] fn month_year() { assert_eq!(extract_year("May 2017"), "2017"); }
    #[test] fn dmy() { assert_eq!(extract_year("03-05-2017"), "2017"); }
    #[test] fn empty() { assert_eq!(extract_year(""), ""); }
    #[test] fn two_digit_year() { assert_eq!(extract_year("03-05-19"), ""); }
    #[test] fn junk() { assert_eq!(extract_year("not a date"), ""); }
    #[test] fn year_out_of_range() { assert_eq!(extract_year("3500"), ""); }
    #[test] fn first_match_wins() { assert_eq!(extract_year("1999 or 2001?"), "1999"); }
    #[test] fn compact_date() { assert_eq!(extract_year("20170503"), "2017"); }
}
