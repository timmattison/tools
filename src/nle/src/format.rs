//! Text formats of the values that the interface shows.

/// Tenths of a second in one minute.
const TENTHS_PER_MINUTE: u64 = 600;
/// Tenths of a second in one hour.
const TENTHS_PER_HOUR: u64 = 36_000;
/// The largest count of tenths that the formatter accepts, about 3 million
/// years. A larger value stops here, so a bad file cannot cause garbage.
const MAX_TENTHS: f64 = 1e15;
/// The text for a timestamp that cannot be shown.
const UNKNOWN: &str = "unknown";
/// The `chrono` format of a record time.
const RECORDED_FORMAT: &str = "%Y-%m-%d %H:%M:%S";
/// The largest count of milliseconds that an `f64` holds as a whole number.
const MAX_MILLISECONDS: f64 = 9e15;

/// Formats a duration in seconds as `Xm Y.Ys`, or as `Xh Xm Y.Ys` from one hour.
///
/// The function rounds to tenths of a second before it splits the duration, so
/// 59.96 is `1m 0.0s` and never `0m 60.0s`. A value that is not finite, or
/// that is below zero, formats as `0m 0.0s`.
pub(crate) fn format_duration(total_seconds: f64) -> String {
    let tenths = tenths_of_a_second(total_seconds);
    let hours = tenths / TENTHS_PER_HOUR;
    let minutes = tenths % TENTHS_PER_HOUR / TENTHS_PER_MINUTE;
    let remainder = tenths % TENTHS_PER_MINUTE;
    let seconds = format!("{}.{}", remainder / 10, remainder % 10);
    if tenths >= TENTHS_PER_HOUR {
        format!("{hours}h {minutes}m {seconds}s")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

/// Formats a time in seconds for the ruler of the timeline: `m:ss` below one
/// hour, and `h:mm:ss` from one hour. With `tenths`, the text ends in the
/// tenths of a second, as in `0:01.5`.
///
/// The function rounds to tenths of a second first, as `format_duration`
/// does. A value that is not finite, or that is below zero, formats as zero.
pub(crate) fn format_clock(total_seconds: f64, tenths: bool) -> String {
    let count = tenths_of_a_second(total_seconds);
    let hours = count / TENTHS_PER_HOUR;
    let minutes = count % TENTHS_PER_HOUR / TENTHS_PER_MINUTE;
    let seconds = count % TENTHS_PER_MINUTE / 10;
    let clock = if count >= TENTHS_PER_HOUR {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    };
    if tenths {
        format!("{clock}.{}", count % 10)
    } else {
        clock
    }
}

/// Rounds `total_seconds` to whole tenths of a second. A value that is not
/// finite, or that is below zero, gives zero. A very large value stops at
/// `MAX_TENTHS`.
fn tenths_of_a_second(total_seconds: f64) -> u64 {
    if !total_seconds.is_finite() || total_seconds < 0.0 {
        return 0;
    }
    let tenths = (total_seconds * 10.0).round().min(MAX_TENTHS);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a whole number between 0 and MAX_TENTHS"
    )]
    let whole = tenths as u64;
    whole
}

/// Returns the part of `path` after the last `/` or `\`.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Formats a Unix timestamp in seconds as `%Y-%m-%d %H:%M:%S` in `zone`.
///
/// Returns `unknown` for a timestamp that is not finite or that chrono cannot
/// represent.
pub(crate) fn format_recorded_in<Tz>(timestamp: f64, zone: &Tz) -> String
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    if !timestamp.is_finite() {
        return UNKNOWN.to_owned();
    }
    let milliseconds = (timestamp * 1000.0).round();
    if milliseconds.abs() > MAX_MILLISECONDS {
        return UNKNOWN.to_owned();
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is a whole number inside the range of i64"
    )]
    let milliseconds = milliseconds as i64;
    zone.timestamp_millis_opt(milliseconds)
        .single()
        .map_or_else(
            || UNKNOWN.to_owned(),
            |time| time.format(RECORDED_FORMAT).to_string(),
        )
}

/// Formats a Unix timestamp in the local time zone of the machine.
pub(crate) fn format_recorded(timestamp: f64) -> String {
    format_recorded_in(timestamp, &chrono::Local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_splits_minutes_and_hours() {
        let cases = [
            (0.0, "0m 0.0s"),
            (59.96, "1m 0.0s"),
            (65.3, "1m 5.3s"),
            (119.96, "2m 0.0s"),
            (125.5, "2m 5.5s"),
            (3599.96, "1h 0m 0.0s"),
            (3725.5, "1h 2m 5.5s"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_duration(seconds), expected, "{seconds} seconds");
        }
    }

    #[test]
    fn format_duration_refuses_values_outside_the_range() {
        for seconds in [f64::NAN, -1.0, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(format_duration(seconds), "0m 0.0s", "{seconds}");
        }
    }

    #[test]
    fn format_duration_does_not_panic_on_a_huge_value() {
        assert!(!format_duration(1e300).is_empty());
    }

    #[test]
    fn format_clock_shows_minutes_and_seconds_and_hours_from_one_hour() {
        let cases = [
            (0.0, "0:00"),
            (5.0, "0:05"),
            (65.0, "1:05"),
            (600.0, "10:00"),
            (3599.0, "59:59"),
            (3600.0, "1:00:00"),
            (3725.0, "1:02:05"),
            (36000.0, "10:00:00"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_clock(seconds, false), expected, "{seconds} seconds");
        }
    }

    #[test]
    fn format_clock_shows_tenths_when_asked() {
        let cases = [
            (0.0, "0:00.0"),
            (0.1, "0:00.1"),
            (1.5, "0:01.5"),
            (0.30000000000000004, "0:00.3"),
            (59.96, "1:00.0"),
            (3600.2, "1:00:00.2"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_clock(seconds, true), expected, "{seconds} seconds");
        }
    }

    #[test]
    fn format_clock_refuses_values_outside_the_range() {
        for seconds in [f64::NAN, -1.0, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(format_clock(seconds, false), "0:00", "{seconds}");
            assert_eq!(format_clock(seconds, true), "0:00.0", "{seconds}");
        }
        assert!(!format_clock(1e300, true).is_empty());
    }

    #[test]
    fn file_name_takes_the_part_after_the_last_separator() {
        assert_eq!(file_name("/home/user/recordings/demo.json"), "demo.json");
        assert_eq!(
            file_name(r"C:\Users\test\recordings\demo.json"),
            "demo.json"
        );
        assert_eq!(file_name("demo.json"), "demo.json");
        assert_eq!(file_name("/tmp/日本語🎉.json"), "日本語🎉.json");
        assert_eq!(file_name("/tmp/dir/"), "");
    }

    #[test]
    fn format_recorded_in_writes_the_date_in_the_zone() {
        assert_eq!(
            format_recorded_in(1_700_000_000.0, &chrono::Utc),
            "2023-11-14 22:13:20"
        );
    }

    #[test]
    fn format_recorded_in_writes_unknown_for_a_bad_timestamp() {
        for timestamp in [f64::NAN, f64::INFINITY, 1e300, -1e300] {
            assert_eq!(
                format_recorded_in(timestamp, &chrono::Utc),
                "unknown",
                "{timestamp}"
            );
        }
    }
}
