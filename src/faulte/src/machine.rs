//! The machine that `faulte` reads, behind one trait.
//!
//! Every fact of this Mac reaches `faulte` through this module: the sample of
//! `top`, the process table of `ps`, the Claude Code role of each process, the
//! registry record of each session, the counters of the virtual memory system,
//! the account names, and the clock. One trait holds all of them, so a test
//! gives each fact as a plain value and no test reads the real machine.
//!
//! The real reader of macOS is in [`macos`]. It is the only part of `faulte`
//! that runs a command or calls the kernel.

use std::time::Duration;

use crate::duration::Span;

/// Gives the time that the ranking divides the faults of the sample by.
///
/// `top` prints the local time of each of its two samples, and the difference
/// of the two times is the true window of the delta. That time is longer than
/// the interval on a Mac that is busy: a sample of 2 seconds took 4 seconds on
/// 2026-09-18. A rate over the interval is then too large.
///
/// Two limits guard the answer. The window is never shorter than the interval,
/// because `top` never samples for less time than it was asked for. The window
/// is never longer than `wall`, the time that the whole run of `top` took,
/// because a change of the local time clock between the two samples adds an
/// hour that no process was running for. `elapsed` of `None` gives the
/// interval, because the output then states no times to subtract.
#[must_use]
pub fn sample_window(interval: Span, elapsed: Option<Duration>, wall: Duration) -> Duration {
    let interval = Duration::from(interval);
    match elapsed {
        Some(elapsed) => interval.max(elapsed.min(wall)),
        None => interval,
    }
}

#[cfg(test)]
mod tests {
    use super::sample_window;
    use crate::duration::Span;
    use std::time::Duration;

    /// Gives the interval of a test as a [`Span`].
    fn span(text: &str) -> Span {
        text.parse().expect("the text is a span")
    }

    /// A busy Mac makes `top` sample for longer than the interval. The faults
    /// of the sample were made over that longer time, so it is the window.
    #[test]
    fn an_elapsed_time_longer_than_the_interval_is_the_window() {
        assert_eq!(
            sample_window(
                span("2s"),
                Some(Duration::from_secs(4)),
                Duration::from_secs(5)
            ),
            Duration::from_secs(4)
        );
    }

    /// `top` never samples for less time than the interval. A shorter time
    /// between the two clock lines is a clock that moved back, and a shorter
    /// window makes every rate of the ranking too large.
    #[test]
    fn an_elapsed_time_shorter_than_the_interval_does_not_shorten_the_window() {
        assert_eq!(
            sample_window(
                span("5s"),
                Some(Duration::from_secs(3)),
                Duration::from_secs(6)
            ),
            Duration::from_secs(5)
        );
    }

    /// A change of daylight saving time between the two samples adds an hour
    /// to the difference of the two clock lines. The run of `top` took 3
    /// seconds, so no process made faults for an hour.
    #[test]
    fn the_wall_time_limits_an_absurd_elapsed_time() {
        assert_eq!(
            sample_window(
                span("2s"),
                Some(Duration::from_secs(3600)),
                Duration::from_secs(3)
            ),
            Duration::from_secs(3)
        );
    }

    /// Output that states no times gives the interval. The interval is what
    /// `top` was asked for, and it is the best answer that is left.
    #[test]
    fn no_elapsed_time_gives_the_interval() {
        assert_eq!(
            sample_window(span("5s"), None, Duration::from_secs(9)),
            Duration::from_secs(5)
        );
    }
}
