//! The screen of cctop: the table of sessions, the change log, and the
//! status line.

/// Format a time span for the screen. A span under one minute shows seconds,
/// under one hour shows minutes and seconds, under one day shows hours and
/// minutes, and a longer span shows days and hours.
#[must_use]
pub fn format_elapsed(ms: u64) -> String {
    format!("{ms}")
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
}
