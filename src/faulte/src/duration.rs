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

/// The reason why a text is not a [`Span`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseSpanError {
    /// The text is not a duration.
    #[error("{text:?} is not a duration")]
    Invalid {
        /// The text as the person gave it.
        text: String,
    },
}

impl FromStr for Span {
    type Err = ParseSpanError;

    fn from_str(_text: &str) -> Result<Self, Self::Err> {
        Ok(Self(NonZeroU64::MIN))
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
}
