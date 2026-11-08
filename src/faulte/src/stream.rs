//! The parser of an endless run of `top`.
//!
//! `top -l 0` prints one sample after the other until the caller stops it.
//! [`StreamParser`] turns the bytes of that run into [`ParsedEvent`] values. It
//! holds no clock and no process, so a test feeds it bytes and reads events.
//!
//! A sample ends when the `Processes:` line of the next sample arrives. A timer
//! on an idle pipe would end it sooner, but it would also cut a sample that
//! `top` is slow to write.

use std::time::Duration;

use chrono::NaiveDateTime;

use crate::top::{self, FaultCount, TopSample};

/// What the parser found in the bytes that it has read so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedEvent {
    /// The header of a sample arrived. `finished` is the sample before it, when
    /// that sample is complete and usable. The first sample of a child never
    /// comes out here, because it holds the counter since each process started.
    Header {
        /// The sample that this header ended.
        finished: Option<TopSample>,
    },
    /// Output that is not a sample. `reason` is the text of a
    /// `MachineError::TopOutput`.
    Refused {
        /// Why the parser refused the output.
        reason: String,
    },
}

/// The byte that ends a line.
const NEWLINE: u8 = b'\n';

/// Where the parser is inside the sample that it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// The `Processes:` line arrived. The next line is the clock line.
    ExpectClock,
    /// The clock line arrived. Lines up to the `PID` line are the rest of the
    /// header block.
    BeforeHeader,
    /// The `PID` line arrived. Each non-blank line is a row.
    Rows,
}

/// The sample that the parser reads now.
#[derive(Debug)]
struct Current {
    /// Where the parser is inside the sample.
    position: Position,
    /// The clock line of the sample, when it parsed.
    clock: Option<NaiveDateTime>,
    /// The rows that arrived.
    rows: Vec<FaultCount>,
}

impl Current {
    /// Starts a sample at its `Processes:` line.
    fn new() -> Self {
        Self {
            position: Position::ExpectClock,
            clock: None,
            rows: Vec::new(),
        }
    }
}

/// Reads the bytes of an endless run of `top` and gives events.
#[derive(Debug, Default)]
pub struct StreamParser {
    /// The sample that the parser reads now. `None` before the first
    /// `Processes:` line.
    current: Option<Current>,
    /// The clock line of the sample before `current`, when it parsed.
    previous_clock: Option<NaiveDateTime>,
    /// Tells whether the parser ended a sample before. The first sample of a
    /// child holds the counter since each process started, so it gives no
    /// result.
    finished_one: bool,
    /// The bytes of the line that has not ended yet. A read can end inside a
    /// line, so the parser keeps the start of the line until the rest arrives.
    pending: Vec<u8>,
}

impl StreamParser {
    /// Makes a parser that has read no byte.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `bytes` and gives the events that they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ParsedEvent> {
        let mut events = Vec::new();
        let mut rest = bytes;
        while let Some(newline) = rest.iter().position(|byte| *byte == NEWLINE) {
            let (head, tail) = rest.split_at(newline);
            rest = tail.get(1..).unwrap_or_default();
            self.pending.extend_from_slice(head);
            let line = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            self.line(line.strip_suffix('\r').unwrap_or(&line), &mut events);
        }
        self.pending.extend_from_slice(rest);
        events
    }

    /// Reads one whole line.
    fn line(&mut self, line: &str, events: &mut Vec<ParsedEvent>) {
        if line.split_ascii_whitespace().next() == Some(top::PROCESSES) {
            let finished = self.finish();
            events.push(ParsedEvent::Header { finished });
            self.current = Some(Current::new());
            return;
        }
        let Some(current) = self.current.as_mut() else {
            return;
        };
        match current.position {
            Position::ExpectClock => {
                current.clock =
                    NaiveDateTime::parse_from_str(line.trim_ascii(), top::CLOCK_FORMAT).ok();
                current.position = Position::BeforeHeader;
            }
            Position::BeforeHeader => {
                if top::is_header_row(line) {
                    current.position = Position::Rows;
                }
            }
            Position::Rows => {
                if let Some(row) = top::parse_row(line) {
                    current.rows.push(row);
                }
            }
        }
    }

    /// Ends the sample that the parser reads. Gives the sample when it is
    /// usable. The first sample of a child gives `None`.
    fn finish(&mut self) -> Option<TopSample> {
        let current = self.current.take()?;
        let before = std::mem::replace(&mut self.previous_clock, current.clock);
        let elapsed = elapsed_between(before, current.clock);
        if !std::mem::replace(&mut self.finished_one, true) {
            return None;
        }
        Some(TopSample {
            rows: current.rows,
            elapsed,
        })
    }
}

/// Gives the time from `before` to `after`. The result is `None` unless both
/// clock lines parsed and the difference is more than zero, which is the filter
/// of [`top::parse`].
fn elapsed_between(
    before: Option<NaiveDateTime>,
    after: Option<NaiveDateTime>,
) -> Option<Duration> {
    (after? - before?)
        .to_std()
        .ok()
        .filter(|elapsed| !elapsed.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::MachineError;
    use crate::pid::Pid;
    use crate::top::{FaultCount, TopParseError};
    use std::time::Duration;

    /// The header row that `top` prints for `-stats pid,faults`.
    const HEADER_ROW: &str = "PID    FAULTS    ";

    /// Gives one sample as `top` prints it, with the clock line `clock`.
    fn sample_at(clock: &str, rows: &str) -> String {
        format!(
            "Processes: 3 total, 1 running, 2 sleeping, 9 threads \n\
             {clock}\n\
             Load Avg: 1.00, 1.00, 1.00 \n\
             \n\
             {HEADER_ROW}\n\
             {rows}"
        )
    }

    /// Gives one sample whose clock line is `2026/09/18 <time>`.
    fn sample(time: &str, rows: &str) -> String {
        sample_at(&format!("2026/09/18 {time}"), rows)
    }

    /// Gives the row of `pid` with `faults`.
    fn row(pid: u32, faults: u64) -> FaultCount {
        FaultCount {
            pid: Pid::new(pid),
            faults,
        }
    }

    /// Feeds `text` to a new parser in one read.
    fn events_of(text: &str) -> Vec<ParsedEvent> {
        StreamParser::new().feed(text.as_bytes())
    }

    /// Three samples give three headers. The first sample holds the counter
    /// since each process started, so it comes out as no sample. The second
    /// sample comes out when the third header arrives, and the third waits.
    #[test]
    fn the_first_sample_is_discarded_and_the_second_comes_out() {
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", "10     3         \n20     4000      \n")
            + &sample("12:00:06", "10     5         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                ParsedEvent::Header { finished: None },
                ParsedEvent::Header { finished: None },
                ParsedEvent::Header {
                    finished: Some(TopSample {
                        rows: vec![row(10, 3), row(20, 4_000)],
                        elapsed: Some(Duration::from_secs(3)),
                    })
                },
            ]
        );
    }

    /// Gives the three samples of the tests above.
    fn three_samples() -> String {
        sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", "10     3         \n20     4000      \n")
            + &sample("12:00:06", "10     5         \n")
    }

    /// A `Processes:` line that arrives in two reads is one line, and so is the
    /// row after it. The same input split at each position gives the same
    /// events as the input in one read, and so does one byte for each read.
    #[test]
    fn a_line_in_two_reads_is_one_line() {
        let text = three_samples();
        let bytes = text.as_bytes();
        let whole = events_of(&text);
        assert_eq!(whole.len(), 3, "the reference run holds three headers");

        for split in 0..=bytes.len() {
            let (head, tail) = bytes.split_at(split);
            let mut parser = StreamParser::new();
            let mut events = parser.feed(head);
            events.extend(parser.feed(tail));

            assert_eq!(events, whole, "the input split at byte {split}");
        }

        let mut parser = StreamParser::new();
        let one_by_one: Vec<ParsedEvent> = bytes
            .chunks(1)
            .flat_map(|chunk| parser.feed(chunk))
            .collect();
        assert_eq!(one_by_one, whole, "one byte in each read");
    }

    /// The last sample has no end until the next header arrives, so it gives
    /// no event, and neither does a line that has not ended. This behavior
    /// follows from the rule that a header ends a sample. It has no code of its
    /// own, so this test passed when it was written.
    #[test]
    fn an_incomplete_last_sample_gives_no_event() {
        let complete =
            sample("12:00:00", "10     9000000   \n") + &sample("12:00:03", "10     3         \n");
        let partial = "10     5   ";
        let mut parser = StreamParser::new();

        let first = parser.feed(complete.as_bytes());
        let second = parser.feed(partial.as_bytes());

        let header = ParsedEvent::Header { finished: None };
        assert_eq!(first, [header.clone(), header]);
        assert_eq!(second, []);
    }

    /// Gives one sample with the header row `header`.
    fn sample_with_header(time: &str, header: &str, rows: &str) -> String {
        sample(time, rows).replace(HEADER_ROW, header)
    }

    /// Gives the reason of a `Refused` event for `error`: the text of a
    /// `MachineError::TopOutput`.
    fn refused(error: TopParseError) -> ParsedEvent {
        ParsedEvent::Refused {
            reason: MachineError::from(error).to_string(),
        }
    }

    /// The event for the header of a sample that ended with no usable sample.
    fn header_only() -> ParsedEvent {
        ParsedEvent::Header { finished: None }
    }

    /// The event for the header that ends the sample `rows`, with `elapsed`
    /// seconds from the clock line before it.
    fn header_after(rows: Vec<FaultCount>, elapsed: u64) -> ParsedEvent {
        ParsedEvent::Header {
            finished: Some(TopSample {
                rows,
                elapsed: Some(Duration::from_secs(elapsed)),
            }),
        }
    }

    /// A row that is not a PID and a count gives one `Refused` with its line
    /// number, and the next good sample still comes out. A second bad row in the
    /// same sample gives no second `Refused`. The sample before the bad one has
    /// 6 lines, so the bad sample starts at line 7, its first row is line 12, and the bad row
    /// is line 13.
    #[test]
    fn a_bad_row_refuses_its_sample_once_and_the_next_sample_comes_out() {
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", "10     3         \nnot a row\nalso not\n")
            + &sample("12:00:06", "10     5         \n")
            + &sample("12:00:09", "10     7         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                header_only(),
                header_only(),
                refused(TopParseError::MalformedRow {
                    number: 13,
                    line: "not a row".to_owned()
                }),
                header_only(),
                header_after(vec![row(10, 5)], 3),
            ]
        );
    }

    /// A `PID` line of other columns gives one `Refused` with its line number.
    /// The rows under it are not read, and the next good sample comes out.
    #[test]
    fn a_bad_header_row_refuses_its_sample_and_the_next_sample_comes_out() {
        let command = "PID    COMMAND   ";
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample_with_header("12:00:03", command, "10     launchd   \n")
            + &sample("12:00:06", "10     5         \n")
            + &sample("12:00:09", "10     7         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                header_only(),
                header_only(),
                refused(TopParseError::UnexpectedHeader {
                    number: 11,
                    line: command.to_owned()
                }),
                header_only(),
                header_after(vec![row(10, 5)], 3),
            ]
        );
    }

    /// A sample with no rows gives a `Refused` when the next header ends it,
    /// then the `Header` event of that next sample.
    #[test]
    fn a_sample_with_no_rows_is_refused_when_the_next_header_ends_it() {
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", "\n  \n")
            + &sample("12:00:06", "10     5         \n")
            + &sample("12:00:09", "10     7         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                header_only(),
                header_only(),
                refused(TopParseError::NoRows),
                header_only(),
                header_after(vec![row(10, 5)], 3),
            ]
        );
    }

    /// Text before the first `Processes:` line gives one `Refused` with the
    /// first such line, however many lines follow. Blank lines are not text.
    /// The first sample after the text still starts the stream.
    #[test]
    fn text_before_the_first_sample_is_refused_once() {
        let text = String::from("\n日本語 not top\nsecond line\n   \n")
            + &sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", "10     3         \n")
            + &sample("12:00:06", "10     5         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                ParsedEvent::Refused {
                    reason: "the output of /usr/bin/top is not a sample of faults: 日本語 not top"
                        .to_owned()
                },
                header_only(),
                header_only(),
                header_after(vec![row(10, 3)], 3),
            ]
        );
    }
}
