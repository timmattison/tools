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

use std::str::FromStr;
use std::time::Duration;

use chrono::NaiveDateTime;

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
    ///
    /// The delay of `top` is a minimum. On a loaded machine, the samples of a
    /// 2 s delay were 4 s apart. So the window of the counts is
    /// `max(interval, elapsed)`, and the interval alone when this is `None`.
    ///
    /// The clock lines are local time with one-second steps, and they name no
    /// zone. A change of the clock between the samples, for example to
    /// daylight saving time, changes this value. A backward change gives
    /// `None`. A forward change gives a value that is too long, so a caller
    /// also limits the window to the wall time that the run of `top` took.
    pub elapsed: Option<Duration>,
}

/// The reason why the output of `top` is not a sample that `faulte` can rank.
///
/// The parser fails closed. An output that it does not know is an error, never
/// an empty or partial sample, because a partial ranking looks the same as a
/// correct one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopParseError {
    /// The output does not hold exactly one header row for each sample that
    /// `faulte` asked for.
    #[error(
        "top printed {found} header rows, and faulte asked for {SAMPLES} samples with one header \
         row each: the output ended early, or this top prints a format that faulte does not know"
    )]
    SampleCount {
        /// The count of header rows in the output.
        found: usize,
    },
    /// The second sample holds no row. A Mac always runs processes, so the
    /// output is incomplete.
    #[error(
        "top listed no process in its second sample: a Mac always runs processes, so the \
         output is incomplete, and faulte does not rank an empty sample"
    )]
    NoRows,
    /// A header row is not the columns that `faulte` asked for.
    #[error(
        "top printed a header row that faulte did not ask for, at line {number}: {line:?}. \
         faulte asked for the columns {PID_HEADER} {FAULTS_HEADER} (-stats {STATS})"
    )]
    UnexpectedHeader {
        /// The number of the line in the output, from 1.
        number: usize,
        /// The line as `top` printed it.
        line: String,
    },
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
    let lines: Vec<&str> = output.lines().collect();
    let headers = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_header_row(line))
        .map(|(index, line)| {
            if line.split_ascii_whitespace().eq(HEADER) {
                Ok(index)
            } else {
                Err(TopParseError::UnexpectedHeader {
                    number: index + 1,
                    line: (*line).to_owned(),
                })
            }
        })
        .collect::<Result<Vec<usize>, _>>()?;
    let [_, second_header] = headers[..] else {
        return Err(TopParseError::SampleCount {
            found: headers.len(),
        });
    };
    let rows: Vec<FaultCount> = lines
        .iter()
        .enumerate()
        .skip(second_header + 1)
        .filter(|(_, line)| !line.trim_ascii().is_empty())
        .map(|(index, line)| {
            parse_row(line).ok_or_else(|| TopParseError::MalformedRow {
                number: index + 1,
                line: (*line).to_owned(),
            })
        })
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Err(TopParseError::NoRows);
    }
    Ok(TopSample {
        rows,
        elapsed: elapsed(&lines),
    })
}

/// The first token of the first line of the header block of each sample.
const PROCESSES: &str = "Processes:";

/// The format of the clock line under each `Processes:` line. It is local
/// time, and it names no zone.
const CLOCK_FORMAT: &str = "%Y/%m/%d %H:%M:%S";

/// Gives the time from the first clock line to the second one.
///
/// The clock line is the line after each `Processes:` line. The result is
/// `None` unless there are exactly two such lines, both parse, and the
/// difference is more than zero. The clock lines name no zone, so this
/// subtracts them as dates and times without a zone.
fn elapsed(lines: &[&str]) -> Option<Duration> {
    let clocks: Vec<Option<NaiveDateTime>> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.split_ascii_whitespace().next() == Some(PROCESSES))
        .map(|(index, _)| {
            let clock = lines.get(index + 1)?;
            NaiveDateTime::parse_from_str(clock.trim_ascii(), CLOCK_FORMAT).ok()
        })
        .collect();
    let [Some(first), Some(second)] = clocks[..] else {
        return None;
    };
    (second - first)
        .to_std()
        .ok()
        .filter(|elapsed| !elapsed.is_zero())
}

/// The first token of the header row of each sample.
const PID_HEADER: &str = "PID";

/// The token that `top` prints in the header row for the column `faults`.
const FAULTS_HEADER: &str = "FAULTS";

/// The header row that `top` prints for [`STATS`], as tokens. The parser
/// compares tokens, so the column widths of `top` do not matter.
const HEADER: [&str; 2] = [PID_HEADER, FAULTS_HEADER];

/// Tells whether `line` is a header row: its first token is `PID`. No other
/// line of the output starts with that token.
///
/// The parser splits each line on ASCII white space only. `top` prints ASCII,
/// so a line that another separator splits is not a line of `top`.
fn is_header_row(line: &str) -> bool {
    line.split_ascii_whitespace().next() == Some(PID_HEADER)
}

/// Reads a row: a PID, then a fault count, and no other token.
fn parse_row(line: &str) -> Option<FaultCount> {
    let mut tokens = line.split_ascii_whitespace();
    let (Some(pid), Some(faults), None) = (tokens.next(), tokens.next(), tokens.next()) else {
        return None;
    };
    Some(FaultCount {
        pid: Pid::new(number(pid)?),
        faults: number(faults)?,
    })
}

/// The marks that `top` can put after a number. The parser removes one.
const TRAILING_MARKS: [char; 2] = ['+', '-'];

/// Reads a number from `token`, after one trailing `+` or `-` is removed.
///
/// The rest must be ASCII digits only. The parse of a Rust integer also
/// accepts a leading `+`, and `top` never prints one.
fn number<T: FromStr>(token: &str) -> Option<T> {
    let digits = token.strip_suffix(TRAILING_MARKS).unwrap_or(token);
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // An empty text fails the parse, and so does a value too large for `T`.
    digits.parse().ok()
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
        sample_at(&format!("2026/09/18 {time}"), header, rows)
    }

    /// Gives one sample with the clock line `clock`.
    fn sample_at(clock: &str, header: &str, rows: &str) -> String {
        format!(
            "Processes: 3 total, 1 running, 2 sleeping, 9 threads \n\
             {clock}\n\
             Load Avg: 1.00, 1.00, 1.00 \n\
             \n\
             {header}\n\
             {rows}"
        )
    }

    /// Gives two samples with the clock lines `first` and `second`.
    fn two_samples_at(first: &str, second: &str) -> String {
        sample_at(first, HEADER_ROW, "10     9000000   \n")
            + &sample_at(second, HEADER_ROW, "10     3         \n")
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

    /// One sample, three samples, and no sample are each refused. The count
    /// of samples is the count of header rows.
    #[test]
    fn an_output_that_is_not_two_samples_is_refused_with_its_count() {
        let one = sample("12:00:00", HEADER_ROW, "10     3         \n");
        let three = two_samples("10     9000000   \n", "10     3         \n")
            + &sample("12:00:04", HEADER_ROW, "10     5         \n");
        let header_block_only = "Processes: 3 total \n2026/09/18 12:00:00\n\n";
        let cases = [(one.as_str(), 1), (three.as_str(), 3), (header_block_only, 0), ("", 0)];

        for (text, found) in cases {
            let error = parse(text).expect_err("an output that is not two samples is refused");

            assert_eq!(error, TopParseError::SampleCount { found }, "the text {text:?}");
            assert!(
                error.to_string().starts_with(&format!("top printed {found} header rows")),
                "the message gives the count: {error}"
            );
        }
    }

    /// A header row that starts with `PID` and does not hold exactly the
    /// columns `PID FAULTS` is refused, with its line. The last case is right
    /// in the first sample and wrong in the second.
    #[test]
    fn a_header_row_that_faulte_did_not_ask_for_is_refused_with_its_line() {
        let command = "PID    COMMAND   ";
        let extra = "PID    FAULTS    COMMAND  ";
        let bare = "PID";
        let cases = [
            (
                sample("12:00:00", command, "10     launchd   \n")
                    + &sample("12:00:02", command, "10     launchd   \n"),
                command,
                5,
            ),
            (
                sample("12:00:00", extra, "10     3    launchd\n")
                    + &sample("12:00:02", extra, "10     3    launchd\n"),
                extra,
                5,
            ),
            (
                sample("12:00:00", HEADER_ROW, "10     3         \n")
                    + &sample("12:00:02", bare, "10     3         \n"),
                bare,
                11,
            ),
        ];

        for (text, header, number) in cases {
            let error = parse(&text).expect_err("a header row of other columns is refused");

            assert_eq!(
                error,
                TopParseError::UnexpectedHeader {
                    number,
                    line: header.to_owned()
                },
                "the header {header:?}"
            );
            let message = error.to_string();
            assert!(
                message.starts_with("top printed a header row that faulte did not ask for")
                    && message.contains(&format!("{header:?}"))
                    && message.contains("PID FAULTS"),
                "the message shows the line and the columns that faulte asked for: {message}"
            );
        }
    }

    /// A second sample with no row is refused. An empty ranking looks the
    /// same as a quiet machine, so `faulte` never prints one.
    #[test]
    fn a_second_sample_with_no_row_is_refused() {
        let text = two_samples("10     9000000   \n20     12        \n", "");

        let error = parse(&text).expect_err("a second sample with no row is refused");

        assert_eq!(error, TopParseError::NoRows);
        assert!(
            error.to_string().contains("no process in its second sample"),
            "the message says which sample is empty: {error}"
        );
    }

    /// A blank line, a line of spaces, and the spaces after a row are not
    /// rows, and they are not errors. A second sample of blank lines only
    /// holds no row.
    #[test]
    fn blank_lines_and_trailing_spaces_are_not_rows() {
        let text = two_samples(
            "10     9000000   \n",
            "\n10     3         \n   \n\n20     4000\t  \n\n  \n",
        );

        let sample = parse(&text).expect("blank lines are not errors");

        assert_eq!(sample.rows, [row(10, 3), row(20, 4_000)]);

        let blank_only = two_samples("10     9000000   \n", "\n   \n\n");
        assert_eq!(parse(&blank_only), Err(TopParseError::NoRows));
    }

    /// `top` can mark a number with one trailing `+` or `-`. The parser
    /// removes that one mark from each token.
    #[test]
    fn one_trailing_plus_or_minus_is_removed_from_each_number() {
        let text = two_samples(
            "10     9000000   \n",
            "10     12024+    \n20     5-        \n30+    7         \n40-    0+        \n",
        );

        let sample = parse(&text).expect("a trailing mark is not an error");

        assert_eq!(
            sample.rows,
            [row(10, 12_024), row(20, 5), row(30, 7), row(40, 0)]
        );
    }

    /// Only one mark comes off. A second mark makes the row malformed.
    #[test]
    fn two_trailing_marks_make_a_malformed_row() {
        for bad in ["10     12+-      ", "10     12--      ", "10     12++      ", "10+-   12        "] {
            let text = two_samples("10     9000000   \n", &format!("{bad}\n"));

            assert_eq!(
                parse(&text),
                Err(TopParseError::MalformedRow {
                    number: 12,
                    line: bad.to_owned()
                }),
                "the row {bad:?}"
            );
        }
    }

    /// Each row that is not exactly a `u32` and a `u64` is refused, with its
    /// line and its number. A sign in front, a text of other digits, and a
    /// number too large for its type are malformed. Multi-byte text gives the
    /// same error, and no panic.
    #[test]
    fn a_malformed_row_is_refused_with_its_line() {
        let malformed = [
            "abc    12        ",
            "12               ",
            "12     34     56 ",
            "-5     12        ",
            "12     -5        ",
            "+12    34        ",
            "12     +34       ",
            "12     +         ",
            "12     -         ",
            "12     3.5       ",
            "12     1e3       ",
            "0x1F   3         ",
            "4294967296 5     ",
            "12     18446744073709551616",
            "日本語 12        ",
            "12     🎉        ",
            "café   3         ",
            "12     34🎉      ",
            "１２   3         ",
            "12\u{a0}3         ",
        ];
        for bad in malformed {
            let text = two_samples(
                "10     9000000   \n",
                &format!("10     3         \n{bad}\n20     4000      \n"),
            );

            let error = parse(&text).expect_err("a malformed row is refused");

            assert_eq!(
                error,
                TopParseError::MalformedRow {
                    number: 13,
                    line: bad.to_owned()
                },
                "the row {bad:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("at line 13") && message.contains(&format!("{bad:?}")),
                "the message shows the line and its number: {message}"
            );
        }
    }

    /// The largest values of each type are rows, not errors.
    #[test]
    fn the_largest_pid_and_the_largest_count_parse() {
        let text = two_samples(
            "10     9000000   \n",
            "4294967295 18446744073709551615\n0      0         \n",
        );

        let sample = parse(&text).expect("the largest values parse");

        assert_eq!(sample.rows, [row(u32::MAX, u64::MAX), row(0, 0)]);
    }

    /// The clock lines of the real capture are `12:31:20` and `12:31:24`. The
    /// delay was 2 s, and the load of the machine made the samples 4 s apart.
    #[test]
    fn the_real_capture_gives_the_time_between_its_clock_lines() {
        let sample = parse(REAL_CAPTURE).expect("the real capture parses");

        assert_eq!(sample.elapsed, Some(Duration::from_secs(4)));
    }

    /// The difference is between two dates and times, so it holds across
    /// midnight. Spaces after the clock line are not part of the time.
    #[test]
    fn the_time_between_the_clock_lines_is_their_difference() {
        let cases = [
            ("2026/09/18 12:00:00", "2026/09/18 12:00:02", 2),
            ("2026/09/18 23:59:59", "2026/09/19 00:00:03", 4),
            ("2026/12/31 23:59:58  ", "2027/01/01 00:00:08 ", 10),
        ];
        for (first, second, seconds) in cases {
            let sample = parse(&two_samples_at(first, second)).expect("the samples parse");

            assert_eq!(
                sample.elapsed,
                Some(Duration::from_secs(seconds)),
                "the clock lines {first:?} and {second:?}"
            );
        }
    }

    /// A clock line that is absent or bad is not an error. The sample has no
    /// elapsed time, and `faulte` uses the interval. A difference of zero or
    /// less is no time either.
    #[test]
    fn a_missing_bad_or_backward_clock_gives_no_elapsed_time() {
        let cases = [
            ("2026/09/18 12:00:05".to_owned(), "2026/09/18 12:00:02".to_owned()),
            ("2026/09/18 12:00:02".to_owned(), "2026/09/18 12:00:02".to_owned()),
            ("not a time".to_owned(), "2026/09/18 12:00:02".to_owned()),
            ("2026/09/18 12:00:02".to_owned(), String::new()),
            ("2026/09/18 12:00".to_owned(), "2026/09/18 12:00:02".to_owned()),
            ("2026/09/18 25:00:00".to_owned(), "2026/09/18 12:00:02".to_owned()),
            ("２０２６/09/18 12:00:00".to_owned(), "2026/09/18 12:00:02".to_owned()),
        ];
        for (first, second) in cases {
            let sample = parse(&two_samples_at(&first, &second)).expect("a bad clock is no error");

            assert_eq!(sample.elapsed, None, "the clock lines {first:?} and {second:?}");
            assert_eq!(sample.rows, [row(10, 3)]);
        }

        let no_block = format!("{HEADER_ROW}\n10     9000000   \n{HEADER_ROW}\n10     3         \n");
        let one_block = format!("{HEADER_ROW}\n10     9000000   \n")
            + &sample("12:00:02", HEADER_ROW, "10     3         \n");
        for text in [no_block, one_block] {
            let sample = parse(&text).expect("an output without clock lines parses");

            assert_eq!(sample.elapsed, None, "the text {text:?}");
        }
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
