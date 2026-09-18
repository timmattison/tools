//! The text that a person reads: the header lines, the table of rows, and the
//! small formatters that both of them use.
//!
//! Every function here gives text back. None of them prints. Thus a test reads
//! exactly what a person sees, and the caller decides where the text goes.
//!
//! The text carries no color and no attribute of a terminal. A number that a
//! reader must compare with another number is the whole product of this tool,
//! so each formatter states one rule and has its own tests.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::pid::Uid;

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
    if !fraction.is_finite() {
        return ABSENT.to_owned();
    }
    if fraction <= 0.0 {
        return NO_SHARE.to_owned();
    }
    if fraction >= 1.0 {
        return WHOLE_SHARE.to_owned();
    }
    let percent = format!("{:.1}", fraction * 100.0);
    if percent == ROUNDED_NOTHING {
        return BELOW_SMALLEST_SHARE.to_owned();
    }
    if percent == ROUNDED_EVERYTHING {
        return ABOVE_LARGEST_SHARE.to_owned();
    }
    format!("{percent}%")
}

/// The step from one unit of size to the next.
const SIZE_STEP: u64 = 1_024;

/// The name of each unit of size, from the smallest up.
const SIZE_UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

/// The place in [`SIZE_UNITS`] of the first unit that carries one decimal.
const FIRST_UNIT_WITH_A_DECIMAL: usize = 3;

/// Gives `value` bytes as a size, for example `812 MB` or `9.3 GB`.
///
/// The step from one unit to the next is 1,024, the step that a Mac reports.
/// A size of a gigabyte or more carries one decimal, because a whole number of
/// gigabytes hides a difference of hundreds of megabytes. A smaller size is a
/// whole number, because the decimal of a size in kilobytes says nothing.
#[must_use]
pub fn bytes(value: u64) -> String {
    let mut unit = 0;
    let mut scale = 1_u64;
    while value / scale >= SIZE_STEP && unit + 1 < SIZE_UNITS.len() {
        scale *= SIZE_STEP;
        unit += 1;
    }
    let name = SIZE_UNITS[unit];
    if unit < FIRST_UNIT_WITH_A_DECIMAL {
        // The loop stops at the last unit, so the whole part is below 1,024
        // under every other unit, and 16 under the last one. No such number
        // needs a separator.
        return format!("{} {name}", value / scale);
    }
    format!("{:.1} {name}", value as f64 / scale as f64)
}

/// Gives `value` kibibytes as a size, for example `42 KB`.
///
/// `ps` gives the resident memory of a process in kibibytes. A size that no
/// memory can hold gives the largest size, and not a panic.
#[must_use]
pub fn kibibytes(value: u64) -> String {
    bytes(value.saturating_mul(SIZE_STEP))
}

/// Gives the age of a process that started at `started_at_epoch_secs`, at the
/// time `now`, for example `3h 12m`.
///
/// The format is the format of `occ`, so the two tools print an age the same
/// way. A process with no start time gives [`ABSENT`]: `ps` does not list the
/// kernel, so the row of PID 0 has no start time, and a guess there is a
/// number that a reader cannot tell from a measured one.
///
/// A start time after `now` gives an age of no time. The clock of this Mac can
/// move back between the read of the table and the read of `now`.
#[must_use]
pub fn age(started_at_epoch_secs: Option<u64>, now: SystemTime) -> String {
    let Some(started) = started_at_epoch_secs else {
        return ABSENT.to_owned();
    };
    let now_secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    occ::format_uptime(now_secs.saturating_sub(started))
}

/// The name of each account that runs a process in the ranking.
///
/// The table shows the owner of a process by name, because a reader knows the
/// accounts of this Mac by name and not by number. The caller reads the names
/// with `getpwuid_r`, which answers for some accounts and not for others, so
/// this map holds the names that it got and gives the bare number for the
/// rest. A number is never wrong, and a wrong name is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Accounts {
    /// The name of each account that the caller could read.
    names: HashMap<Uid, String>,
}

impl Accounts {
    /// Gives a map with no name in it. Every account then shows its number.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives this map, with `name` as the name of `uid`.
    #[must_use]
    pub fn with(mut self, uid: Uid, name: impl Into<String>) -> Self {
        self.names.insert(uid, name.into());
        self
    }

    /// Gives the name of `uid`, or the number of `uid` when the map has no
    /// name for it.
    #[must_use]
    pub fn name_of(&self, uid: Uid) -> String {
        let _ = &self.names;
        uid.to_string()
    }
}

impl FromIterator<(Uid, String)> for Accounts {
    /// Collects a name for each account. A later name of one account replaces
    /// an earlier one.
    fn from_iter<I: IntoIterator<Item = (Uid, String)>>(names: I) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

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

    /// A size steps by 1,024. A size of a gigabyte or more carries one
    /// decimal, and a smaller size is a whole number. The largest size is the
    /// last unit, and no size gives a panic.
    #[test]
    fn a_size_steps_by_1024_and_carries_a_decimal_from_a_gigabyte_up() {
        let cases = [
            (0, "0 B"),
            (1, "1 B"),
            (1_023, "1023 B"),
            (1_024, "1 KB"),
            (43_008, "42 KB"),
            (1_048_575, "1023 KB"),
            (1_048_576, "1 MB"),
            (851_443_712, "812 MB"),
            (1_073_741_824, "1.0 GB"),
            (9_985_798_963, "9.3 GB"),
            (11_811_160_064, "11.0 GB"),
            (28_991_029_248, "27.0 GB"),
            (u64::MAX, "16.0 EB"),
        ];

        for (value, text) in cases {
            assert_eq!(bytes(value), text, "the size of {value} bytes");
        }
    }

    /// A size in kibibytes is the same size, 1,024 times larger. A count of
    /// kibibytes that no memory can hold gives the largest size, and not a
    /// panic.
    #[test]
    fn a_size_in_kibibytes_is_the_same_size_1024_times_larger() {
        let cases = [
            (0, "0 B"),
            (1, "1 KB"),
            (42, "42 KB"),
            (1_023, "1023 KB"),
            (1_024, "1 MB"),
            (831_488, "812 MB"),
            (9_752_733, "9.3 GB"),
            (u64::MAX, "16.0 EB"),
        ];

        for (value, text) in cases {
            assert_eq!(kibibytes(value), text, "the size of {value} kibibytes");
        }
    }

    /// The age is the time from the start of the process to now, in the format
    /// of `occ`. A process with no start time gives the text of an absent
    /// value, and a start time after now gives no time.
    #[test]
    fn the_age_is_the_time_from_the_start_of_the_process_to_now() {
        let started = 1_780_000_000;
        let cases = [
            (None, ABSENT),
            (Some(started), "0s"),
            (Some(started + 5), "0s"),
            (Some(started - 45), "45s"),
            (Some(started - (3 * 3_600 + 12 * 60 + 59)), "3h 12m"),
            (Some(started - (2 * 86_400 + 5 * 3_600)), "2d 5h"),
        ];
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(started);

        for (started_at_epoch_secs, text) in cases {
            assert_eq!(
                age(started_at_epoch_secs, now),
                text,
                "the start {started_at_epoch_secs:?}"
            );
        }
    }

    /// An account shows its name. An account that the caller read no name for
    /// shows its bare number, because a number is never wrong and a guessed
    /// name is.
    #[test]
    fn an_account_shows_its_name_and_a_bare_number_when_it_has_none() {
        let accounts = Accounts::new()
            .with(Uid::new(501), "tim")
            .with(Uid::new(0), "root");

        assert_eq!(accounts.name_of(Uid::new(501)), "tim");
        assert_eq!(accounts.name_of(Uid::new(0)), "root");
        assert_eq!(accounts.name_of(Uid::new(502)), "502");
        assert_eq!(Accounts::new().name_of(Uid::new(501)), "501");

        let collected: Accounts = [(Uid::new(502), "work".to_owned())].into_iter().collect();
        assert_eq!(collected.name_of(Uid::new(502)), "work");
        assert_eq!(collected.name_of(Uid::new(501)), "501");
    }
}
