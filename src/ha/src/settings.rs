//! The settings of `ha`: what to watch, how far back to look, and how often
//! to refresh.
//!
//! The settings come from two places: the command line and a TOML config
//! file. The command line wins. This module reads the file, merges the two
//! sources, and checks every value. Each error names the key, the value, and
//! the file, so a person can find the mistake at once.

use std::time::Duration;

/// The reasons why `ha` cannot build its settings. Each message names the
/// input that caused the error.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// A duration text is not a whole number above zero with a unit.
    #[error(
        "invalid duration \"{input}\": write a whole number above zero and a unit, \
         s, m, h, or d (for example 30s, 15m, 8h, or 2d)"
    )]
    InvalidDuration {
        /// The text as given, after the trim.
        input: String,
    },
}

/// Reads a duration such as `30s`, `15m`, `8h`, or `2d`.
///
/// The text is a whole number above zero and one unit: `s` for seconds, `m`
/// for minutes, `h` for hours, or `d` for days. Spaces around the text do not
/// count.
///
/// # Errors
///
/// Returns [`SettingsError::InvalidDuration`] when the text has another shape,
/// when the number is zero, or when the result does not fit in a duration.
pub fn parse_duration(input: &str) -> Result<Duration, SettingsError> {
    let text = input.trim();
    let invalid = || SettingsError::InvalidDuration {
        input: text.to_owned(),
    };
    let unit = text.chars().next_back().ok_or_else(invalid)?;
    let seconds_per_unit: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        _ => return Err(invalid()),
    };
    let digits = text.strip_suffix(unit).ok_or_else(invalid)?;
    let count: u64 = digits.parse().map_err(|_| invalid())?;
    let seconds = count.checked_mul(seconds_per_unit).ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_reads_each_unit() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("15m").unwrap(), Duration::from_mins(15));
        assert_eq!(parse_duration("8h").unwrap(), Duration::from_hours(8));
        assert_eq!(parse_duration("2d").unwrap(), Duration::from_hours(48));
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(parse_duration("  8h\t").unwrap(), Duration::from_hours(8));
    }

    #[test]
    fn parse_duration_rejects_a_bad_text_and_quotes_it() {
        let bad = [
            "0s",
            "0d",
            "8x",
            "h",
            "",
            "   ",
            "-1h",
            "+5s",
            "1.5h",
            "8 h",
            "99999999999999999999d",
            "18446744073709551615d",
            "日本語h",
        ];
        for input in bad {
            let error = parse_duration(input).expect_err(input).to_string();
            assert!(
                error.contains(&format!("\"{}\"", input.trim())),
                "{input:?}: the message does not quote the input: {error}"
            );
            assert!(
                error.contains("s, m, h, or d"),
                "{input:?}: the message does not name the units: {error}"
            );
        }
    }
}
