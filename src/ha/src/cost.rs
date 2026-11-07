//! The list-price cost of one job and of one run (issue #639).
//!
//! GitHub gives no cost for one job. The `timing` endpoint is closing down,
//! and its `billable` data is empty. Thus the cost here is an estimate at list
//! price, from data that `ha` already reads: the runner labels of the job, its
//! time on a runner, and the visibility of its repository.
//!
//! The estimate is not the bill. The free minutes that an account includes can
//! make the bill $0.

use std::time::Duration;

/// The number of seconds in one billed minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// The whole minutes that GitHub bills for a time on a runner.
///
/// GitHub rounds each job up to a whole minute. Zero seconds give zero minutes.
#[must_use]
pub fn billed_minutes(time: Duration) -> u64 {
    time.as_secs().div_ceil(SECONDS_PER_MINUTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billed_minutes_round_up_to_a_whole_minute() {
        let cases = [
            (0, 0),
            (1, 1),
            (59, 1),
            (60, 1),
            (61, 2),
            (120, 2),
            (121, 3),
        ];
        for (seconds, minutes) in cases {
            assert_eq!(
                billed_minutes(Duration::from_secs(seconds)),
                minutes,
                "{seconds} s"
            );
        }
    }
}
