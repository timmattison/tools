//! TOML configuration and CLI-override plumbing for `seescc`.
//!
//! This module owns the small, human-friendly value parsers that both the
//! config file and the command-line layer share. The first of these is
//! [`parse_duration`], which turns strings like `500ms`, `1s`, `15m`, and `1h`
//! into [`std::time::Duration`] values. Parse failures surface as
//! [`ConfigError`] so the CLI can report exactly which input was rejected.

use std::time::Duration;

/// Errors produced while interpreting `seescc` configuration values.
///
/// Designed to grow: later slices add variants for malformed TOML, unknown
/// keys, and out-of-range numeric settings. Each variant carries enough context
/// to name the offending input in a user-facing message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    /// A duration string was empty, missing its magnitude, used an unknown unit
    /// suffix, or had a non-integer / overflowing magnitude.
    #[error(
        "invalid duration {input:?}: expected an integer magnitude followed by one of \
         the unit suffixes `ms`, `s`, `m`, or `h` (e.g. `500ms`, `1s`, `15m`, `1h`)"
    )]
    InvalidDuration {
        /// The rejected input, echoed back so the user can spot the typo.
        input: String,
    },
}

/// Parse a human-friendly duration string into a [`Duration`].
///
/// Accepts an integer magnitude immediately followed by one of the unit
/// suffixes `ms`, `s`, `m`, or `h` (for example `500ms`, `1s`, `15m`, `1h`).
/// Surrounding whitespace is trimmed. The `ms` suffix is checked before `s`
/// because both end in `s`.
///
/// # Errors
/// Returns [`ConfigError::InvalidDuration`] when `s` is empty, omits the
/// magnitude (`"s"`), omits or uses an unknown unit suffix (`"10"`, `"10x"`),
/// has a non-integer magnitude (`"1.5s"`), or whose magnitude overflows the
/// internal `u64` second/millisecond arithmetic.
pub(crate) fn parse_duration(s: &str) -> Result<Duration, ConfigError> {
    let trimmed = s.trim();
    let invalid = || ConfigError::InvalidDuration {
        input: trimmed.to_string(),
    };

    // Match the longest suffix first so `ms` is not mistaken for `s`.
    let (digits, build): (&str, fn(u64) -> Option<Duration>) =
        if let Some(d) = trimmed.strip_suffix("ms") {
            (d, |n| Some(Duration::from_millis(n)))
        } else if let Some(d) = trimmed.strip_suffix('s') {
            (d, |n| Some(Duration::from_secs(n)))
        } else if let Some(d) = trimmed.strip_suffix('m') {
            (d, |n| n.checked_mul(60).map(Duration::from_secs))
        } else if let Some(d) = trimmed.strip_suffix('h') {
            (d, |n| n.checked_mul(3600).map(Duration::from_secs))
        } else {
            return Err(invalid());
        };

    let magnitude: u64 = digits.parse().map_err(|_| invalid())?;
    build(magnitude).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_unit_suffix() {
        assert_eq!(
            parse_duration("500ms").expect("500ms should parse"),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_duration("1s").expect("1s should parse"),
            Duration::from_secs(1)
        );
        assert_eq!(
            parse_duration("15m").expect("15m should parse"),
            Duration::from_secs(900)
        );
        assert_eq!(
            parse_duration("1h").expect("1h should parse"),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            parse_duration("  250ms  ").expect("padded input should parse"),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn rejects_missing_magnitude() {
        assert!(parse_duration("s").is_err());
    }

    #[test]
    fn rejects_missing_suffix() {
        assert!(parse_duration("10").is_err());
    }

    #[test]
    fn rejects_unknown_suffix() {
        assert!(parse_duration("10x").is_err());
    }

    #[test]
    fn rejects_non_integer_magnitude() {
        assert!(parse_duration("1.5s").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_duration("garbage").is_err());
    }

    #[test]
    fn rejects_overflowing_magnitude() {
        // u64::MAX hours overflows the n * 3600 multiplication.
        assert!(parse_duration("18446744073709551615h").is_err());
    }

    #[test]
    fn error_message_names_input_and_units() {
        let err = parse_duration("10x").expect_err("`10x` must be rejected");
        let message = err.to_string();
        assert!(message.contains("\"10x\""), "message was: {message}");
        assert!(message.contains("ms"), "message was: {message}");
        assert!(message.contains('h'), "message was: {message}");
    }
}
