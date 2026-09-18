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

/// The units that the parser accepts, as a refusal names them.
const UNIT_NAMES: &str = "s, m, h, or d";

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
    /// A whole number comes first, and the rest is not a unit that the
    /// parser accepts.
    #[error("{text:?} has the unknown unit {unit:?}: use {UNIT_NAMES}")]
    UnknownUnit {
        /// The text as the person gave it.
        text: String,
        /// The part of the text after the number.
        unit: String,
    },
    /// The text does not start with a whole number of ASCII digits. A sign,
    /// a decimal point, and an exponent are refused here.
    #[error("{text:?} is not a whole number with a unit: {HINT}")]
    NotAWholeNumber {
        /// The text as the person gave it.
        text: String,
    },
    /// The number of seconds does not fit in 64 bits.
    #[error("{text:?} is too long: the number of seconds does not fit in 64 bits")]
    TooLong {
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
        let too_long = || ParseSpanError::TooLong {
            text: text.to_owned(),
        };
        let number = text.trim_end_matches(|character: char| !character.is_ascii_digit());
        let unit = text.strip_prefix(number).unwrap_or_default();
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ParseSpanError::NotAWholeNumber {
                text: text.to_owned(),
            });
        }
        let per_unit = seconds_per(unit).ok_or_else(|| ParseSpanError::UnknownUnit {
            text: text.to_owned(),
            unit: unit.to_owned(),
        })?;
        // The number holds ASCII digits only, so the parse fails only when the
        // value does not fit in 64 bits.
        let count: u64 = number.parse().map_err(|_| too_long())?;
        let seconds = count.checked_mul(per_unit).ok_or_else(too_long)?;
        NonZeroU64::new(seconds)
            .map(Self)
            .ok_or_else(|| ParseSpanError::Zero {
                text: text.to_owned(),
            })
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

    #[test]
    fn an_unknown_unit_is_refused_and_named() {
        let cases = [
            ("5x", "x"),
            ("5ms", "ms"),
            ("5S", "S"),
            ("5M", "M"),
            ("5 s", " s"),
            ("5sec", "sec"),
            ("10mm", "mm"),
            ("5日", "日"),
            ("5🎉", "🎉"),
            ("5é", "é"),
        ];
        for (text, unit) in cases {
            let error = seconds(text).expect_err("an unknown unit is not a duration");

            assert_eq!(
                error,
                ParseSpanError::UnknownUnit {
                    text: text.to_owned(),
                    unit: unit.to_owned(),
                },
                "the text {text:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&format!("{text:?}")) && message.contains(&format!("{unit:?}")),
                "the message names the text {text:?} and the unit {unit:?}: {message}"
            );
        }
    }

    #[test]
    fn a_text_that_is_not_a_whole_number_is_refused_and_named() {
        let cases = [
            "-5s", "-5", "-0s", "+5s", "1.5h", "0.5", "1e3", "5s5", " 5s", "s", "m", "日本語",
            "🎉s", "café", "五s", "٥s", "5日5",
        ];
        for text in cases {
            let error = seconds(text).expect_err("the text is not a whole number");

            assert_eq!(
                error,
                ParseSpanError::NotAWholeNumber {
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

    #[test]
    fn seconds_that_do_not_fit_in_64_bits_are_refused_and_named() {
        let largest = [
            ("18446744073709551615", u64::MAX),
            ("18446744073709551615s", u64::MAX),
            ("307445734561825860m", 18_446_744_073_709_551_600),
            ("5124095576030431h", 18_446_744_073_709_551_600),
            ("213503982334601d", 18_446_744_073_709_526_400),
        ];
        for (text, expected) in largest {
            assert_eq!(seconds(text), Ok(expected), "the text {text:?}");
        }

        let too_long = [
            "18446744073709551616",
            "18446744073709551616s",
            "307445734561825861m",
            "5124095576030432h",
            "213503982334602d",
            "99999999999999999999999999999999d",
        ];
        for text in too_long {
            let error = seconds(text).expect_err("the seconds do not fit in 64 bits");

            assert_eq!(
                error,
                ParseSpanError::TooLong {
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

    #[test]
    fn a_span_prints_in_the_largest_unit_that_divides_it_exactly() {
        let cases = [
            ("604800", "7d"),
            ("600", "10m"),
            ("90", "90s"),
            ("5", "5s"),
            ("3600", "1h"),
            ("5400", "90m"),
            ("86400", "1d"),
            ("90000", "25h"),
            ("86401", "86401s"),
            ("120s", "2m"),
            ("48h", "2d"),
            ("18446744073709551615", "18446744073709551615s"),
            ("213503982334601d", "213503982334601d"),
        ];
        for (text, expected) in cases {
            let span: Span = text.parse().expect("the text is a duration");
            let printed = span.to_string();

            assert_eq!(printed, expected, "the text {text:?}");
            assert_eq!(
                printed.parse::<Span>(),
                Ok(span),
                "the printed text {printed:?} parses back to the same span"
            );
        }
    }
}
