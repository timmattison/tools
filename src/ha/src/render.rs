//! Draws the board of `ha` and the text formats of its columns.

use std::time::Duration;

/// Writes how long a run took, in at most 7 columns.
///
/// Examples are `45s`, `12m 03s`, `2h 14m`, and `3d 04h`. A duration of 100
/// days or more gives `>99d`.
#[must_use]
pub fn format_elapsed(d: Duration) -> String {
    let total = d.as_secs();
    let (minute, hour, day) = (60, 3_600, 86_400);
    if total < minute {
        format!("{total}s")
    } else if total < hour {
        format!("{}m {:02}s", total / minute, total % minute)
    } else if total < day {
        format!("{}h {:02}m", total / hour, total % hour / minute)
    } else if total < 100 * day {
        format!("{}d {:02}h", total / day, total % day / hour)
    } else {
        ">99d".to_owned()
    }
}

/// Writes how long ago a run finished, with one unit.
///
/// Examples are `45s ago`, `12m ago`, `2h ago`, and `3d ago`. A duration of
/// 100 days or more gives `>99d ago`.
#[must_use]
pub fn format_ago(d: Duration) -> String {
    let total = d.as_secs();
    let (minute, hour, day) = (60, 3_600, 86_400);
    if total < minute {
        format!("{total}s ago")
    } else if total < hour {
        format!("{}m ago", total / minute)
    } else if total < day {
        format!("{}h ago", total / hour)
    } else if total < 100 * day {
        format!("{}d ago", total / day)
    } else {
        ">99d ago".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_writes_two_units_and_a_cap() {
        let cases: [(u64, &str); 13] = [
            (0, "0s"),
            (45, "45s"),
            (59, "59s"),
            (60, "1m 00s"),
            (723, "12m 03s"),
            (3_599, "59m 59s"),
            (3_600, "1h 00m"),
            (8_040, "2h 14m"),
            (86_399, "23h 59m"),
            (86_400, "1d 00h"),
            (3 * 86_400 + 4 * 3_600, "3d 04h"),
            (99 * 86_400 + 23 * 3_600 + 3_599, "99d 23h"),
            (100 * 86_400, ">99d"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_elapsed(Duration::from_secs(seconds)), expected);
        }
    }

    #[test]
    fn format_elapsed_is_never_wider_than_seven_columns() {
        let mut seconds: u64 = 0;
        while seconds < 200 * 86_400 {
            let text = format_elapsed(Duration::from_secs(seconds));
            assert!(text.chars().count() <= 7, "{seconds}s gave {text:?}");
            seconds += 997;
        }
        let text = format_elapsed(Duration::from_secs(u64::MAX));
        assert_eq!(text, ">99d");
    }

    #[test]
    fn format_ago_writes_one_unit_and_a_cap() {
        let cases: [(u64, &str); 8] = [
            (0, "0s ago"),
            (45, "45s ago"),
            (12 * 60 + 30, "12m ago"),
            (2 * 3_600 + 59, "2h ago"),
            (3 * 86_400 + 5, "3d ago"),
            (99 * 86_400 + 86_399, "99d ago"),
            (100 * 86_400, ">99d ago"),
            (u64::MAX, ">99d ago"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(format_ago(Duration::from_secs(seconds)), expected);
        }
    }
}
