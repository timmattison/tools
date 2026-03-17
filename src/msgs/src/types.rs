use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Conversation {
    pub chat_id: i64,
    pub chat_identifier: String,
    pub display_name: Option<String>,
    pub is_group: bool,
    pub participants: Vec<String>,
    pub last_message_date: DateTime<FixedOffset>,
    pub last_message_preview: String,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Message {
    pub message_id: i64,
    pub text: Option<String>,
    pub sender: String,
    pub is_from_me: bool,
    pub date: DateTime<FixedOffset>,
    pub date_read: Option<DateTime<FixedOffset>>,
    pub is_audio: bool,
    pub attachments: Vec<AttachmentInfo>,
    pub reply_to_guid: Option<String>,
    pub associated_emoji: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AttachmentInfo {
    pub attachment_id: i64,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub total_bytes: i64,
    pub transfer_name: Option<String>,
    pub is_sticker: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchResult {
    pub message: Message,
    pub conversation: Conversation,
    pub context_before: Vec<Message>,
    pub context_after: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ExportResult {
    pub export_path: String,
    pub message_count: i64,
    pub attachment_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DbStatus {
    pub accessible: bool,
    pub message_count: Option<i64>,
    pub error: Option<String>,
}

/// Convert Apple Core Data timestamp (nanoseconds since 2001-01-01) to DateTime.
pub fn apple_timestamp_to_datetime(apple_ns: i64) -> Option<DateTime<FixedOffset>> {
    if apple_ns == 0 {
        return None;
    }
    let unix_seconds = apple_ns / 1_000_000_000 + 978_307_200;
    let local = chrono::Local::now().fixed_offset().timezone();
    chrono::DateTime::from_timestamp(unix_seconds, 0)
        .map(|dt| dt.with_timezone(&local))
}

/// Truncate a string to max_chars characters, appending "..." if truncated.
/// Uses chars() for UTF-8 safety per repo guidelines.
pub fn truncate_preview(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apple_timestamp_zero_returns_none() {
        assert!(apple_timestamp_to_datetime(0).is_none());
    }

    #[test]
    fn test_apple_timestamp_known_value() {
        // 2024-01-15 12:00:00 UTC = 725_760_000 + (14*86400 + 43200) = 726_969_600 seconds since 2001-01-01
        // Using noon UTC ensures the date is 2024-01-15 in all UTC-aligned timezones.
        // Seconds since 2001-01-01 00:00:00 UTC for 2024-01-15 12:00:00 UTC:
        //   725_760_000 (to 2024-01-01) + 14*86400 + 43200 = 726_972_000... let's compute exactly:
        // 2001-01-01 unix = 978_307_200
        // 2024-01-15 12:00:00 UTC unix = 1705320000
        // diff = 1705320000 - 978307200 = 727012800 seconds
        let ns = 727_012_800_i64 * 1_000_000_000;
        let dt = apple_timestamp_to_datetime(ns).unwrap();
        // Convert to UTC for a timezone-independent check
        let dt_utc = dt.with_timezone(&chrono::Utc);
        assert_eq!(dt_utc.format("%Y-%m-%d").to_string(), "2024-01-15");
    }

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate_preview("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate_preview("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        assert_eq!(truncate_preview("hello world this is long", 10), "hello w...");
    }

    #[test]
    fn test_truncate_utf8_emoji() {
        assert_eq!(truncate_preview("🎉🎊🎁🎈🎂", 4), "🎉...");
    }

    #[test]
    fn test_truncate_utf8_japanese() {
        assert_eq!(truncate_preview("日本語テスト", 5), "日本...");
    }

    #[test]
    fn test_truncate_utf8_accented() {
        assert_eq!(truncate_preview("café au lait", 8), "café ...");
    }
}
