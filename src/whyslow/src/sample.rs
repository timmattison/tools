//! One sample of the process table.
//!
//! `whyslow` reads the table of `/bin/ps` twice. The CPU time that a process
//! used between the two reads is the second CPU time minus the first. This
//! module holds the command line of `ps`, the parser of its output, the type
//! of a CPU time, and the subtraction of two tables.

use std::time::Duration;

use faulte::table::parse_ps_number;

/// The character between the minutes and the seconds of a time token.
const MINUTES_SEPARATOR: char = ':';

/// The character between the seconds and the hundredths of a time token.
const HUNDREDTHS_SEPARATOR: char = '.';

/// The count of digits of the seconds and of the hundredths.
const DIGITS_OF_A_FIELD: usize = 2;

/// The seconds in one minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// The hundredths of a second in one second.
const CENTISECONDS_PER_SECOND: u64 = 100;

/// The milliseconds in one hundredth of a second.
const MILLIS_PER_CENTISECOND: u128 = 10;

/// CPU time in hundredths of a second, as `ps -o time` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CpuTime(u64);

impl CpuTime {
    /// Makes a CPU time from a count of hundredths of a second.
    #[must_use]
    pub const fn from_centiseconds(centiseconds: u64) -> Self {
        Self(centiseconds)
    }

    /// Gives this CPU time as a count of hundredths of a second.
    #[must_use]
    pub const fn centiseconds(self) -> u64 {
        self.0
    }

    /// Reads a CPU time from a token of `ps -o time`.
    ///
    /// The layout is `M:SS.cc`. The minutes are ASCII digits and have no
    /// upper bound. The seconds are exactly two digits below 60. The
    /// hundredths are exactly two digits. Any other text gives `None`, so a
    /// layout that the parser does not know never gives a wrong time.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        let (minutes, rest) = token.split_once(MINUTES_SEPARATOR)?;
        let (seconds, hundredths) = rest.split_once(HUNDREDTHS_SEPARATOR)?;
        let minutes: u64 = parse_ps_number(minutes)?;
        let seconds: u64 = fixed_digits(seconds)?;
        let hundredths: u64 = fixed_digits(hundredths)?;
        if seconds >= SECONDS_PER_MINUTE {
            return None;
        }
        minutes
            .checked_mul(SECONDS_PER_MINUTE)?
            .checked_add(seconds)?
            .checked_mul(CENTISECONDS_PER_SECOND)?
            .checked_add(hundredths)
            .map(Self)
    }

    /// Takes `other` from this CPU time. The result stops at zero.
    #[must_use]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    /// Gives this CPU time as a percent of one processor over `window`.
    ///
    /// The result is `(cs * 100 + w / 2) / w`, where `w` is the window in
    /// hundredths of a second. This rounds half up. A window of zero, or a
    /// window shorter than one hundredth of a second, gives 0.
    #[must_use]
    pub fn percent_of(self, window: Duration) -> u64 {
        let window = u64::try_from(window.as_millis() / MILLIS_PER_CENTISECOND).unwrap_or(u64::MAX);
        if window == 0 {
            return 0;
        }
        let scaled = u128::from(self.0) * 100 + u128::from(window / 2);
        u64::try_from(scaled / u128::from(window)).unwrap_or(u64::MAX)
    }
}

/// Reads exactly two ASCII digits.
fn fixed_digits(token: &str) -> Option<u64> {
    if token.len() == DIGITS_OF_A_FIELD {
        parse_ps_number(token)
    } else {
        None
    }
}

impl std::ops::Add for CpuTime {
    type Output = Self;

    /// Adds two times. The sum stops at the largest value.
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl std::iter::Sum for CpuTime {
    fn sum<I: Iterator<Item = Self>>(times: I) -> Self {
        times.fold(Self::default(), |total, time| total + time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token of `M:SS.cc` gives hundredths of a second. The minutes are
    /// not bounded: a process that ran for 6 days shows `9086:33.94`.
    #[test]
    fn a_time_token_gives_hundredths_of_a_second() {
        for (token, centiseconds) in [
            ("0:00.05", 5),
            ("1:15.84", 7_584),
            ("9086:33.94", 54_519_394),
            ("0:00.00", 0),
            ("0:59.99", 5_999),
        ] {
            assert_eq!(
                CpuTime::parse(token),
                Some(CpuTime::from_centiseconds(centiseconds)),
                "token {token:?}"
            );
        }
    }

    /// A token that is not exactly `M:SS.cc` is refused. A wrong guess of
    /// the layout would give a wrong CPU time that looks right.
    #[test]
    fn a_token_that_is_not_a_time_is_refused() {
        for token in [
            "1:60.00",
            "1:5.00",
            "1:05.5",
            "1:05",
            ":05.00",
            "a:05.00",
            "-1:05.00",
            "",
            "1:05.000",
            "1:05.0a",
            "+1:05.00",
            "1:+5.00",
            "日:05.00",
            "1:05.00 ",
        ] {
            assert_eq!(CpuTime::parse(token), None, "token {token:?}");
        }
    }

    /// A time as a percent of one processor over a window. The result is
    /// rounded half up.
    #[test]
    fn a_time_gives_a_percent_of_one_processor() {
        let one_second = Duration::from_secs(1);
        let three_seconds = Duration::from_secs(3);
        assert_eq!(CpuTime::from_centiseconds(290).percent_of(one_second), 290);
        assert_eq!(CpuTime::from_centiseconds(1).percent_of(three_seconds), 0);
        assert_eq!(CpuTime::from_centiseconds(2).percent_of(three_seconds), 1);
    }

    /// A window of zero gives zero, and does not divide by zero. A window
    /// shorter than one hundredth of a second counts as zero too.
    #[test]
    fn a_zero_window_gives_zero_percent() {
        let time = CpuTime::from_centiseconds(500);
        assert_eq!(time.percent_of(Duration::ZERO), 0);
        assert_eq!(time.percent_of(Duration::from_millis(9)), 0);
    }

    /// The sum of times adds them, and the subtraction stops at zero.
    #[test]
    fn times_add_sum_and_subtract() {
        let a = CpuTime::from_centiseconds(30);
        let b = CpuTime::from_centiseconds(12);
        assert_eq!(a + b, CpuTime::from_centiseconds(42));
        assert_eq!(
            [a, b, a].into_iter().sum::<CpuTime>(),
            CpuTime::from_centiseconds(72)
        );
        assert_eq!(a.saturating_sub(b), CpuTime::from_centiseconds(18));
        assert_eq!(b.saturating_sub(a), CpuTime::from_centiseconds(0));
        assert_eq!(CpuTime::default().centiseconds(), 0);
    }
}
