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
    let _ = s;
    todo!()
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
}
