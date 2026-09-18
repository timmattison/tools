//! A length of time on the command line: `5s`, `10m`, `2h`, `7d`, or a bare
//! `5`, which is 5 seconds.
//!
//! Each flag of `faulte` that takes a time takes a [`Span`]. No such flag means
//! anything at zero: a sample of no time measures nothing, and a session idle
//! for no time is a session in use. Thus the parser refuses zero for every
//! flag, and a caller never checks for it.
//!
//! A [`Span`] is a whole number of seconds. Thus [`Span`] prints back exactly,
//! and `faulte` can print a command line that gives the same values again.

use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;
use std::time::Duration;

/// A length of time that a person gave on the command line: a whole number of
/// seconds, 1 or more.
///
/// Parse it with [`str::parse`]. Print it with [`fmt::Display`], which gives the
/// shortest text that parses back to the same value. Convert it to a
/// [`Duration`] with [`From`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span(NonZeroU64);

/// What each refusal tells the person to give in place of the bad text.
const HINT: &str = "give a whole number and a unit, for example 5s, 10m, 2h, or 7d";

/// The reason why a text is not a [`Span`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseSpanError {
    /// The text is empty.
    #[error("the duration is empty: {HINT}")]
    Empty,
    /// The text gives zero seconds. No flag of `faulte` means anything at
    /// zero.
    #[error("{text:?} is zero: give a duration of 1 second or more")]
    Zero {
        /// The text as the person gave it.
        text: String,
    },
    /// The text is not a duration.
    #[error("{text:?} is not a duration")]
    Invalid {
        /// The text as the person gave it.
        text: String,
    },
}

/// The seconds in one day.
const SECONDS_PER_DAY: u64 = 86_400;

/// The seconds in one hour.
const SECONDS_PER_HOUR: u64 = 3_600;

/// The seconds in one minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// Each unit that the parser accepts, and its length in seconds, largest
/// first.
const UNITS: [(&str, u64); 4] = [
    ("d", SECONDS_PER_DAY),
    ("h", SECONDS_PER_HOUR),
    ("m", SECONDS_PER_MINUTE),
    ("s", 1),
];

/// Gives the length in seconds of `unit`. A text with no unit is a number of
/// seconds.
fn seconds_per(unit: &str) -> Option<u64> {
    if unit.is_empty() {
        return Some(1);
    }
    UNITS
        .iter()
        .find(|(name, _)| *name == unit)
        .map(|(_, seconds)| *seconds)
}

impl FromStr for Span {
    type Err = ParseSpanError;

    /// Parses a whole number, then an optional unit: `s`, `m`, `h`, or `d`.
    ///
    /// The number is the text up to the last ASCII digit, and the unit is the
    /// rest. Thus a sign, a decimal point, or a letter inside the number makes
    /// the number fail, and does not become part of a unit.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() {
            return Err(ParseSpanError::Empty);
        }
        let invalid = || ParseSpanError::Invalid {
            text: text.to_owned(),
        };
        let number = text.trim_end_matches(|character: char| !character.is_ascii_digit());
        let unit = text.strip_prefix(number).unwrap_or_default();
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let per_unit = seconds_per(unit).ok_or_else(invalid)?;
        let count: u64 = number.parse().map_err(|_| invalid())?;
        let seconds = count.checked_mul(per_unit).ok_or_else(invalid)?;
        NonZeroU64::new(seconds).map(Self).ok_or_else(invalid)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}s", self.0)
    }
}

impl From<Span> for Duration {
    fn from(span: Span) -> Self {
        Self::from_secs(span.0.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` and gives the number of seconds, or the error.
    fn seconds(text: &str) -> Result<u64, ParseSpanError> {
        text.parse::<Span>()
            .map(|span| Duration::from(span).as_secs())
    }

    #[test]
    fn a_whole_number_with_a_unit_parses_to_its_seconds() {
        let cases = [
            ("5s", 5),
            ("10m", 600),
            ("2h", 7_200),
            ("7d", 604_800),
            ("5", 5),
            ("90", 90),
            ("05m", 300),
        ];
        for (text, expected) in cases {
            assert_eq!(seconds(text), Ok(expected), "the text {text:?}");
        }
    }

    #[test]
    fn an_empty_text_is_refused_as_empty() {
        let error = seconds("").expect_err("an empty text is not a duration");

        assert_eq!(error, ParseSpanError::Empty);
        assert!(
            error.to_string().starts_with("the duration is empty: "),
            "the message says that the text is empty: {error}"
        );
    }

    #[test]
    fn zero_is_refused_in_every_unit() {
        for text in ["0", "0s", "0m", "0h", "0d", "000m"] {
            let error = seconds(text).expect_err("zero is not a duration");

            assert_eq!(
                error,
                ParseSpanError::Zero {
                    text: text.to_owned()
                },
                "the text {text:?}"
            );
            assert!(
                error.to_string().contains(&format!("{text:?}")),
                "the message names the text {text:?}: {error}"
            );
        }
    }
}
