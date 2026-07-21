/// Check if a URL is a UniFi cloud console URL
pub fn is_cloud_console_url(url: &str) -> bool {
    url.starts_with("https://unifi.ui.com/consoles/")
        || url.starts_with("http://unifi.ui.com/consoles/")
}

/// Maximum number of characters of a cloud host ID to show before it is
/// truncated for tabular display.
pub const CLOUD_HOST_ID_DISPLAY_CHARS: usize = 60;

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
    fn test_is_cloud_console_url() {
        assert!(is_cloud_console_url(
            "https://unifi.ui.com/consoles/ABC123/network"
        ));
        assert!(!is_cloud_console_url("https://192.168.1.1:8443"));
    }

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

    #[test]
    fn test_truncate_cloud_host_id_budget_is_character_based() {
        let ascii_id = "a".repeat(CLOUD_HOST_ID_DISPLAY_CHARS);
        assert_eq!(
            truncate_for_display(&ascii_id, CLOUD_HOST_ID_DISPLAY_CHARS),
            ascii_id
        );

        let long_ascii_id = "a".repeat(CLOUD_HOST_ID_DISPLAY_CHARS + 1);
        assert_eq!(
            truncate_for_display(&long_ascii_id, CLOUD_HOST_ID_DISPLAY_CHARS),
            format!("{ascii_id}...")
        );

        // A multi-byte ID well under the character budget must survive intact
        // even though its byte length exceeds the budget.
        let multibyte_id = "日".repeat(CLOUD_HOST_ID_DISPLAY_CHARS - 1);
        assert_eq!(
            truncate_for_display(&multibyte_id, CLOUD_HOST_ID_DISPLAY_CHARS),
            multibyte_id
        );
    }
}
