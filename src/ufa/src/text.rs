//! Fitting text into a budget without ever splitting a character.

/// Truncate `value` so that it is at most `max_chars` characters long,
/// appending an ellipsis (`...`) only when truncation actually happened.
///
/// Truncation happens on character boundaries, so multi-byte UTF-8 input
/// (Japanese, emoji, accented Latin, ...) is never split mid-character.
///
/// # Arguments
///
/// * `value` - The string to truncate.
/// * `max_chars` - Maximum number of characters to keep from `value`.
///
/// # Returns
///
/// `value` unchanged when it is `max_chars` characters or shorter, otherwise
/// the first `max_chars` characters followed by `...`.
pub fn truncate_for_display(value: &str, max_chars: usize) -> String {
    let mut kept: String = value.chars().take(max_chars).collect();

    // Only an ellipsis when at least one character was actually dropped.
    if value.chars().nth(max_chars).is_some() {
        kept.push_str("...");
    }

    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_ascii_shorter_than_limit_is_unchanged() {
        assert_eq!(truncate_for_display("abc", 10), "abc");
    }

    #[test]
    fn test_truncate_ascii_exactly_at_limit_is_unchanged() {
        assert_eq!(truncate_for_display("abcdefghij", 10), "abcdefghij");
    }

    #[test]
    fn test_truncate_ascii_one_char_over_limit_is_truncated() {
        assert_eq!(truncate_for_display("abcdefghijk", 10), "abcdefghij...");
    }

    #[test]
    fn test_truncate_japanese_crossing_boundary_does_not_panic() {
        // "日本語テスト" is 6 characters but 18 bytes; a 4-byte-offset slice
        // would land in the middle of the second character.
        let truncated = truncate_for_display("日本語テスト", 4);
        assert_eq!(truncated, "日本語テ...");
        assert_eq!(truncated.chars().count(), 7);
    }

    #[test]
    fn test_truncate_japanese_shorter_than_limit_is_unchanged() {
        // 6 characters but 18 bytes: a byte-length comparison would wrongly
        // consider this over a 10-character limit.
        assert_eq!(truncate_for_display("日本語テスト", 10), "日本語テスト");
    }

    #[test]
    fn test_truncate_emoji_crossing_boundary_does_not_panic() {
        // Each emoji is 4 bytes; a 2-byte-offset slice would split one.
        let truncated = truncate_for_display("🎉🎊🎁🎈🎂", 2);
        assert_eq!(truncated, "🎉🎊...");
        assert_eq!(truncated.chars().count(), 5);
    }

    #[test]
    fn test_truncate_accented_latin_crossing_boundary() {
        // "café au lait" has a 2-byte 'é' at character index 3.
        assert_eq!(truncate_for_display("café au lait", 5), "café ...");
    }
}
