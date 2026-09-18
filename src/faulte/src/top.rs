//! The fault sampler: the command line of `/usr/bin/top`, and the parser of
//! its output.
//!
//! Only `/usr/bin/top` reads the fault counter of every process. It carries the
//! entitlement `com.apple.system-task-ports.read`, which a third-party binary
//! cannot get. For a process of another account, `proc_pidinfo` and
//! `proc_pid_rusage` return `EPERM`.

use std::time::Duration;

use crate::duration::Span;

/// The path of the sampler. A `top` found on the `PATH` can be a different
/// program with a different output, so `faulte` names the one of macOS.
pub const PROGRAM: &str = "/usr/bin/top";

/// The number of samples that `faulte` asks `top` for.
///
/// The first sample holds the counter since each process started. The second
/// sample holds the change over the interval, which is the rate that `faulte`
/// ranks by.
const SAMPLES: usize = 2;

/// The value of `-stats`: the columns that `faulte` asks `top` for. Both are
/// numbers, so no command name with spaces can move a column.
const STATS: &str = "pid,faults";

/// Gives the arguments of [`PROGRAM`] for one sample of `interval`.
///
/// `max_processes` is `kern.maxproc`. `top` prints no more rows than `-n`
/// allows, and no more processes than `kern.maxproc` can exist, so this limit
/// drops no process.
#[must_use]
pub fn arguments(interval: Span, max_processes: u32) -> Vec<String> {
    let seconds = Duration::from(interval).as_secs();
    vec![
        // Logging mode: print this count of samples, then stop.
        "-l".to_owned(),
        SAMPLES.to_string(),
        // The delay between the samples, in whole seconds.
        "-s".to_owned(),
        seconds.to_string(),
        // Delta mode: the second sample gives the change of each counter.
        "-c".to_owned(),
        "d".to_owned(),
        "-n".to_owned(),
        max_processes.to_string(),
        "-stats".to_owned(),
        STATS.to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` as a [`Span`] for a test.
    fn span(text: &str) -> Span {
        text.parse().expect("the test gives a valid duration")
    }

    /// The list is exact: two samples, the delay in whole seconds, the delta
    /// mode, a process limit, and the two numeric columns.
    #[test]
    fn the_arguments_ask_for_two_delta_samples_of_the_pid_and_the_faults() {
        assert_eq!(
            arguments(span("5s"), 16_000),
            [
                "-l",
                "2",
                "-s",
                "5",
                "-c",
                "d",
                "-n",
                "16000",
                "-stats",
                "pid,faults"
            ]
        );
    }

    /// `top` reads the delay as a number of seconds. The text of a [`Span`]
    /// carries a unit, so the arguments give the seconds, not that text.
    #[test]
    fn the_delay_is_a_bare_number_of_seconds() {
        let arguments = arguments(span("10m"), 4_000);

        assert_eq!(arguments.get(2..4), Some(["-s", "600"].map(String::from).as_slice()));
        assert_eq!(arguments.get(6..8), Some(["-n", "4000"].map(String::from).as_slice()));
    }
}
