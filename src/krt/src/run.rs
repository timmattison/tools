//! The run loop: the record that opens a run, one record for each round, the
//! record that closes it, and one status line for each round.
//!
//! The tracer of `trace.rs` carries no limit of its own. It sends one round
//! after another until the process ends, and this module owns the number of
//! rounds and the moment that stop a run. A closed channel is therefore the one
//! signal of a tracer thread that died, and the loop reads it as a fault.
//!
//! The design puts this loop in `main.rs`. It lives here because `main.rs`
//! already carries the whole command line, and a loop that a test drives needs
//! its own door.
//!
//! Every record goes through the writer, and a failed write stops the run. The
//! recording is the whole purpose of the tool, and a run that keeps a display
//! while it silently records nothing is worse than a run that stops.

use crate::record::{EndReason, EndRecord, Record, RoundRecord, RunId, RunRecord, Writer};
use crate::ui::render_duration;
use crate::{counted, HOP, NEVER_REACHED, REACHED, ROUND, SUMMARY_SEPARATOR};
use chrono::Utc;
use std::io::Write;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// The longest wait for one round.
///
/// The loop still sees the stop flag and the deadline when no round arrives,
/// because the wait ends after this time and the loop takes another turn.
const POLL: Duration = Duration::from_millis(100);

/// The limits that stop a run.
#[derive(Debug)]
pub(crate) struct Limits {
    /// The number of rounds that stops the run. No number runs until the user
    /// stops it.
    pub(crate) rounds: Option<u64>,
    /// The moment that stops the run. No moment runs until the user stops it.
    pub(crate) deadline: Option<Instant>,
}

/// What a run produced.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// The number of rounds that the run recorded.
    pub(crate) rounds: u64,
    /// Why the run stopped.
    pub(crate) reason: EndReason,
}

/// The fault that stopped a run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RunError {
    /// A write to the recorded file failed.
    #[error("the recorded file did not take a record: {0}")]
    Write(
        /// The fault that the operating system reported.
        std::io::Error,
    ),
    /// The tracer thread stopped before a limit did.
    #[error("the tracer stopped after {rounds} rounds")]
    Tracer {
        /// The number of rounds that the run recorded.
        rounds: u64,
    },
}

/// Records one run: the record that opens it, one record for each round, and
/// the record that closes it.
///
/// `stop` answers whether the user asked the run to stop. The loop asks it once
/// at the top of each turn, before it reads a round, so a run that the user
/// stops records no further round.
///
/// The `run` record goes first, and a fault there stops the run before it reads
/// anything. Each round that arrives becomes one `round` record. The `end`
/// record names the number of rounds that the run recorded and why it stopped.
///
/// Each round that the run records also prints one status line to `status`.
///
/// A failed write of any record stops the run. The recording is the whole
/// purpose of the tool, and a run that keeps a display while it silently
/// records nothing is worse than a run that stops.
///
/// # Errors
///
/// Returns [`RunError::Write`] when a record does not reach the file, and
/// [`RunError::Tracer`] when the tracer thread stops before a limit does.
pub(crate) fn record<W: Write, S: Write>(
    start: &RunRecord,
    rounds: &Receiver<RoundRecord>,
    limits: &Limits,
    stop: &dyn Fn() -> bool,
    writer: &mut Writer<W>,
    status: &mut S,
) -> Result<Outcome, RunError> {
    writer
        .write(&Record::Run(start.clone()))
        .map_err(RunError::Write)?;
    let mut recorded: u64 = 0;
    loop {
        if let Some(reason) = stopped(recorded, limits, stop) {
            close(writer, &start.run, recorded, reason)?;
            return Ok(Outcome {
                rounds: recorded,
                reason,
            });
        }
        match rounds.recv_timeout(wait(limits.deadline)) {
            Ok(round) => {
                let line = status_line(&round);
                writer
                    .write(&Record::Round(round))
                    .map_err(RunError::Write)?;
                // A line that does not print stops nothing. The recording is
                // the purpose of the tool, and the line is one view of it, so a
                // reader who closes the pipe of the display loses the display
                // and keeps the recording.
                drop(writeln!(status, "{line}"));
                recorded += 1;
            }
            // No round arrived inside the wait. The loop takes another turn, so
            // it reads the stop flag and the deadline again.
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // The tracer holds the sender for as long as it lives, so a
                // closed channel names a tracer thread that died.
                //
                // The `?` gives the write fault the last word. A run whose
                // `end` record will not write reports that fault and not the
                // dead tracer, because a file that takes no record names the
                // fault that stops the tool from doing its job at all.
                close(writer, &start.run, recorded, EndReason::Error)?;
                return Err(RunError::Tracer { rounds: recorded });
            }
        }
    }
}

/// Writes the one line that a run prints for one round.
///
/// The line holds the number of the round, the number of hops that answered,
/// whether the round reached the target, and the time that the round took. Two
/// spaces separate the fields, as they do in the closing line of the run.
///
/// A hop that did not answer is absent from the record, so the count is the
/// number of hops that answered and not the length of the path.
///
/// A later slice replaces this line with the live table.
fn status_line(round: &RoundRecord) -> String {
    let reached = if round.reached {
        REACHED
    } else {
        NEVER_REACHED
    };
    [
        format!("{ROUND} {}", round.seq),
        counted(round.hops.len(), HOP),
        reached.to_owned(),
        render_duration(Duration::from_millis(round.dur_ms)),
    ]
    .join(SUMMARY_SEPARATOR)
}

/// Why the run stops at the top of this turn. A run that goes on stops for
/// nothing.
///
/// The user comes first, then the number of rounds, then the moment.
fn stopped(recorded: u64, limits: &Limits, stop: &dyn Fn() -> bool) -> Option<EndReason> {
    if stop() {
        return Some(EndReason::Quit);
    }
    if limits.rounds.is_some_and(|limit| recorded >= limit) {
        return Some(EndReason::Rounds);
    }
    if limits
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return Some(EndReason::Duration);
    }
    None
}

/// The longest wait for the next round.
///
/// The wait ends at the deadline when the deadline stands nearer than [`POLL`],
/// so a run stops at the moment of its time limit and not one poll after it.
fn wait(deadline: Option<Instant>) -> Duration {
    deadline.map_or(POLL, |deadline| {
        deadline.saturating_duration_since(Instant::now()).min(POLL)
    })
}

/// Writes the record that closes a run.
///
/// # Errors
///
/// Returns [`RunError::Write`] when the record does not reach the file.
fn close<W: Write>(
    writer: &mut Writer<W>,
    run: &RunId,
    rounds: u64,
    reason: EndReason,
) -> Result<(), RunError> {
    writer
        .write(&Record::End(EndRecord {
            run: run.clone(),
            ts: Utc::now(),
            rounds,
            reason,
        }))
        .map_err(RunError::Write)
}

#[cfg(test)]
mod tests {
    use super::{record, Limits, Outcome, RunError};
    use crate::record::{
        EndReason, EndRecord, Family, Hop, Privilege, Record, Recording, RoundRecord, RunConfig,
        RunId, RunRecord, SourceKind, SourceLabel, Target, TtlRange, Writer,
    };
    use crate::testing::address;
    use crate::{Multipath, Protocol};
    use chrono::{DateTime, Utc};
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    /// The identifier of the run that every test record belongs to.
    const RUN: &str = "2026-08-18T12:00:00.123Z";

    /// The moment that every test round starts.
    const ROUND_START: &str = "2026-08-18T12:34:56.789Z";

    /// The address that the probes of a test run leave from.
    const SOURCE_ADDRESS: &str = "1.2.3.4";

    /// The address of the first hop of a test round.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The address of the target of a test run.
    const TARGET_ADDRESS: &str = "93.184.216.34";

    /// The destination of a test run, as the user typed it.
    const TARGET_ARG: &str = "example.com";

    /// The name of the machine that made a test run.
    const HOST: &str = "tims-mac";

    /// The build string of the `krt` that made a test run.
    const KRT: &str = "0.1.0 (abc1234, clean)";

    /// The period of one round of a test run, in milliseconds.
    const INTERVAL_MS: u64 = 1000;

    /// The first TTL that a test run probes.
    const FIRST_TTL: u8 = 1;

    /// The last TTL that a test run probes.
    const MAX_TTL: u8 = 30;

    /// The TTL of the target of a test round.
    const TARGET_TTL: u8 = 14;

    /// The round trip time of the first hop of a test round, in milliseconds.
    const FIRST_HOP_RTT_MS: f64 = 1.23;

    /// The round trip time of the target of a test round, in milliseconds.
    const TARGET_RTT_MS: f64 = 24.1;

    /// The name that the schema records when a hop below the target answered
    /// that the TTL of the probe ran out.
    const TIME_EXCEEDED: &str = "time_exceeded";

    /// The name that the schema records when the target answered the echo
    /// request.
    const ECHO_REPLY: &str = "echo_reply";

    /// The time that a round of the whole path takes, in milliseconds.
    const WHOLE_PATH_MS: u64 = 1000;

    /// The time that a round of one answer takes, in milliseconds.
    const LOST_ROUND_MS: u64 = 1004;

    /// The status line of the first round of a run that answered the whole path.
    const A_WHOLE_PATH_LINE: &str = "round 1  2 hops  reached  1s";

    /// The number of the round that answered one hop.
    const LOST_ROUND_SEQ: u64 = 2;

    /// The status line of a round that answered one hop and reached nothing.
    const A_LOST_ROUND_LINE: &str = "round 2  1 hop  never reached  1004ms";

    /// The limits of a run that stops on nothing.
    const NO_LIMIT: Limits = Limits {
        rounds: None,
        deadline: None,
    };

    /// The number of turns that the run of one test takes before the user stops
    /// it.
    ///
    /// The loop asks the stop closure once at the top of each turn, and each
    /// turn of that test records one round, so the third question follows the
    /// second round.
    const STOP_AFTER: u64 = 2;

    /// The fault of a sink that takes no more records.
    const THE_SINK_IS_FULL: &str = "the sink takes no more records";

    /// The byte that ends one record.
    const NEWLINE: u8 = b'\n';

    /// Reads a moment that a test names, and converts it to UTC.
    fn moment(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the test moment must parse")
            .with_timezone(&Utc)
    }

    /// The record that opens the run of every test.
    fn a_run_record() -> RunRecord {
        RunRecord {
            run: RunId::from(RUN),
            krt: KRT.to_owned(),
            source: SourceLabel {
                addr: address(SOURCE_ADDRESS),
                kind: SourceKind::Public,
            },
            target: Target {
                arg: TARGET_ARG.to_owned(),
                addr: address(TARGET_ADDRESS),
                family: Family::Ipv4,
            },
            config: RunConfig {
                interval_ms: INTERVAL_MS,
                protocol: Protocol::Icmp,
                first_ttl: FIRST_TTL,
                max_ttl: MAX_TTL,
                multipath: Multipath::Classic,
                privilege: Privilege::Unprivileged,
                dns: true,
            },
            host: HOST.to_owned(),
        }
    }

    /// One round that answered the whole path and reached the target.
    fn a_round(seq: u64) -> RoundRecord {
        RoundRecord {
            run: RunId::from(RUN),
            seq,
            ts: moment(ROUND_START),
            dur_ms: WHOLE_PATH_MS,
            ttl_range: TtlRange::new(FIRST_TTL, TARGET_TTL).expect("the test range must hold"),
            reached: true,
            hops: vec![
                Hop {
                    ttl: FIRST_TTL,
                    addr: address(FIRST_HOP),
                    rtt_ms: FIRST_HOP_RTT_MS,
                    icmp: TIME_EXCEEDED.to_owned(),
                },
                Hop {
                    ttl: TARGET_TTL,
                    addr: address(TARGET_ADDRESS),
                    rtt_ms: TARGET_RTT_MS,
                    icmp: ECHO_REPLY.to_owned(),
                },
            ],
        }
    }

    /// One round that answered one hop and reached nothing.
    fn a_lost_round(seq: u64) -> RoundRecord {
        RoundRecord {
            run: RunId::from(RUN),
            seq,
            ts: moment(ROUND_START),
            dur_ms: LOST_ROUND_MS,
            ttl_range: TtlRange::new(FIRST_TTL, TARGET_TTL).expect("the test range must hold"),
            reached: false,
            hops: vec![Hop {
                ttl: FIRST_TTL,
                addr: address(FIRST_HOP),
                rtt_ms: FIRST_HOP_RTT_MS,
                icmp: TIME_EXCEEDED.to_owned(),
            }],
        }
    }

    /// The rounds of a test, in the order the tracer sends them.
    fn rounds_of(seqs: &[u64]) -> Vec<RoundRecord> {
        seqs.iter().copied().map(a_round).collect()
    }

    /// A channel that holds these rounds and that no sender keeps open.
    ///
    /// The tracer sends from its own thread. A test needs no thread: it sends
    /// every round first and drops the sender, so the loop reads what the
    /// channel holds and then reads a closed channel.
    fn a_stream(rounds: &[RoundRecord]) -> Receiver<RoundRecord> {
        let (sender, receiver) = mpsc::channel();
        for round in rounds {
            sender
                .send(round.clone())
                .expect("the receiver of the test must stand");
        }
        receiver
    }

    /// The limits of a run that stops after this many rounds.
    fn after_rounds(rounds: u64) -> Limits {
        Limits {
            rounds: Some(rounds),
            deadline: None,
        }
    }

    /// The limits of a run whose moment already passed.
    fn a_past_deadline() -> Limits {
        Limits {
            rounds: None,
            deadline: Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .expect("the clock must stand one second after the start of the process"),
            ),
        }
    }

    /// Builds a path under the temporary directory that no other run reaches.
    ///
    /// Two runs of one test can overlap, because `cargo test` runs on many
    /// threads and more than one `cargo test` can run at once. The process
    /// identifier and the nanosecond keep the two runs apart.
    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must stand after the epoch")
            .as_nanos();
        let process = std::process::id();
        std::env::temp_dir().join(format!("krt-run-{label}-{process}-{nanos}.jsonl"))
    }

    /// A file that one test makes. The file goes away when the test ends, and
    /// also when the test panics.
    struct TempFile {
        /// The path of the file.
        path: PathBuf,
    }

    impl TempFile {
        /// Holds a path that no file uses yet, and that no other run reaches.
        fn absent(label: &str) -> Self {
            Self {
                path: temp_path(label),
            }
        }

        /// The path of the file.
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    /// A sink that takes a number of records and then fails every write.
    ///
    /// A file fails a write when the disk fills or the device goes away, and no
    /// test makes either one happen. This sink makes that fault on demand.
    ///
    /// The writer owns its sink and gives it back to nobody, so the bytes live
    /// behind a handle that the test keeps. One test and one writer share the
    /// handle, on one thread.
    #[derive(Clone)]
    struct Sink {
        /// The number of records that the sink takes before it fails.
        takes: usize,
        /// The bytes that reached the sink.
        bytes: Rc<RefCell<Vec<u8>>>,
    }

    impl Sink {
        /// A sink that takes this many records.
        fn that_takes(takes: usize) -> Self {
            Self {
                takes,
                bytes: Rc::new(RefCell::new(Vec::new())),
            }
        }

        /// The bytes that reached the sink.
        fn bytes(&self) -> Vec<u8> {
            self.bytes.borrow().clone()
        }

        /// The text that reached the sink.
        fn text(&self) -> String {
            String::from_utf8(self.bytes()).expect("the sink must hold UTF-8 text")
        }

        /// The number of whole records that reached the sink. Every record ends
        /// with one newline.
        #[allow(
            clippy::naive_bytecount,
            reason = "the sink of a test holds a few hundred bytes, which is no reason to take the bytecount crate as a dependency"
        )]
        fn records(&self) -> usize {
            self.bytes
                .borrow()
                .iter()
                .filter(|byte| **byte == NEWLINE)
                .count()
        }
    }

    impl Write for Sink {
        /// Takes the bytes of a record, until the sink holds the number of
        /// records that it takes.
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.records() >= self.takes {
                return Err(io::Error::other(THE_SINK_IS_FULL));
            }
            self.bytes.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        /// The sink holds every byte in memory, so it flushes nothing.
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// What one recorded run produced.
    struct Ran {
        /// What the run loop gave back.
        outcome: Result<Outcome, RunError>,
        /// What the recorded file holds.
        recording: Recording,
        /// What the run printed.
        status: String,
    }

    /// Records one run to a real file, and reads the file back.
    fn ran(label: &str, rounds: &[RoundRecord], limits: &Limits, stop: &dyn Fn() -> bool) -> Ran {
        let file = TempFile::absent(label);
        let stream = a_stream(rounds);
        let mut status: Vec<u8> = Vec::new();
        let outcome = {
            let mut writer = Writer::append(file.path()).expect("the test file must open");
            record(
                &a_run_record(),
                &stream,
                limits,
                stop,
                &mut writer,
                &mut status,
            )
        };
        Ran {
            outcome,
            recording: Recording::read(file.path()).expect("the test file must read"),
            status: String::from_utf8(status).expect("the status must hold UTF-8 text"),
        }
    }

    /// The outcome of a run that reached a limit.
    fn outcome_of(ran: &Ran) -> &Outcome {
        match &ran.outcome {
            Ok(outcome) => outcome,
            Err(error) => panic!("the run must reach a limit: {error:?}"),
        }
    }

    /// The `type` value that one record writes.
    fn kind_of(record: &Record) -> &'static str {
        match record {
            Record::Run(_) => "run",
            Record::Name(_) => "name",
            Record::Round(_) => "round",
            Record::End(_) => "end",
            Record::Unknown => "unknown",
        }
    }

    /// The `type` value of every record of a recording, in file order.
    fn kinds_of(recording: &Recording) -> Vec<&'static str> {
        recording.records().iter().map(kind_of).collect()
    }

    /// The record that closed the run of a recording.
    fn end_of(recording: &Recording) -> &EndRecord {
        match recording.records().last() {
            Some(Record::End(end)) => end,
            other => panic!("the file must end with an `end` record: {other:?}"),
        }
    }

    /// The number of every round of a recording, in file order.
    fn seqs_of(recording: &Recording) -> Vec<u64> {
        recording
            .records()
            .iter()
            .filter_map(|record| match record {
                Record::Round(round) => Some(round.seq),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn three_rounds_and_a_round_limit_of_three_record_a_run_three_rounds_and_an_end() {
        let ran = ran(
            "round-limit",
            &rounds_of(&[1, 2, 3]),
            &after_rounds(3),
            &|| false,
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "round", "round", "end"]
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Rounds);
        assert_eq!(outcome_of(&ran).rounds, 3);
    }

    #[test]
    fn the_end_record_of_three_rounds_counts_three_rounds() {
        let ran = ran(
            "end-count",
            &rounds_of(&[1, 2, 3]),
            &after_rounds(3),
            &|| false,
        );
        let end = end_of(&ran.recording);
        assert_eq!(end.rounds, 3);
        assert_eq!(end.reason, EndReason::Rounds);
        assert_eq!(end.run, RunId::from(RUN));
    }

    /// A deadline that already passed stops the run before it reads a round,
    /// even though the channel holds rounds that are ready.
    #[test]
    fn a_deadline_that_passed_stops_the_run_and_records_no_round() {
        let ran = ran(
            "past-deadline",
            &rounds_of(&[1, 2]),
            &a_past_deadline(),
            &|| false,
        );
        assert_eq!(kinds_of(&ran.recording), ["run", "end"]);
        assert_eq!(outcome_of(&ran).reason, EndReason::Duration);
        assert_eq!(outcome_of(&ran).rounds, 0);
        assert_eq!(end_of(&ran.recording).reason, EndReason::Duration);
    }

    #[test]
    fn a_user_who_stops_the_run_at_once_records_no_round() {
        let ran = ran("quit-at-once", &rounds_of(&[1, 2]), &NO_LIMIT, &|| true);
        assert_eq!(kinds_of(&ran.recording), ["run", "end"]);
        assert_eq!(outcome_of(&ran).reason, EndReason::Quit);
        assert_eq!(outcome_of(&ran).rounds, 0);
        assert_eq!(end_of(&ran.recording).reason, EndReason::Quit);
    }

    /// The stop closure counts the questions that the loop asks it, so the test
    /// needs no thread and no sleep.
    #[test]
    fn a_user_who_stops_the_run_after_two_rounds_records_two_rounds() {
        let asked = Cell::new(0_u64);
        let stop = || {
            let asked_before = asked.get();
            asked.set(asked_before + 1);
            asked_before >= STOP_AFTER
        };
        let ran = ran(
            "quit-after-two",
            &rounds_of(&[1, 2, 3, 4, 5]),
            &NO_LIMIT,
            &stop,
        );
        assert_eq!(kinds_of(&ran.recording), ["run", "round", "round", "end"]);
        assert_eq!(seqs_of(&ran.recording), [1, 2]);
        assert_eq!(outcome_of(&ran).reason, EndReason::Quit);
        assert_eq!(outcome_of(&ran).rounds, 2);
        assert_eq!(end_of(&ran.recording).rounds, 2);
    }

    #[test]
    fn a_tracer_that_stops_before_a_limit_ends_the_run_with_a_fault() {
        let ran = ran(
            "dead-tracer",
            &rounds_of(&[1, 2]),
            &after_rounds(10),
            &|| false,
        );
        match &ran.outcome {
            Err(RunError::Tracer { rounds }) => assert_eq!(*rounds, 2),
            other => panic!("a dead tracer must stop the run: {other:?}"),
        }
        assert_eq!(kinds_of(&ran.recording), ["run", "round", "round", "end"]);
        let end = end_of(&ran.recording);
        assert_eq!(end.reason, EndReason::Error);
        assert_eq!(end.rounds, 2);
    }

    #[test]
    fn a_stream_that_holds_no_round_still_records_a_run_and_an_end() {
        let ran = ran("no-round", &[], &after_rounds(10), &|| false);
        assert_eq!(kinds_of(&ran.recording), ["run", "end"]);
        match &ran.outcome {
            Err(RunError::Tracer { rounds }) => assert_eq!(*rounds, 0),
            other => panic!("a dead tracer must stop the run: {other:?}"),
        }
        assert_eq!(end_of(&ran.recording).reason, EndReason::Error);
        assert_eq!(end_of(&ran.recording).rounds, 0);
    }

    #[test]
    fn the_rounds_reach_the_file_in_the_order_the_tracer_sent_them() {
        let ran = ran(
            "round-order",
            &rounds_of(&[7, 3, 9]),
            &after_rounds(3),
            &|| false,
        );
        assert_eq!(seqs_of(&ran.recording), [7, 3, 9]);
    }

    #[test]
    fn a_first_write_that_fails_stops_the_run_and_writes_nothing() {
        let sink = Sink::that_takes(0);
        let mut writer = Writer::to_sink(sink.clone());
        let outcome = record(
            &a_run_record(),
            &a_stream(&rounds_of(&[1])),
            &after_rounds(1),
            &|| false,
            &mut writer,
            &mut Vec::new(),
        );
        assert!(
            matches!(outcome, Err(RunError::Write(_))),
            "a failed write stops the run: {outcome:?}"
        );
        assert!(
            sink.bytes().is_empty(),
            "no record reached the sink: {:?}",
            sink.text()
        );
    }

    #[test]
    fn a_write_that_fails_part_way_through_a_run_stops_it_too() {
        let sink = Sink::that_takes(1);
        let mut writer = Writer::to_sink(sink.clone());
        let outcome = record(
            &a_run_record(),
            &a_stream(&rounds_of(&[1])),
            &after_rounds(3),
            &|| false,
            &mut writer,
            &mut Vec::new(),
        );
        assert!(
            matches!(outcome, Err(RunError::Write(_))),
            "a failed write stops the run: {outcome:?}"
        );
        let text = sink.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "one record reached the sink: {text}");
        let written = Record::from_line(lines[0])
            .expect("the record must parse")
            .expect("the record must name a type that this build knows");
        assert_eq!(kind_of(&written), "run");
    }

    // The status line of one round. A later slice replaces this line with the
    // live table.

    /// The status lines that a run printed, without the newline of each one.
    fn lines_of(ran: &Ran) -> Vec<&str> {
        ran.status.lines().collect()
    }

    #[test]
    fn a_round_that_answered_two_hops_and_reached_the_target_prints_one_line() {
        let ran = ran(
            "status-reached",
            &rounds_of(&[1]),
            &after_rounds(1),
            &|| false,
        );
        assert_eq!(ran.status, format!("{A_WHOLE_PATH_LINE}\n"));
    }

    /// One hop keeps the singular name, and a round that reached nothing says
    /// so.
    #[test]
    fn a_round_that_answered_one_hop_and_reached_nothing_prints_the_singular_name() {
        let ran = ran(
            "status-lost",
            &[a_lost_round(LOST_ROUND_SEQ)],
            &after_rounds(1),
            &|| false,
        );
        assert_eq!(ran.status, format!("{A_LOST_ROUND_LINE}\n"));
    }

    #[test]
    fn a_run_of_three_rounds_prints_one_line_for_each_round() {
        let ran = ran(
            "status-three",
            &rounds_of(&[1, 2, 3]),
            &after_rounds(3),
            &|| false,
        );
        assert_eq!(
            lines_of(&ran),
            [
                A_WHOLE_PATH_LINE,
                "round 2  2 hops  reached  1s",
                "round 3  2 hops  reached  1s",
            ]
        );
    }

    #[test]
    fn a_run_that_records_no_round_prints_nothing() {
        let ran = ran("status-none", &rounds_of(&[1, 2]), &NO_LIMIT, &|| true);
        assert_eq!(outcome_of(&ran).rounds, 0);
        assert!(
            ran.status.is_empty(),
            "the run printed no line: {:?}",
            ran.status
        );
    }
}
