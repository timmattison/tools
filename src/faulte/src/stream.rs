//! The parser of an endless run of `top`.
//!
//! `top -l 0` prints one sample after the other until the caller stops it.
//! [`StreamParser`] turns the bytes of that run into [`ParsedEvent`] values. It
//! holds no clock and no process, so a test feeds it bytes and reads events.
//!
//! A sample ends when the `Processes:` line of the next sample arrives. A timer
//! on an idle pipe would end it sooner, but it would also cut a sample that
//! `top` is slow to write.
//!
//! [`TopStream`] adds the child process and the clock. A reader thread of its
//! own stamps each read of the pipe, because the thread that uses the events
//! can spend seconds in another command, and a stamp that it made late would
//! make a false window.

use std::io::{ErrorKind, Read};
use std::process::{Child, ChildStderr, ChildStdout, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use chrono::NaiveDateTime;

use crate::machine::MachineError;
use crate::top::{self, FaultCount, TopParseError, TopSample};

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
    /// The sample that the next header ends holds no rows. `reason` is the
    /// text of a `MachineError::TopOutput`. The parser finds it when the next
    /// header arrives, but the window of the sample ended at its own header.
    Empty {
        /// Why the parser refused the sample.
        reason: String,
    },
}

/// The byte that ends a line.
const NEWLINE: u8 = b'\n';

/// The longest line that the parser holds, in bytes. A line of `top` is far
/// shorter. A longer one is not the output of `top`, and a parser that holds it
/// would let a broken pipe fill the memory.
const LONGEST_LINE: usize = 64 * 1024;

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
    /// Tells whether the parser refused this sample. A refused sample gives one
    /// `Refused` event and no sample, and the parser reads no more of it.
    refused: bool,
}

impl Current {
    /// Starts a sample at its `Processes:` line.
    fn new() -> Self {
        Self {
            position: Position::ExpectClock,
            clock: None,
            rows: Vec::new(),
            refused: false,
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
    /// The count of lines that ended, so an error names the line of the
    /// stream. The first line is line 1.
    lines: usize,
    /// Tells whether the parser refused the text before the first sample. It
    /// refuses that text once.
    stray_refused: bool,
    /// Tells whether the parser drops the bytes of a line that was too long,
    /// up to the next newline.
    discarding: bool,
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
            if self.discarding {
                // The overlong line ends here. Its refusal went out already.
                self.discarding = false;
                self.lines += 1;
            } else if self.pending.len() + head.len() > LONGEST_LINE {
                self.refuse_long_line(&mut events);
                self.lines += 1;
            } else {
                self.pending.extend_from_slice(head);
                let line = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                self.line(line.strip_suffix('\r').unwrap_or(&line), &mut events);
            }
        }
        if !self.discarding {
            if self.pending.len() + rest.len() > LONGEST_LINE {
                self.refuse_long_line(&mut events);
                self.discarding = true;
            } else {
                self.pending.extend_from_slice(rest);
            }
        }
        events
    }

    /// Refuses a line that is longer than [`LONGEST_LINE`], and drops the bytes
    /// that the parser holds for it. The sample that the line belongs to is
    /// refused too, because the parser lost a row or a header of it.
    fn refuse_long_line(&mut self, events: &mut Vec<ParsedEvent>) {
        self.pending.clear();
        let already_refused = match self.current.as_mut() {
            Some(current) => std::mem::replace(&mut current.refused, true),
            None => std::mem::replace(&mut self.stray_refused, true),
        };
        if !already_refused {
            events.push(ParsedEvent::Refused {
                reason: format!(
                    "the output of {} is not a sample of faults: a line is longer than \
                     {LONGEST_LINE} bytes",
                    top::PROGRAM
                ),
            });
        }
    }

    /// Reads one whole line.
    fn line(&mut self, line: &str, events: &mut Vec<ParsedEvent>) {
        self.lines += 1;
        if line.split_ascii_whitespace().next() == Some(top::PROCESSES) {
            let finished = self.finish(events);
            events.push(ParsedEvent::Header { finished });
            self.current = Some(Current::new());
            return;
        }
        let number = self.lines;
        let Some(current) = self.current.as_mut() else {
            self.refuse_stray(line, events);
            return;
        };
        if current.refused {
            return;
        }
        let error = match current.position {
            Position::ExpectClock => {
                current.clock =
                    NaiveDateTime::parse_from_str(line.trim_ascii(), top::CLOCK_FORMAT).ok();
                current.position = Position::BeforeHeader;
                None
            }
            Position::BeforeHeader => {
                if top::is_header_row(line) {
                    current.position = Position::Rows;
                    (!line.split_ascii_whitespace().eq(top::HEADER)).then(|| {
                        TopParseError::UnexpectedHeader {
                            number,
                            line: line.to_owned(),
                        }
                    })
                } else {
                    None
                }
            }
            Position::Rows => match top::parse_row(line) {
                Some(row) => {
                    current.rows.push(row);
                    None
                }
                None if line.trim_ascii().is_empty() => None,
                None => Some(TopParseError::MalformedRow {
                    number,
                    line: line.to_owned(),
                }),
            },
        };
        if let Some(error) = error {
            current.refused = true;
            events.push(refused(error));
        }
    }

    /// Refuses a line that comes before the first `Processes:` line. A blank
    /// line is not text. Only the first such line gives an event.
    fn refuse_stray(&mut self, line: &str, events: &mut Vec<ParsedEvent>) {
        if self.stray_refused || line.trim_ascii().is_empty() {
            return;
        }
        self.stray_refused = true;
        events.push(ParsedEvent::Refused {
            reason: format!(
                "the output of {} is not a sample of faults: {line}",
                top::PROGRAM
            ),
        });
    }

    /// Ends the sample that the parser reads. Gives the sample when it is
    /// usable. The first sample of a child, a refused sample, and a sample with
    /// no rows give `None`. A sample with no rows also adds an `Empty` event.
    fn finish(&mut self, events: &mut Vec<ParsedEvent>) -> Option<TopSample> {
        let current = self.current.take()?;
        let before = std::mem::replace(&mut self.previous_clock, current.clock);
        let elapsed = elapsed_between(before, current.clock);
        let first = !std::mem::replace(&mut self.finished_one, true);
        if current.refused {
            return None;
        }
        if current.rows.is_empty() {
            events.push(ParsedEvent::Empty {
                reason: MachineError::from(TopParseError::NoRows).to_string(),
            });
            return None;
        }
        if first {
            return None;
        }
        Some(TopSample {
            rows: current.rows,
            elapsed,
        })
    }
}

/// Makes the `Refused` event for `error`. The reason is the text of a
/// `MachineError::TopOutput`, so a person reads the same words as for a run
/// of the single-sample sampler.
fn refused(error: TopParseError) -> ParsedEvent {
    ParsedEvent::Refused {
        reason: MachineError::from(error).to_string(),
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

/// A sample with the times that its window began and ended.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedSample {
    /// The faults of each process over the window.
    pub sample: TopSample,
    /// The time between the arrival of the header before this sample's and of
    /// this sample's. It is the window that the faults were made over.
    pub wall: Duration,
    /// When the header of this sample arrived: the end of its window.
    pub ended_at: SystemTime,
}

/// What a [`FaultStream`] tells the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// The header of a sample arrived at `at`. `finished` is the sample before
    /// it, when that sample is complete and usable.
    Header {
        /// When the reader thread read the header.
        at: SystemTime,
        /// The sample that this header ended.
        finished: Option<TimedSample>,
    },
    /// Output that is not a sample. `reason` is the text of a
    /// `MachineError::TopOutput`.
    Refused {
        /// When the reader thread read the output. For a sample with no rows,
        /// the time that the header of that sample arrived: the end of its
        /// window.
        at: SystemTime,
        /// Why the parser refused the output.
        reason: String,
    },
    /// The source ended. No event follows this one.
    Ended {
        /// When the reader thread saw the end.
        at: SystemTime,
        /// The text of a `MachineError::CommandFailed` with the exit status and
        /// the first line of the error output.
        reason: String,
    },
}

/// A source of [`StreamEvent`] values.
pub trait FaultStream: Send {
    /// Waits up to `wait`. `None` means that no event arrived in that time.
    fn next_event(&mut self, wait: Duration) -> Option<StreamEvent>;

    /// Gives the handle that stops the source of this stream from another
    /// thread.
    fn stopper(&self) -> Stopper;
}

/// A handle that stops the source of a stream from any thread.
#[derive(Clone)]
pub struct Stopper(Arc<dyn Fn() + Send + Sync>);

impl Stopper {
    /// Makes a handle that runs `stop`. `stop` must be safe to run twice.
    pub fn new(stop: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(stop))
    }

    /// Stops the source. A second call does nothing.
    pub fn stop(&self) {
        (self.0)();
    }
}

/// The child process of a [`TopStream`].
///
/// The reader thread and the owner of the stream both reach the child, so the
/// child lives in a shared cell. Whoever takes it out of the cell owns the
/// stop and the status.
#[derive(Debug)]
struct TopChild {
    /// The child, until something takes it to stop it or to read its status.
    child: Mutex<Option<Child>>,
}

impl TopChild {
    /// Takes the child out of the cell. The lock is free again when this
    /// returns, so a caller never holds it across a blocking wait.
    fn take(&self) -> Option<Child> {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Stops the child and reaps it. A second call finds no child and does
    /// nothing. The kill comes after the child left the cell, so the lock is
    /// free while this waits.
    fn stop(&self) {
        if let Some(mut owned) = self.take() {
            let _ = owned.kill();
            let _ = owned.wait();
        }
    }
}

/// How many bytes of the error output the reader keeps.
const STDERR_LIMIT: u64 = 4_096;

/// How many bytes the reader asks the pipe for in one read.
const READ_CHUNK: usize = 8_192;

/// An endless run of a sampler, as a stream of events.
#[derive(Debug)]
pub struct TopStream {
    /// The events that the reader thread sent.
    events: Receiver<StreamEvent>,
    /// The child process.
    child: Arc<TopChild>,
    /// The reader thread.
    reader: Option<JoinHandle<()>>,
}

impl TopStream {
    /// Starts `program` with `arguments` and reads its output.
    ///
    /// The child starts through `childenv::command`, so it sheds the inherited
    /// git environment.
    ///
    /// # Errors
    ///
    /// Gives `MachineError::CommandDidNotStart` when the program does not
    /// start.
    pub fn spawn(program: &str, arguments: &[String]) -> Result<Self, MachineError> {
        let did_not_start = |reason: String| MachineError::CommandDidNotStart {
            program: program.to_owned(),
            reason,
        };
        let mut child = childenv::command(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| did_not_start(error.to_string()))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(did_not_start("the child has no pipe".to_owned()));
        };
        let child = Arc::new(TopChild {
            child: Mutex::new(Some(child)),
        });
        let (sender, events) = mpsc::channel();
        let reader_child = Arc::clone(&child);
        let reader_program = program.to_owned();
        let spawned = thread::Builder::new()
            .name("faulte-top-reader".to_owned())
            .spawn(move || read_stream(stdout, stderr, &reader_child, &reader_program, &sender));
        match spawned {
            Ok(reader) => Ok(Self {
                events,
                child,
                reader: Some(reader),
            }),
            Err(error) => {
                if let Some(mut orphan) = child.take() {
                    let _ = orphan.kill();
                    let _ = orphan.wait();
                }
                Err(did_not_start(error.to_string()))
            }
        }
    }
}

impl Drop for TopStream {
    /// Stops the child, reaps it, and joins the reader thread. The thread ends
    /// when the pipe closes, and a killed child closes it.
    fn drop(&mut self) {
        self.child.stop();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl FaultStream for TopStream {
    fn next_event(&mut self, wait: Duration) -> Option<StreamEvent> {
        self.events.recv_timeout(wait).ok()
    }

    fn stopper(&self) -> Stopper {
        let child = Arc::clone(&self.child);
        Stopper::new(move || child.stop())
    }
}

/// When a chunk of output arrived, on both clocks.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    /// The monotonic clock. It measures a window, and it never moves back.
    monotonic: Instant,
    /// The wall clock. It names the moment for a person.
    wall: SystemTime,
}

impl Stamp {
    /// Reads both clocks now.
    fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now(),
        }
    }
}

/// The stamps of the last two headers.
///
/// A sample ends at its own header and begins at the header before it, so the
/// window of a sample needs both stamps.
#[derive(Debug, Default)]
struct HeaderStamps {
    /// The header before the one that arrived last.
    before_previous: Option<Stamp>,
    /// The header that arrived last.
    previous: Option<Stamp>,
}

impl HeaderStamps {
    /// Turns a header that arrived at `at` into an event, and keeps its stamp.
    ///
    /// `finished` is the sample that this header ended. That sample began at
    /// the header before `previous` and ended at `previous`.
    fn header(&mut self, at: Stamp, finished: Option<TopSample>) -> StreamEvent {
        let timed = finished.map(|sample| TimedSample {
            sample,
            wall: match (self.before_previous, self.previous) {
                (Some(began), Some(ended)) => {
                    ended.monotonic.saturating_duration_since(began.monotonic)
                }
                _ => Duration::ZERO,
            },
            ended_at: self.previous.map_or(at.wall, |ended| ended.wall),
        });
        self.before_previous = self.previous;
        self.previous = Some(at);
        StreamEvent::Header {
            at: at.wall,
            finished: timed,
        }
    }

    /// Gives the wall time of the header that arrived last, if any.
    fn last_wall(&self) -> Option<SystemTime> {
        self.previous.map(|stamp| stamp.wall)
    }
}

/// The status that an end states when the owner stopped the child, so nobody
/// read the status of the process.
const STOPPED_STATUS: &str = "stopped by faulte";

/// How many times the reader asks for the status of a child that closed its
/// output, before it stops the child.
const STATUS_TRIES: u32 = 10;

/// The pause between two of those questions.
const STATUS_PAUSE: Duration = Duration::from_millis(50);

/// The status that an end states when the operating system gives none.
const UNKNOWN_STATUS: &str = "an unknown status";

/// Reads the output of the child until it ends, and sends the events.
///
/// This is the body of the reader thread. It stops early when the owner of
/// the stream dropped the receiver. At the end it sends one
/// [`StreamEvent::Ended`].
fn read_stream(
    mut stdout: ChildStdout,
    stderr: ChildStderr,
    child: &TopChild,
    program: &str,
    sender: &Sender<StreamEvent>,
) {
    let mut parser = StreamParser::new();
    let mut stamps = HeaderStamps::default();
    let mut buffer = [0_u8; READ_CHUNK];
    loop {
        let count = match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let at = Stamp::now();
        let chunk = buffer.get(..count).unwrap_or_default();
        for event in parser.feed(chunk) {
            let event = match event {
                ParsedEvent::Header { finished } => stamps.header(at, finished),
                ParsedEvent::Refused { reason } => StreamEvent::Refused {
                    at: at.wall,
                    reason,
                },
                ParsedEvent::Empty { reason } => StreamEvent::Refused {
                    at: stamps.last_wall().unwrap_or(at.wall),
                    reason,
                },
            };
            if sender.send(event).is_err() {
                return;
            }
        }
    }
    let status = match child.take() {
        Some(mut owned) => status_of(&mut owned),
        None => STOPPED_STATUS.to_owned(),
    };
    let reason = MachineError::CommandFailed {
        program: program.to_owned(),
        status,
        message: first_line(&read_limited(stderr)),
    }
    .to_string();
    // The owner may have dropped the receiver already, and then nobody reads
    // the end.
    let _ = sender.send(StreamEvent::Ended {
        at: SystemTime::now(),
        reason,
    });
}

/// Reads up to [`STDERR_LIMIT`] bytes of `source`.
fn read_limited(source: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    // A failed read leaves the bytes that arrived, and those are the message.
    let _ = source.take(STDERR_LIMIT).read_to_end(&mut bytes);
    bytes
}

/// Gives the status of a child whose output closed.
///
/// A child that closed its output is about to exit, so this asks a few times.
/// A child that is still alive after that is stopped, because a sampler that
/// holds no output has nothing more to give.
fn status_of(child: &mut Child) -> String {
    for _ in 0..STATUS_TRIES {
        match child.try_wait() {
            Ok(Some(status)) => return status.to_string(),
            Ok(None) => thread::sleep(STATUS_PAUSE),
            Err(_) => return UNKNOWN_STATUS.to_owned(),
        }
    }
    let _ = child.kill();
    child
        .wait()
        .map_or_else(|_| UNKNOWN_STATUS.to_owned(), |status| status.to_string())
}

/// The text that an error states when a command wrote nothing to explain
/// itself.
const NO_MESSAGE: &str = "the command wrote no message";

/// Gives the first line that a command wrote, for an error message.
///
/// One line is enough to name the fault, and a command that failed can write
/// many lines. The bytes are read without a check of UTF-8, because a message
/// that `faulte` cannot read is still a message that a person can.
pub(crate) fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    line.unwrap_or(NO_MESSAGE).to_owned()
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

    /// Gives the three samples of the tests above, one text for each sample.
    fn three_sample_texts() -> [String; 3] {
        [
            sample("12:00:00", "10     9000000   \n"),
            sample("12:00:03", "10     3         \n20     4000      \n"),
            sample("12:00:06", "10     5         \n"),
        ]
    }

    /// Gives the three samples of the tests above as one text.
    fn three_samples() -> String {
        three_sample_texts().concat()
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

    /// A sample with no rows gives an `Empty` when the next header ends it,
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
                ParsedEvent::Empty {
                    reason: MachineError::from(TopParseError::NoRows).to_string(),
                },
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

    /// A line longer than `LONGEST_LINE` gives one `Refused`, and the parser
    /// drops its bytes up to the next newline. It does not hold them. The
    /// events are the same when the line arrives in one read, and when it
    /// arrives in many. The sample that the line belongs to is refused, so no
    /// row is lost without a word.
    #[test]
    fn a_line_longer_than_the_limit_is_refused_and_dropped() {
        let long = "x".repeat(LONGEST_LINE + 1);
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", &format!("10     3         \n{long}\n"))
            + &sample("12:00:06", "10     5         \n")
            + &sample("12:00:09", "10     7         \n");
        let expected = [
            header_only(),
            header_only(),
            ParsedEvent::Refused {
                reason: "the output of /usr/bin/top is not a sample of faults: a line is longer \
                         than 65536 bytes"
                    .to_owned(),
            },
            header_only(),
            header_after(vec![row(10, 5)], 3),
        ];

        assert_eq!(events_of(&text), expected, "all in one read");

        let mut parser = StreamParser::new();
        let mut events = Vec::new();
        for chunk in text.as_bytes().chunks(1_000) {
            events.extend(parser.feed(chunk));
            assert!(
                parser.pending.len() <= LONGEST_LINE,
                "the parser holds at most one line, not the whole long line"
            );
        }
        assert_eq!(events, expected, "in reads of 1,000 bytes");
    }

    /// An overlong line at the limit exactly is a line. One byte more is not.
    #[test]
    fn a_line_of_exactly_the_limit_is_still_a_line() {
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample(
                "12:00:03",
                &format!("10     3{}\n", " ".repeat(LONGEST_LINE - 8)),
            )
            + &sample("12:00:06", "10     5         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                header_only(),
                header_only(),
                header_after(vec![row(10, 3)], 3)
            ]
        );
    }

    /// Gives the `elapsed` of the one sample that a stream of three clock lines
    /// finishes. The sample is the second one, and its clock line is `second`.
    fn elapsed_of(first: &str, second: &str, third: &str) -> Option<Duration> {
        let text = sample_at(first, "10     9000000   \n")
            + &sample_at(second, "10     3         \n")
            + &sample_at(third, "10     5         \n");

        match events_of(&text).pop() {
            Some(ParsedEvent::Header {
                finished: Some(sample),
            }) => sample.elapsed,
            other => panic!("the stream did not finish a sample: {other:?}"),
        }
    }

    /// The elapsed time of a sample is its clock line minus the clock line of
    /// the sample before it. It holds across midnight, and the spaces after a
    /// clock line are not part of the time. This test passed when it was
    /// written, because the first green commit of the parser stored the clock
    /// lines already.
    #[test]
    fn the_elapsed_time_is_the_difference_of_two_clock_lines() {
        let cases = [
            ("2026/09/18 12:00:00", "2026/09/18 12:00:02", 2),
            ("2026/09/18 23:59:59", "2026/09/19 00:00:03", 4),
            ("2026/12/31 23:59:58  ", "2027/01/01 00:00:08 ", 10),
        ];
        for (before, after, seconds) in cases {
            assert_eq!(
                elapsed_of(before, after, "2030/01/01 00:00:00"),
                Some(Duration::from_secs(seconds)),
                "the clock lines {before:?} and {after:?}"
            );
        }
    }

    /// A clock line that is bad, or a difference of zero or less, gives no
    /// elapsed time, and no error. The sample still comes out with its rows.
    #[test]
    fn a_bad_backward_or_equal_clock_gives_no_elapsed_time() {
        let good = "2026/09/18 12:00:02";
        let cases = [
            ("2026/09/18 12:00:05", good),
            (good, good),
            ("not a time", good),
            (good, "not a time"),
            (good, ""),
            ("2026/09/18 12:00", good),
            ("2026/09/18 25:00:00", good),
            ("２０２６/09/18 12:00:00", good),
        ];
        for (before, after) in cases {
            assert_eq!(
                elapsed_of(before, after, "2030/01/01 00:00:00"),
                None,
                "the clock lines {before:?} and {after:?}"
            );
        }
    }

    /// The clock line of the sample before is the one that counts, not the
    /// clock line of the first sample. A bad clock line in one sample spoils
    /// that sample and the next one only.
    #[test]
    fn the_elapsed_time_uses_the_sample_just_before() {
        let text = sample_at("2026/09/18 12:00:00", "10     9000000   \n")
            + &sample_at("2026/09/18 12:00:10", "10     3         \n")
            + &sample_at("not a time", "10     5         \n")
            + &sample_at("2026/09/18 12:00:20", "10     7         \n")
            + &sample_at("2026/09/18 12:00:23", "10     9         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                header_only(),
                header_only(),
                header_after(vec![row(10, 3)], 10),
                ParsedEvent::Header {
                    finished: Some(TopSample {
                        rows: vec![row(10, 5)],
                        elapsed: None
                    })
                },
                ParsedEvent::Header {
                    finished: Some(TopSample {
                        rows: vec![row(10, 7)],
                        elapsed: None
                    })
                },
            ]
        );
    }

    /// The output of `top -l 3 -s 3 -c d -n 16000 -stats pid,faults` on this
    /// Mac on 2026-11-08, byte for byte. It holds three samples, and the third
    /// one is cut short, as the end of a stream is.
    const REAL_CAPTURE: &str = include_str!("../tests/fixtures/top-l3-cd.txt");

    /// The real capture finishes one sample, the second. Its header line says
    /// `Processes: 1287 total`, and the sample holds 1,287 rows. The clock
    /// lines are 3 s apart. The sum of the faults is the sum of the lines that
    /// follow the second `PID` line, up to the third `Processes:` line. A scan
    /// of every line that starts with a digit counts one line too many, because
    /// the clock line of the third sample starts with a digit.
    #[test]
    fn the_real_capture_gives_its_second_sample() {
        let events = events_of(REAL_CAPTURE);

        let [first, second, third] = events.as_slice() else {
            panic!("three headers expected, got {events:?}");
        };
        assert_eq!(first, &header_only());
        assert_eq!(second, &header_only());
        let ParsedEvent::Header {
            finished: Some(sample),
        } = third
        else {
            panic!("the third header finishes the second sample: {third:?}");
        };
        assert_eq!(sample.rows.len(), 1_287);
        assert_eq!(sample.rows.first(), Some(&row(7_013, 15_069)));
        assert_eq!(sample.rows.last(), Some(&row(210, 0)));
        assert_eq!(
            sample.rows.iter().map(|count| count.faults).sum::<u64>(),
            432_130,
            "the sum of the second sample, not the counts since start of the first"
        );
        assert_eq!(sample.elapsed, Some(Duration::from_secs(3)));
    }

    /// The real capture in reads of 4,096 bytes, as a pipe gives it, gives the
    /// same events as the capture in one read.
    #[test]
    fn the_real_capture_in_pipe_sized_reads_gives_the_same_events() {
        let mut parser = StreamParser::new();
        let events: Vec<ParsedEvent> = REAL_CAPTURE
            .as_bytes()
            .chunks(4_096)
            .flat_map(|chunk| parser.feed(chunk))
            .collect();

        assert_eq!(events, events_of(REAL_CAPTURE));
    }

    /// The longest that a test waits for the child. The wait ends as soon as
    /// the condition is true, so the bound only matters on a loaded machine.
    const BOUND: Duration = Duration::from_secs(60);

    /// The time between two checks while a test waits for an event.
    const POLL: Duration = Duration::from_millis(100);

    /// The pause after an answer of no event.
    const PAUSE: Duration = Duration::from_millis(10);

    /// The text that makes a stub live for at most 60 seconds. Every stub
    /// carries it, so a stub cannot outlive a test run that was killed.
    const STUB_LIFETIME: &str = "i=0; while [ $i -lt 600 ]; do i=$((i+1)); sleep 0.1; done";

    /// Gives the arguments of `/bin/sh` that run `script`.
    fn shell(script: &str) -> Vec<String> {
        vec!["-c".to_owned(), script.to_owned()]
    }

    /// Starts a stub that runs `before`, prints `text`, and then lives on until
    /// its time limit.
    fn stub_printing(before: &str, text: &str) -> TopStream {
        let text = shellquote::shell_quote(text);
        let script = format!("{before} printf '%s' {text}; {STUB_LIFETIME}");
        TopStream::spawn("/bin/sh", &shell(&script)).expect("the shell starts")
    }

    /// Reads events until `done` is true for the events so far. Fails the test
    /// when the bound passes first.
    fn events_until(
        stream: &mut TopStream,
        done: impl Fn(&[StreamEvent]) -> bool,
    ) -> Vec<StreamEvent> {
        let deadline = Instant::now() + BOUND;
        let mut events = Vec::new();
        while !done(&events) {
            assert!(
                Instant::now() < deadline,
                "no such events in time: {events:?}"
            );
            match stream.next_event(POLL) {
                Some(event) => events.push(event),
                // A stream that is closed answers at once, so the pause keeps
                // the wait from spinning.
                None => std::thread::sleep(PAUSE),
            }
        }
        events
    }

    /// Tells whether `events` hold at least `count` headers.
    fn headers_at_least(count: usize) -> impl Fn(&[StreamEvent]) -> bool {
        move |events| {
            events
                .iter()
                .filter(|event| matches!(event, StreamEvent::Header { .. }))
                .count()
                >= count
        }
    }

    /// Gives the headers among `events`, as the sample that each one finished.
    fn finished_of(events: &[StreamEvent]) -> Vec<Option<TimedSample>> {
        events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Header { finished, .. } => Some(finished.clone()),
                _ => None,
            })
            .collect()
    }

    /// Three samples from a child give three headers in order. The first
    /// sample holds the counter since each process started, so the first two
    /// headers finish nothing. The third header finishes the second sample.
    #[test]
    fn a_child_that_prints_three_samples_gives_three_headers_in_order() {
        let mut stream = stub_printing("", &three_samples());

        let events = events_until(&mut stream, headers_at_least(3));

        let finished = finished_of(&events);
        assert_eq!(finished.len(), 3);
        assert_eq!(finished.first(), Some(&None));
        assert_eq!(finished.get(1), Some(&None));
        let second = finished
            .get(2)
            .cloned()
            .flatten()
            .expect("a finished sample");
        assert_eq!(second.sample.rows, vec![row(10, 3), row(20, 4000)]);
    }

    /// The largest difference between the window of a sample, which the
    /// monotonic clock measures, and the wall times of the two headers that
    /// bound it. The reader reads both clocks at once for each header, so the
    /// two differ only by a preemption between the two reads and by a slew of
    /// the wall clock.
    const CLOCK_TOLERANCE: Duration = Duration::from_millis(50);

    /// The sample that a header finishes began at the header before the last
    /// one, and it ended at the last one. The stub sleeps 0.3 s before the
    /// second sample and 0.1 s before the third. Thus a window of zero, a
    /// window between the second and the third header, and an end at the
    /// third header each fail. The test compares the window with the wall
    /// times of its own headers and not with the sleeps, so a reader thread
    /// that wakes late under load does not fail it.
    #[test]
    fn a_finished_sample_states_its_window_and_the_time_that_it_ended() {
        let [first, second, third] = three_sample_texts();
        let before = format!(
            "printf '%s' {}; sleep 0.3; printf '%s' {}; sleep 0.1;",
            shellquote::shell_quote(&first),
            shellquote::shell_quote(&second),
        );
        let mut stream = stub_printing(&before, &third);

        let events = events_until(&mut stream, headers_at_least(3));

        let headers: Vec<SystemTime> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Header { at, .. } => Some(*at),
                _ => None,
            })
            .collect();
        let (Some(first_at), Some(second_at)) = (headers.first(), headers.get(1)) else {
            panic!("three headers: {events:?}");
        };
        let Some(Some(timed)) = finished_of(&events).get(2).cloned() else {
            panic!("the third header finishes the second sample: {events:?}");
        };
        assert_eq!(
            timed.ended_at, *second_at,
            "the sample ended at the second header: {events:?}"
        );
        let between = second_at.duration_since(*first_at).unwrap_or_default();
        assert!(
            timed.wall.abs_diff(between) <= CLOCK_TOLERANCE,
            "the window {:?} is the time between the first two headers, {between:?}",
            timed.wall
        );
    }

    /// A sample with no rows is refused at the end of its own window: the
    /// time that its own header arrived, not the time of the header that
    /// ended it. A gap that ends there covers the one sample and no more.
    #[test]
    fn a_sample_with_no_rows_is_refused_at_the_time_of_its_own_header() {
        let early = sample("12:00:00", "10     9000000   \n") + &sample("12:00:03", "\n");
        let late = sample("12:00:06", "10     5         \n");
        let before = format!(
            "printf '%s' {}; sleep 0.3;",
            shellquote::shell_quote(&early)
        );
        let mut stream = stub_printing(&before, &late);

        let events = events_until(&mut stream, headers_at_least(3));

        let headers: Vec<SystemTime> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Header { at, .. } => Some(*at),
                _ => None,
            })
            .collect();
        let refusals: Vec<SystemTime> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Refused { at, .. } => Some(*at),
                _ => None,
            })
            .collect();
        assert_eq!(
            refusals,
            headers.get(1..2).unwrap_or_default(),
            "{events:?}"
        );
    }

    /// Gives a path in the temporary directory that no other run uses.
    fn unique_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        std::env::temp_dir().join(format!(
            "faulte-stream-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    /// Tells whether the process `pid` exists, as `ps` sees it.
    fn process_exists(pid: u32) -> bool {
        childenv::fixture_command("/bin/ps")
            .args(["-p", &pid.to_string()])
            .output()
            .expect("ps runs")
            .status
            .success()
    }

    /// Starts a stub that writes its own PID to a file before it prints its
    /// samples, waits for the first header, and gives the stream and the PID.
    fn stub_with_pid() -> (TopStream, u32) {
        let path = unique_path("pid");
        let before = format!(
            "echo $$ > {};",
            shellquote::shell_quote(&path.display().to_string())
        );
        let mut stream = stub_printing(&before, &three_samples());
        events_until(&mut stream, headers_at_least(1));
        let text = std::fs::read_to_string(&path).expect("the stub wrote its PID");
        std::fs::remove_file(&path).expect("the file is there");
        (stream, text.trim().parse().expect("the PID is a number"))
    }

    /// A program that does not exist is a command that did not start.
    #[test]
    fn a_program_that_does_not_exist_did_not_start() {
        let missing = "/nonexistent/faulte-top";

        let error = TopStream::spawn(missing, &[]).expect_err("the program is not there");

        assert!(
            matches!(&error, MachineError::CommandDidNotStart { program, .. } if program == missing),
            "{error:?}"
        );
    }

    /// A child that exits with a status that is not success, and wrote a line
    /// on its error output, gives one end event. The event names the status
    /// and the line, and no event follows it.
    #[test]
    fn a_child_that_fails_gives_an_end_with_its_status_and_its_message() {
        let script = "echo 'boom: bad flag' >&2; echo second line >&2; exit 3";
        let mut stream = TopStream::spawn("/bin/sh", &shell(script)).expect("the shell starts");

        let events = events_until(&mut stream, |events| {
            matches!(events.last(), Some(StreamEvent::Ended { .. }))
        });

        let [StreamEvent::Ended { reason, .. }] = events.as_slice() else {
            panic!("one end event and nothing else: {events:?}");
        };
        assert!(reason.contains("exit status: 3"), "{reason}");
        assert!(reason.contains("boom: bad flag"), "{reason}");
        assert!(!reason.contains("second line"), "{reason}");
        assert!(reason.contains("/bin/sh"), "{reason}");
        assert_eq!(stream.next_event(POLL), None, "nothing follows the end");
    }

    /// A child that never ends stops when its stream is dropped. The drop
    /// reaps the child, so `ps` finds no such process afterwards.
    #[test]
    fn dropping_the_stream_stops_the_child() {
        let (stream, pid) = stub_with_pid();
        assert!(process_exists(pid), "the child runs before the drop");

        drop(stream);

        assert!(
            !process_exists(pid),
            "the child {pid} is gone after the drop"
        );
    }

    /// The handle stops the child from another thread, and the stream then
    /// gives its end. A second stop does nothing. The handle outlives the
    /// stream, and a stop through it after the drop does nothing too.
    #[test]
    fn the_stopper_stops_the_child_from_another_thread() {
        let (mut stream, pid) = stub_with_pid();
        let stopper = stream.stopper();
        let remote = stopper.clone();

        std::thread::spawn(move || {
            remote.stop();
            remote.stop();
        })
        .join()
        .expect("the stop does not panic");

        let events = events_until(&mut stream, |events| {
            matches!(events.last(), Some(StreamEvent::Ended { .. }))
        });
        assert!(matches!(events.last(), Some(StreamEvent::Ended { .. })));
        assert!(
            !process_exists(pid),
            "the child {pid} is gone after the stop"
        );
        stopper.stop();
        drop(stream);
        stopper.stop();
    }
}
