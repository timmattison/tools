//! The screen of cctop: the table of sessions, the change log, and the
//! status line.

use chrono::TimeZone;

use crate::state::EpochMillis;

/// Format a time span for the screen. A span under one minute shows seconds,
/// under one hour shows minutes and seconds, under one day shows hours and
/// minutes, and a longer span shows days and hours.
#[must_use]
pub fn format_elapsed(ms: u64) -> String {
    let seconds = ms / 1000;
    let (minutes, hours, days) = (seconds / 60, seconds / 3600, seconds / 86_400);
    if seconds < 60 {
        format!("{seconds}s")
    } else if minutes < 60 {
        format!("{minutes}m {:02}s", seconds % 60)
    } else if hours < 24 {
        format!("{hours}h {:02}m", minutes % 60)
    } else {
        format!("{days}d {:02}h", hours % 24)
    }
}

/// Format a time of day as `HH:MM:SS` in the time zone `zone`.
#[must_use]
pub fn clock_time<Tz: TimeZone>(at: EpochMillis, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let millis = i64::try_from(at.as_u64()).unwrap_or(i64::MAX);
    zone.timestamp_millis_opt(millis)
        .single()
        .map_or_else(String::new, |time| time.format("%H:%M:%S").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = 1000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn format_elapsed_uses_the_unit_that_fits() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(59_999), "59s");
        assert_eq!(format_elapsed(MINUTE), "1m 00s");
        assert_eq!(format_elapsed(3_599_000), "59m 59s");
        assert_eq!(format_elapsed(HOUR), "1h 00m");
        assert_eq!(format_elapsed(86_399_000), "23h 59m");
        assert_eq!(format_elapsed(DAY), "1d 00h");
        assert_eq!(format_elapsed(2 * DAY + 4 * HOUR), "2d 04h");
    }

    #[test]
    fn clock_time_shows_the_time_of_day_in_the_zone() {
        let at = EpochMillis::new(1_794_824_265_058);
        let expected = chrono::DateTime::from_timestamp_millis(1_794_824_265_058)
            .expect("the instant is valid")
            .format("%H:%M:%S")
            .to_string();
        assert_eq!(clock_time(at, &chrono::Utc), expected);
        assert_eq!(expected.len(), 8);
    }
}
