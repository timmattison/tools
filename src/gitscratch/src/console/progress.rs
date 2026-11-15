//! The handle of the progress display that [`Console`](crate::Console) draws.

use std::time::Duration;

use crate::progress::{ReplayObserver, ReplayProgress};

/// The handle that a tool gives to a replay, so the replay can draw its
/// progress on standard error.
///
/// [`Console::progress`](crate::Console::progress) gives the handle. The handle
/// is a [`ReplayObserver`], and a clone of it reports to the same display.
#[derive(Clone)]
pub struct Progress;

impl ReplayObserver for Progress {
    fn report(&self, _progress: ReplayProgress) {}
}

/// Format the elapsed time of a display as `M:SS`, or as `H:MM:SS` from one
/// hour on.
fn clock(_elapsed: Duration) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table of elapsed times and the text of the clock for each one.
    #[test]
    fn the_clock_reads_minutes_and_seconds_and_adds_hours_from_one_hour() {
        for (seconds, text) in [
            (0, "0:00"),
            (5, "0:05"),
            (65, "1:05"),
            (3600, "1:00:00"),
            (3725, "1:02:05"),
        ] {
            assert_eq!(clock(Duration::from_secs(seconds)), text, "{seconds} s");
        }
    }

    /// The clock drops the part of a second.
    #[test]
    fn the_clock_drops_the_part_of_a_second() {
        assert_eq!(clock(Duration::from_millis(5999)), "0:05");
    }
}
