use chrono::{DateTime, FixedOffset, TimeZone};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Conversation {
    pub chat_id: i64,
    pub chat_identifier: String,
    pub display_name: Option<String>,
    pub is_group: bool,
    pub participants: Vec<String>,
    pub last_message_date: Option<DateTime<FixedOffset>>,
    pub last_message_preview: String,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Message {
    pub message_id: i64,
    pub text: Option<String>,
    pub sender: String,
    pub is_from_me: bool,
    pub date: Option<DateTime<FixedOffset>>,
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
    apple_timestamp_in_zone(apple_ns, &chrono::Local)
}

/// Convert Apple Core Data timestamp (nanoseconds since 2001-01-01) to a
/// DateTime in `zone`.
///
/// Each date gets the UTC offset of `zone` at that date, not the offset of
/// now. Thus a summer date keeps its summer offset in winter, and a date near
/// midnight keeps its own local date. An instant has one local time, but if
/// the zone gives two, the earlier one wins, and no case panics.
pub fn apple_timestamp_in_zone<Tz: TimeZone>(
    apple_ns: i64,
    zone: &Tz,
) -> Option<DateTime<FixedOffset>> {
    if apple_ns == 0 {
        return None;
    }
    let unix_seconds = apple_ns / 1_000_000_000 + 978_307_200;
    zone.timestamp_opt(unix_seconds, 0)
        .earliest()
        .map(|dt| dt.fixed_offset())
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
        // 2024-01-15 12:00:00 UTC is Unix 1_705_320_000. The Apple epoch
        // (2001-01-01 00:00:00 UTC) is Unix 978_307_200. The difference is
        // 727_012_800 seconds. Noon UTC keeps the UTC date 2024-01-15.
        let ns = 727_012_800_i64 * 1_000_000_000;
        let dt = apple_timestamp_to_datetime(ns).unwrap();
        // Convert to UTC for a timezone-independent check
        let dt_utc = dt.with_timezone(&chrono::Utc);
        assert_eq!(dt_utc.format("%Y-%m-%d").to_string(), "2024-01-15");
    }

    /// The Apple date of the instant that `utc` gives as RFC 3339 text.
    fn apple_ns_at(utc: &str) -> i64 {
        let instant = DateTime::parse_from_rfc3339(utc).expect("RFC 3339 instant");
        crate::test_fixture::apple_ns(instant.timestamp(), 0)
    }

    /// The RFC 3339 text of the instant `utc` in `zone`.
    fn rfc3339_in<Tz: TimeZone>(utc: &str, zone: &Tz) -> String {
        apple_timestamp_in_zone(apple_ns_at(utc), zone)
            .expect("date")
            .to_rfc3339()
    }

    #[test]
    fn a_summer_date_gets_the_summer_offset_of_its_zone() {
        assert_eq!(
            rfc3339_in("2024-07-15T14:30:00Z", &chrono_tz::America::New_York),
            "2024-07-15T10:30:00-04:00"
        );
    }

    #[test]
    fn a_winter_date_gets_the_winter_offset_of_its_zone() {
        assert_eq!(
            rfc3339_in("2024-01-15T14:30:00Z", &chrono_tz::America::New_York),
            "2024-01-15T09:30:00-05:00"
        );
    }

    #[test]
    fn a_date_near_midnight_keeps_its_own_local_date() {
        // 00:30 on 16 July in New York. With the winter offset, it reads
        // 23:30 on 15 July.
        assert_eq!(
            rfc3339_in("2024-07-16T04:30:00Z", &chrono_tz::America::New_York),
            "2024-07-16T00:30:00-04:00"
        );
        // 23:30 on 15 January in New York. With the summer offset, it reads
        // 00:30 on 16 January.
        assert_eq!(
            rfc3339_in("2024-01-16T04:30:00Z", &chrono_tz::America::New_York),
            "2024-01-15T23:30:00-05:00"
        );
    }

    #[test]
    fn the_default_zone_is_local_with_the_offset_of_each_date() {
        // The zone of the process, read and not set, thus parallel tests see
        // the same zone. A zone with no summer time gives one offset for both.
        for utc in ["2024-07-15T14:30:00Z", "2024-01-15T14:30:00Z"] {
            let instant = DateTime::parse_from_rfc3339(utc).expect("RFC 3339 instant");
            let expected = chrono::Local
                .from_utc_datetime(&instant.naive_utc())
                .fixed_offset();
            assert_eq!(
                apple_timestamp_to_datetime(apple_ns_at(utc)).map(|d| d.to_rfc3339()),
                Some(expected.to_rfc3339()),
                "{utc}"
            );
        }
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
        assert_eq!(
            truncate_preview("hello world this is long", 10),
            "hello w..."
        );
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
