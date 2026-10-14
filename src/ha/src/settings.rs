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
    Err(SettingsError::InvalidDuration {
        input: input.trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_reads_each_unit() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("15m").unwrap(), Duration::from_secs(900));
        assert_eq!(parse_duration("8h").unwrap(), Duration::from_secs(28_800));
        assert_eq!(parse_duration("2d").unwrap(), Duration::from_secs(172_800));
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(
            parse_duration("  8h\t").unwrap(),
            Duration::from_secs(28_800)
        );
    }
}
