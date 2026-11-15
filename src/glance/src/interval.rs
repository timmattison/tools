//! The time between two samples.
//!
//! A person gives the interval in seconds on the command line. This module
//! turns that text into a duration and refuses a value that the tool cannot
//! use.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// The default interval, in seconds.
const DEFAULT_SECONDS: u64 = 1;

/// The time between two samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval(Duration);

impl Interval {
    /// The default interval of 1 second.
    pub const DEFAULT: Interval = Interval(Duration::from_secs(DEFAULT_SECONDS));

    /// The smallest interval, in seconds. A shorter interval costs more CPU time than it shows.
    pub const MINIMUM_SECONDS: f64 = 0.25;

    /// The interval as a duration.
    #[must_use]
    pub fn duration(self) -> Duration {
        self.0
    }

    /// The interval in seconds.
    #[must_use]
    pub fn seconds(self) -> f64 {
        self.0.as_secs_f64()
    }
}

/// The reason that a text is not an interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntervalError {
    /// The text is not a decimal number.
    NotANumber {
        /// The text that the person gave.
        text: String,
    },
    /// The number is NaN or infinite.
    NotFinite {
        /// The text that the person gave.
        text: String,
    },
    /// The number is below `Interval::MINIMUM_SECONDS`.
    BelowMinimum {
        /// The text that the person gave.
        text: String,
    },
    /// The number is too large for a duration.
    TooLarge {
        /// The text that the person gave.
        text: String,
    },
}

impl fmt::Display for IntervalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntervalError::NotANumber { text } => {
                write!(
                    f,
                    "the interval must be a number of seconds, and {text:?} is not"
                )
            }
            IntervalError::NotFinite { text } => {
                write!(f, "the interval must be a finite number, and {text} is not")
            }
            IntervalError::BelowMinimum { text } => write!(
                f,
                "the interval must be at least {} seconds, and {text} is less",
                Interval::MINIMUM_SECONDS
            ),
            IntervalError::TooLarge { text } => {
                write!(
                    f,
                    "the interval is too large, and {text} seconds do not fit"
                )
            }
        }
    }
}

impl std::error::Error for IntervalError {}

impl FromStr for Interval {
    type Err = IntervalError;

    /// Reads the interval from a decimal number of seconds.
    ///
    /// # Errors
    ///
    /// Returns an error if the text is not a number, is not finite, is below
    /// `Interval::MINIMUM_SECONDS`, or is too large for a duration.
    fn from_str(text: &str) -> Result<Interval, IntervalError> {
        let trimmed = text.trim();
        let owned = || trimmed.to_owned();
        let seconds: f64 = trimmed
            .parse()
            .map_err(|_| IntervalError::NotANumber { text: owned() })?;
        if !seconds.is_finite() {
            return Err(IntervalError::NotFinite { text: owned() });
        }
        if seconds < Interval::MINIMUM_SECONDS {
            return Err(IntervalError::BelowMinimum { text: owned() });
        }
        Duration::try_from_secs_f64(seconds)
            .map(Interval)
            .map_err(|_| IntervalError::TooLarge { text: owned() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Interval, IntervalError> {
        text.parse::<Interval>()
    }

    #[test]
    fn one_is_one_second() {
        assert_eq!(parse("1").unwrap().duration(), Duration::from_secs(1));
    }

    #[test]
    fn the_minimum_is_accepted() {
        assert_eq!(
            parse("0.25").unwrap().duration(),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn a_decimal_number_is_accepted() {
        let interval = parse("2.5").unwrap();
        assert_eq!(interval.duration(), Duration::from_millis(2500));
        assert!((interval.seconds() - 2.5).abs() < 1e-12);
    }

    #[test]
    fn a_number_below_the_minimum_is_refused() {
        for text in ["0.1", "0", "-1"] {
            assert!(
                matches!(parse(text), Err(IntervalError::BelowMinimum { .. })),
                "{text}"
            );
        }
    }

    #[test]
    fn a_text_that_is_not_a_number_is_refused() {
        for text in ["abc", ""] {
            assert!(
                matches!(parse(text), Err(IntervalError::NotANumber { .. })),
                "{text:?}"
            );
        }
    }

    #[test]
    fn nan_and_infinity_are_refused() {
        for text in ["NaN", "inf"] {
            assert!(
                matches!(parse(text), Err(IntervalError::NotFinite { .. })),
                "{text}"
            );
        }
    }

    #[test]
    fn a_number_too_large_for_a_duration_is_refused() {
        assert!(matches!(
            parse("1e300"),
            Err(IntervalError::TooLarge { .. })
        ));
    }

    #[test]
    fn the_default_is_one_second() {
        assert_eq!(Interval::DEFAULT.duration(), Duration::from_secs(1));
    }

    #[test]
    fn the_message_of_a_refusal_names_the_text_and_the_minimum() {
        let message = parse("0.1").unwrap_err().to_string();
        assert!(message.contains("0.25"), "{message}");
        assert!(message.contains("0.1"), "{message}");
    }
}
