//! The text that a person reads: the header lines, the table of rows, and the
//! small formatters that both of them use.
//!
//! Every function here gives text back. None of them prints. Thus a test reads
//! exactly what a person sees, and the caller decides where the text goes.
//!
//! The text carries no color and no attribute of a terminal. A number that a
//! reader must compare with another number is the whole product of this tool,
//! so each formatter states one rule and has its own tests.

/// The text in place of a value that `faulte` could not read.
///
/// One text for every absent value, the same as `occ`. A reader then learns
/// the mark once.
pub const ABSENT: &str = "—";

/// What stands between two groups of three digits.
const GROUP_SEPARATOR: char = ',';

/// Writes the digits of `digits` in groups of three, from the right.
///
/// The input is the text of a number, and not a number, because a rate is a
/// `f64`. A cast from `f64` to an integer truncates a value that does not fit,
/// and `format!` with no decimal gives the digits of any value.
fn separate(digits: &str) -> String {
    let (sign, digits) = match digits.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", digits),
    };
    let places = digits.chars().count();
    let mut grouped = String::with_capacity(sign.len() + places + places / 3);
    grouped.push_str(sign);
    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (places - place) % 3 == 0 {
            grouped.push(GROUP_SEPARATOR);
        }
        grouped.push(digit);
    }
    grouped
}

/// Gives `value` with a separator between each group of three digits, for
/// example `959,815`.
///
/// A count of page faults has six or seven digits on a Mac that is short of
/// memory. The groups let a reader compare two such counts at a glance.
#[must_use]
pub fn count(value: u64) -> String {
    separate(&value.to_string())
}

/// The rate below which a rate carries one decimal.
const SMALLEST_WHOLE_RATE: f64 = 10.0;

/// Gives `per_second` as a rate of page faults, for example `0.4` or
/// `239,953`.
///
/// A rate below ten carries one decimal, because the whole part of such a rate
/// says almost nothing. A rate of ten or more is a whole number with a
/// separator between each group of three digits, because the decimal of a
/// large rate says nothing at all.
///
/// A rate that is not a number gives [`ABSENT`]. The window of the ranking can
/// be no time, and a division by no time gives no number.
#[must_use]
pub fn rate(per_second: f64) -> String {
    if !per_second.is_finite() {
        return ABSENT.to_owned();
    }
    if per_second < SMALLEST_WHOLE_RATE {
        return format!("{per_second:.1}");
    }
    separate(&format!("{per_second:.0}"))
}

/// The text of a share of nothing.
const NO_SHARE: &str = "0%";

/// The text of a share of everything.
const WHOLE_SHARE: &str = "100%";

/// The text of a share that is above nothing and below the smallest share
/// that one decimal shows.
const BELOW_SMALLEST_SHARE: &str = "<0.1%";

/// The text of a share that is below everything and above the largest share
/// that one decimal shows.
const ABOVE_LARGEST_SHARE: &str = ">99.9%";

/// One decimal of a share of nothing.
const ROUNDED_NOTHING: &str = "0.0";

/// One decimal of a share of everything.
const ROUNDED_EVERYTHING: &str = "100.0";

/// Gives `fraction`, a value from 0 to 1, as a share in percent, for example
/// `5.2%`.
///
/// A share that is not zero and that rounds to zero gives `<0.1%`, and a share
/// that is not everything and that rounds to everything gives `>99.9%`. On
/// 2026-09-18, 213 sessions made 90.2% of all faults, and each one of them
/// made a share that one decimal shows as zero. A row that says `0.0%` about a
/// process which made 23,000 faults hides the cause of the load.
///
/// A share that is not a number gives [`ABSENT`].
#[must_use]
pub fn share(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A count carries a separator between each group of three digits, from
    /// the right. A count below a thousand carries none.
    #[test]
    fn a_count_gets_a_separator_between_each_group_of_three_digits() {
        let cases = [
            (0, "0"),
            (1, "1"),
            (999, "999"),
            (1_000, "1,000"),
            (1_549, "1,549"),
            (12_345, "12,345"),
            (959_815, "959,815"),
            (1_000_000, "1,000,000"),
            (u64::MAX, "18,446,744,073,709,551,615"),
        ];

        for (value, text) in cases {
            assert_eq!(count(value), text, "the count {value}");
        }
    }

    /// A rate below ten carries one decimal. A rate of ten or more is a whole
    /// number in groups of three digits. A rate that is not a number gives the
    /// text of an absent value.
    #[test]
    fn a_rate_below_ten_carries_one_decimal_and_a_larger_rate_is_whole() {
        let cases = [
            (0.0, "0.0"),
            (0.4, "0.4"),
            (0.75, "0.8"),
            (9.7, "9.7"),
            (10.0, "10"),
            (10.4, "10"),
            (999.0, "999"),
            (1_000.0, "1,000"),
            (239_952.6, "239,953"),
            (f64::INFINITY, ABSENT),
            (f64::NAN, ABSENT),
        ];

        for (value, text) in cases {
            assert_eq!(rate(value), text, "the rate {value}");
        }
    }

    /// A share carries one decimal. Nothing and everything carry none. A share
    /// that is above nothing and rounds to nothing says so, and a share that
    /// is below everything and rounds to everything says so.
    #[test]
    fn a_share_that_is_not_zero_never_prints_as_zero() {
        let cases = [
            (0.0, "0%"),
            (1.0, "100%"),
            (0.052, "5.2%"),
            (0.902, "90.2%"),
            (0.004, "0.4%"),
            (0.001, "0.1%"),
            (0.0004, "<0.1%"),
            (0.000_001, "<0.1%"),
            (0.9999, ">99.9%"),
            (f64::NAN, ABSENT),
            (f64::INFINITY, ABSENT),
        ];

        for (fraction, text) in cases {
            assert_eq!(share(fraction), text, "the share {fraction}");
        }
    }
}
