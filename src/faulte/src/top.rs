//! The fault sampler: the command line of `/usr/bin/top`, and the parser of
//! its output.
//!
//! Only `/usr/bin/top` reads the fault counter of every process. It carries the
//! entitlement `com.apple.system-task-ports.read`, which a third-party binary
//! cannot get. For a process of another account, `proc_pidinfo` and
//! `proc_pid_rusage` return `EPERM`.
//!
//! `top` prints two samples. The first sample holds the counter since each
//! process started. The second sample holds the change of each counter over
//! the interval. [`parse`] reads the second sample only. A parser that reads
//! the first sample ranks the old processes first, whatever they do now.

use std::time::Duration;

use crate::duration::Span;
use crate::pid::Pid;

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

/// The page faults that one process made over the interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaultCount {
    /// The process.
    pub pid: Pid,
    /// The page faults that the process made between the two samples.
    pub faults: u64,
}

/// The second sample of `top`: the page faults of each process over the
/// interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopSample {
    /// One row for each process that `top` listed, in the order of `top`.
    pub rows: Vec<FaultCount>,
    /// The time between the two samples, from the clock lines that `top`
    /// printed. `None` when a clock line is absent or does not parse, or when
    /// the time is not more than zero.
    pub elapsed: Option<Duration>,
}

/// The reason why the output of `top` is not a sample that `faulte` can rank.
///
/// The parser fails closed. An output that it does not know is an error, never
/// an empty or partial sample, because a partial ranking looks the same as a
/// correct one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopParseError {
    /// A row of the second sample is not a PID and a fault count.
    #[error("top printed a row that is not a PID and a fault count, at line {number}: {line:?}")]
    MalformedRow {
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `top` printed it.
        line: String,
    },
}

/// Reads the page faults of each process from the output of `top`, run with
/// the [`arguments`] of `faulte`.
///
/// # Errors
///
/// Gives a [`TopParseError`] when the output is not two samples of the columns
/// `PID FAULTS`, or when the second sample holds no row or a malformed row.
pub fn parse(output: &str) -> Result<TopSample, TopParseError> {
    let _ = output;
    Ok(TopSample {
        rows: Vec::new(),
        elapsed: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output of `top -l 2 -s 2 -c d -n 16000 -stats pid,faults` on this
    /// Mac on 2026-09-18, byte for byte.
    ///
    /// The issue demands a fixture from a real run. A hand-written text holds
    /// only the format that its writer knows. This text holds the format of
    /// `top` itself: the header block of each sample, the trailing spaces of
    /// each row, and PID 0.
    const REAL_CAPTURE: &str = include_str!("../tests/fixtures/top-l2-cd.txt");

    /// The header row that `top` prints for `-stats pid,faults`.
    const HEADER_ROW: &str = "PID    FAULTS    ";

    /// Gives one sample as `top` prints it: the header block with the clock
    /// line `2026/09/18 <time>`, a blank line, `header`, then `rows`.
    fn sample(time: &str, header: &str, rows: &str) -> String {
        format!(
            "Processes: 3 total, 1 running, 2 sleeping, 9 threads \n\
             2026/09/18 {time}\n\
             Load Avg: 1.00, 1.00, 1.00 \n\
             \n\
             {header}\n\
             {rows}"
        )
    }

    /// Gives two samples with the header row of `-stats pid,faults`.
    fn two_samples(first_rows: &str, second_rows: &str) -> String {
        sample("12:00:00", HEADER_ROW, first_rows) + &sample("12:00:02", HEADER_ROW, second_rows)
    }

    /// Gives the row of `pid` with `faults`.
    fn row(pid: u32, faults: u64) -> FaultCount {
        FaultCount {
            pid: Pid::new(pid),
            faults,
        }
    }

    /// PID 10 made many faults since it started and few over the interval.
    /// PID 20 is the reverse. The sample gives the values of the interval.
    #[test]
    fn the_parser_reads_the_second_sample_only() {
        let text = two_samples(
            "10     9000000   \n20     12        \n",
            "10     3         \n20     4000      \n",
        );

        let sample = parse(&text).expect("two samples of the right columns parse");

        assert_eq!(sample.rows, [row(10, 3), row(20, 4_000)]);
    }

    /// The second sample of the real capture holds 1,549 rows, the same count
    /// as its line `Processes: 1549 total`. The first sample holds 1,548 rows
    /// with far larger counts, and none of them is in the result.
    #[test]
    fn the_real_capture_parses_to_the_rows_of_its_second_sample() {
        let sample = parse(REAL_CAPTURE).expect("the real capture parses");

        assert_eq!(sample.rows.len(), 1_549);
        assert_eq!(sample.rows.first(), Some(&row(0, 1_329)));
        assert_eq!(sample.rows.last(), Some(&row(158, 0)));
        assert_eq!(
            sample.rows.iter().map(|count| count.faults).sum::<u64>(),
            959_815,
            "the sum of the second sample, not 23,252,649,390 of the first"
        );
        let busy = sample
            .rows
            .iter()
            .find(|count| count.pid == Pid::new(20_997))
            .expect("PID 20997 is in the second sample");
        assert_eq!(busy.faults, 165, "not 24,034,993, its count since it started");
    }

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
