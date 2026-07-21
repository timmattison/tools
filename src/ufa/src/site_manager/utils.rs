/// Check if a URL is a UniFi cloud console URL
pub fn is_cloud_console_url(url: &str) -> bool {
    url.starts_with("https://unifi.ui.com/consoles/")
        || url.starts_with("http://unifi.ui.com/consoles/")
}

/// Maximum number of characters of a cloud host ID to show before it is
/// truncated for tabular display.
pub const CLOUD_HOST_ID_DISPLAY_CHARS: usize = 60;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::truncate_for_display;

    #[test]
    fn test_is_cloud_console_url() {
        assert!(is_cloud_console_url(
            "https://unifi.ui.com/consoles/ABC123/network"
        ));
        assert!(!is_cloud_console_url("https://192.168.1.1:8443"));
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
