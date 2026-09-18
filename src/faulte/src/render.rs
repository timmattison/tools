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
    value.to_string()
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
}
