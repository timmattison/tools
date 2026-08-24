//! The run loop: the record that opens a run, one record for each name that a
//! reverse lookup gave, one record for each round, the record that closes it,
//! and the screen that shows the run while it stands.
//!
//! A reverse lookup takes a time that no round waits for, so the loop asks the
//! namer of `names.rs` on every turn and writes whatever the namer hands back.
//! A turn that saw no round asks too, so a name that arrives between two rounds
//! still reaches the file. The run also asks one last time before it closes,
//! because the name of an address that a round reports arrives after that
//! round, and the last round of a run has no turn behind it.
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

use crate::live::Screen;
use crate::names::Namer;
use crate::record::{
    EndReason, EndRecord, NameRecord, Record, RoundRecord, RunId, RunRecord, Writer,
};
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
    /// The longest that the run waits, after its last round, for the names that
    /// its lookups have not given yet.
    ///
    /// The wait ends at the moment that every lookup settles, so a run whose
    /// addresses already gave their names pays nothing here. The ceiling of the
    /// value is the timeout of the resolver, because every lookup settles when
    /// that time runs out. A grace of that length therefore outlives every
    /// lookup that can still answer.
    pub(crate) name_grace: Duration,
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
/// `stop` answers whether the user asked the run to stop, and the ask of
/// `screen` answers the same question. The loop asks both once at the top of
/// each turn, before it reads a round, so a run that the user stops records no
/// further round. The ask of the screen also draws that screen, which is why one
/// turn asks one time.
///
/// The `run` record goes first, and a fault there stops the run before it reads
/// anything. Each round that arrives becomes one `round` record. The `end`
/// record names the number of rounds that the run recorded and why it stopped.
///
/// Every turn also asks `namer` for the names that arrived, and each name
/// becomes one `name` record. The turn hands the namer the hops of the round it
/// read, and a turn that read no round hands it nothing, so a lookup that
/// finishes between two rounds still reaches the file. The `name` records of a
/// turn stand before the `round` record of that turn.
///
/// The run asks the namer one last time before it writes the `end` record, and
/// again until no address waits or `limits.name_grace` runs out. The first ask
/// of an address starts the lookup of that address, so the name of an address
/// that the last round reported arrives after that round, and this last ask is
/// what puts it in the file.
///
/// Each round that the run records also reaches `screen`, and the recording
/// comes first: a round reaches the file before it reaches the screen, so a
/// screen that fails loses one frame and no record.
///
/// A failed write of any record stops the run. The recording is the whole
/// purpose of the tool, and a run that keeps a display while it silently
/// records nothing is worse than a run that stops.
///
/// # Errors
///
/// Returns [`RunError::Write`] when a record does not reach the file, and
/// [`RunError::Tracer`] when the tracer thread stops before a limit does.
pub(crate) fn record<W: Write>(
    start: &RunRecord,
    rounds: &Receiver<RoundRecord>,
    limits: &Limits,
    stop: &dyn Fn() -> bool,
    namer: &mut Namer,
    writer: &mut Writer<W>,
    screen: &mut dyn Screen,
) -> Result<Outcome, RunError> {
    writer
        .write(&Record::Run(start.clone()))
        .map_err(RunError::Write)?;
    let mut recorded: u64 = 0;
    loop {
        // The ask of the screen draws it, so one turn asks one time. The answer
        // then travels to the decision as a value.
        let asked = screen.poll();
        if let Some(reason) = stopped(recorded, limits, stop, asked) {
            drain_names(writer, namer, limits.name_grace)?;
            close(writer, &start.run, recorded, reason)?;
            return Ok(Outcome {
                rounds: recorded,
                reason,
            });
        }
        match rounds.recv_timeout(wait(limits.deadline)) {
            Ok(round) => {
                // The names that arrived stand before the round of the same
                // turn. A name always lands on a later turn than the round that
                // first reported its address, because the first ask of an
                // address starts the lookup of that address.
                show_names(writer, screen, &namer.names(&round.hops, Utc::now()))?;
                // The recording comes first. The round reaches the file before
                // it reaches the screen, so a screen that fails loses one frame
                // and no record, and a run whose display the user held still
                // holds every round in its file. The record takes a copy of the
                // round, because the screen reads the round after the write.
                writer
                    .write(&Record::Round(round.clone()))
                    .map_err(RunError::Write)?;
                screen.round(&round);
                recorded += 1;
            }
            // No round arrived inside the wait. The loop takes another turn, so
            // it reads the stop flag and the deadline again. A lookup that
            // finished inside the wait lands here, so a name that arrives
            // between two rounds reaches the file at the moment it arrives.
            Err(RecvTimeoutError::Timeout) => {
                show_names(writer, screen, &namer.names(&[], Utc::now()))?;
            }
            Err(RecvTimeoutError::Disconnected) => {
                // The tracer holds the sender for as long as it lives, so a
                // closed channel names a tracer thread that died.
                //
                // The `?` gives the write fault the last word. A run whose
                // `end` record will not write reports that fault and not the
                // dead tracer, because a file that takes no record names the
                // fault that stops the tool from doing its job at all.
                //
                // The grace of this drain is zero. The names that already
                // arrived still reach the file, and a run that is already
                // failing waits for nothing.
                drain_names(writer, namer, Duration::ZERO)?;
                close(writer, &start.run, recorded, EndReason::Error)?;
                return Err(RunError::Tracer { rounds: recorded });
            }
        }
    }
}

/// Appends one record for each name that arrived, and then shows the names.
///
/// The record comes first, for the reason that the round of a turn does: the
/// file is the purpose of the tool, and the screen is one view of it.
///
/// A turn of no name shows nothing. The loop takes ten turns each second, and a
/// live table that took an empty list on each of them would clear the screen and
/// paint it again at that rate.
///
/// # Errors
///
/// Returns [`RunError::Write`] when a record does not reach the file.
fn show_names<W: Write>(
    writer: &mut Writer<W>,
    screen: &mut dyn Screen,
    names: &[NameRecord],
) -> Result<(), RunError> {
    write_names(writer, names)?;
    if !names.is_empty() {
        screen.names(names);
    }
    Ok(())
}

/// Appends one record for each name that arrived.
///
/// Each record holds a copy of one name, because the caller keeps the names for
/// the screen and the writer takes a whole record.
///
/// # Errors
///
/// Returns [`RunError::Write`] when a record does not reach the file.
fn write_names<W: Write>(writer: &mut Writer<W>, names: &[NameRecord]) -> Result<(), RunError> {
    for name in names {
        writer
            .write(&Record::Name(name.clone()))
            .map_err(RunError::Write)?;
    }
    Ok(())
}

/// Asks the namer one last time, and again until no address waits or the grace
/// runs out. Each name that arrives becomes one record.
///
/// The first ask of an address starts the lookup of that address, so the name of
/// an address that a round reports arrives on a later turn. A run whose last
/// turn is its stopping turn therefore holds names that no turn wrote, and this
/// drain is the turn that writes them.
///
/// The drain asks once whatever the grace, so a grace of zero still writes the
/// names that already arrived. Between two asks it sleeps for [`POLL`] at the
/// longest, and never past the end of the grace.
///
/// The names of the drain reach no screen. The drain runs while the run closes,
/// and the display of a run that closes shows nothing more. Every one of them
/// reaches the file, so a replay of that file names the address.
///
/// # Errors
///
/// Returns [`RunError::Write`] when a record does not reach the file.
fn drain_names<W: Write>(
    writer: &mut Writer<W>,
    namer: &mut Namer,
    grace: Duration,
) -> Result<(), RunError> {
    let end = Instant::now() + grace;
    loop {
        write_names(writer, &namer.names(&[], Utc::now()))?;
        if !namer.waits() {
            return Ok(());
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(POLL));
    }
}

/// Why the run stops at the top of this turn. A run that goes on stops for
/// nothing.
///
/// `asked` is what the screen of this turn answered. The ask of a screen draws
/// it, so the turn asks one time and hands the answer here as a value.
///
/// The user comes first, then the number of rounds, then the moment. The screen
/// and the stop flag are two doors of the same user: the key of a live table and
/// the signal of a headless run both give [`EndReason::Quit`].
fn stopped(
    recorded: u64,
    limits: &Limits,
    stop: &dyn Fn() -> bool,
    asked: bool,
) -> Option<EndReason> {
    if asked || stop() {
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
    use crate::live::{Command, RunFacts, Screen, Table, Window};
    use crate::names::{Lookup, Namer, NoLookups};
    use crate::record::{
        EndReason, EndRecord, Family, Hop, NameRecord, Privilege, Record, Recording, RoundRecord,
        RunConfig, RunId, RunRecord, SourceKind, SourceLabel, Target, TtlRange, Writer,
    };
    use crate::testing::{address, named, FakeKeys, FakeResolver};
    use crate::{Multipath, Protocol};
    use chrono::{DateTime, Utc};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    /// The identifier of the run that every test record belongs to.
    const RUN: &str = "2026-08-18T12:00:00.123Z";

    /// The moment that every test round starts.
    const ROUND_START: &str = "2026-08-18T12:34:56.789Z";

    /// The address that the probes of a test run leave from.
    const SOURCE_ADDRESS: &str = "1.2.3.4";

    /// The address of the first hop of a test round.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The name that a reverse lookup gives the first hop of a test round.
    const FIRST_HOP_NAME: &str = "gateway.example.com";

    /// The address of the target of a test run.
    const TARGET_ADDRESS: &str = "93.184.216.34";

    /// The name that a reverse lookup gives the target of a test run.
    const TARGET_NAME: &str = "target.example.com";

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

    /// The grace that every test run gives its names.
    ///
    /// The fake resolver of a test answers at once, so no test waits for a
    /// lookup. The run still asks one last time before it closes, whatever this
    /// value, so a name that arrived after the last round still lands.
    const NO_GRACE: Duration = Duration::ZERO;

    /// The grace of a run whose name arrives after its last round.
    ///
    /// The drain sleeps for one hundred milliseconds at the longest between two
    /// asks, so this value holds a few asks. The name arrives on the second ask
    /// of the drain, and the run closes there, so the test pays one sleep and
    /// not this whole time.
    const A_GRACE: Duration = Duration::from_millis(300);

    /// The grace of a run whose lookup never finishes.
    ///
    /// The drain asks until this time runs out, so this value is the whole cost
    /// of the test that reads it.
    const A_SHORT_GRACE: Duration = Duration::from_millis(200);

    /// The limits of a run that stops on nothing.
    const NO_LIMIT: Limits = Limits {
        rounds: None,
        deadline: None,
        name_grace: NO_GRACE,
    };

    /// The number of turns that a run takes before the user stops it.
    ///
    /// The loop asks the stop closure once at the top of each turn, so the
    /// third question follows the second turn.
    const STOP_AFTER: u64 = 2;

    /// The number of turns that a run takes before the user stops it right
    /// after its one round.
    ///
    /// The second question follows the first turn, so the run stops on the turn
    /// that comes straight after the round it recorded.
    const STOP_AFTER_ONE_TURN: u64 = 1;

    /// The number of terminal columns that the live table of a test draws in.
    ///
    /// The nominal frame is 97 columns wide: every column of the table, with a
    /// Host column of 30.
    const WIDTH: u16 = 97;

    /// The rows of the window that the live table of a test draws in.
    ///
    /// No probe measured that window, so the table fits its frames to no
    /// height and draws every line of them. A test of this file reads what a
    /// frame holds, and a height that left rows out would take those lines off
    /// the frame.
    const NO_ROWS: Option<u16> = None;

    /// The period of one round of the run that a live table shows.
    const AN_INTERVAL: Duration = Duration::from_secs(1);

    /// The word that a table which holds where it stands writes under itself.
    ///
    /// The test spells the word, and the module of the table spells it again.
    /// That is on purpose: a test that read the constant of that module would
    /// agree with every word the module ever holds, and this word is what a
    /// reader of the screen sees.
    const PAUSED: &str = "paused";

    /// The sequence that starts every frame of a live table.
    ///
    /// A draw clears the screen first, so the text between two of these
    /// sequences is one frame.
    const CLEAR: &str = "\u{1b}[2J";

    /// The number of frames that the run of a held table drew.
    ///
    /// One frame at the pause, one at the release of that pause, and one for
    /// the round that arrived after the release. The rounds that arrived while
    /// the table held drew none.
    const FRAMES_OF_A_HELD_RUN: usize = 3;

    /// The number of rounds that arrive while the table holds.
    const ROUNDS_WHILE_HELD: u64 = 3;

    /// The number of rounds of the run of a held table.
    const ROUNDS_OF_A_HELD_RUN: u64 = 4;

    /// The number of rounds that a table which folded none names.
    const NO_ROUND: u64 = 0;

    /// The number of turns that a run takes before its screen stops it.
    ///
    /// The screen answers the first turn, and the run records the round of that
    /// turn. The second answer stops the run, so the run holds one round.
    const SCREEN_STOPS_AFTER: usize = 1;

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

    /// One round that no hop answered.
    fn a_silent_round(seq: u64) -> RoundRecord {
        RoundRecord {
            run: RunId::from(RUN),
            seq,
            ts: moment(ROUND_START),
            dur_ms: LOST_ROUND_MS,
            ttl_range: TtlRange::new(FIRST_TTL, TARGET_TTL).expect("the test range must hold"),
            reached: false,
            hops: Vec::new(),
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

    /// A channel that holds these rounds and whose sender the caller keeps.
    ///
    /// The loop reads a closed channel as a tracer that died, so a test whose
    /// run must reach a limit holds the sender for the whole run. The tracer
    /// holds its own sender the same way.
    fn a_held_stream(rounds: &[RoundRecord]) -> (Sender<RoundRecord>, Receiver<RoundRecord>) {
        let (sender, receiver) = mpsc::channel();
        for round in rounds {
            sender
                .send(round.clone())
                .expect("the receiver of the test must stand");
        }
        (sender, receiver)
    }

    /// The limits of a run that stops after this many rounds.
    fn after_rounds(rounds: u64) -> Limits {
        after_rounds_with_grace(rounds, NO_GRACE)
    }

    /// The limits of a run that stops after this many rounds and that gives its
    /// names this grace.
    fn after_rounds_with_grace(rounds: u64, grace: Duration) -> Limits {
        Limits {
            rounds: Some(rounds),
            deadline: None,
            name_grace: grace,
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
            name_grace: NO_GRACE,
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

    /// A sink that the test reads while a live table writes into it.
    ///
    /// A table owns its sink and gives it back to nobody, so the bytes live
    /// behind a handle that the test keeps. One test and one table share the
    /// handle, on one thread.
    #[derive(Clone)]
    struct Shared {
        /// The bytes that reached the sink.
        bytes: Rc<RefCell<Vec<u8>>>,
    }

    impl Shared {
        /// A sink that holds nothing.
        fn new() -> Self {
            Self {
                bytes: Rc::new(RefCell::new(Vec::new())),
            }
        }

        /// The text that reached the sink.
        fn text(&self) -> String {
            String::from_utf8(self.bytes.borrow().clone()).expect("the sink must hold UTF-8 text")
        }
    }

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        /// The sink holds every byte in memory, so it flushes nothing.
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The frames that a live table drew, in the order they reached the sink.
    fn frames(painted: &str) -> Vec<&str> {
        painted.split(CLEAR).skip(1).collect()
    }

    /// The text of a header line that names this number of rounds.
    fn rounds_in_header(rounds: u64) -> String {
        format!("round {rounds}")
    }

    /// A namer of the test run that asks this resolver.
    fn a_namer(resolver: &Rc<FakeResolver>) -> Namer {
        Namer::new(Box::new(Rc::clone(resolver)), RunId::from(RUN))
    }

    /// A namer of the test run that looks nothing up.
    fn a_nameless_namer() -> Namer {
        Namer::new(Box::new(NoLookups), RunId::from(RUN))
    }

    /// A screen that keeps what the run showed on it.
    ///
    /// A run hands its screen every round and every name that it recorded, and
    /// a test of the loop asks what reached the screen. This screen keeps both,
    /// and it answers the stop question from a script.
    struct Recorder {
        /// The answer of each turn, the next turn first. A turn past the end of
        /// the script asks for no stop of the run.
        stops: VecDeque<bool>,
        /// The rounds that reached the screen, in the order they arrived.
        rounds: Vec<RoundRecord>,
        /// The names that reached the screen, in the order they arrived.
        names: Vec<NameRecord>,
    }

    impl Recorder {
        /// A screen that asks for no stop of the run.
        fn new() -> Self {
            Self {
                stops: VecDeque::new(),
                rounds: Vec::new(),
                names: Vec::new(),
            }
        }

        /// A screen that asks for the stop of the run on the turn that follows
        /// this many turns.
        fn that_stops_after(turns: usize) -> Self {
            let mut screen = Self::new();
            screen.stops = vec![false; turns].into();
            screen.stops.push_back(true);
            screen
        }

        /// The number of every round that reached the screen, in the order they
        /// arrived.
        fn seqs(&self) -> Vec<u64> {
            self.rounds.iter().map(|round| round.seq).collect()
        }
    }

    impl Screen for Recorder {
        fn poll(&mut self) -> bool {
            self.stops.pop_front().unwrap_or(false)
        }

        fn round(&mut self, round: &RoundRecord) {
            self.rounds.push(round.clone());
        }

        fn names(&mut self, names: &[NameRecord]) {
            self.names.extend_from_slice(names);
        }
    }

    /// What one recorded run produced.
    struct Ran {
        /// What the run loop gave back.
        outcome: Result<Outcome, RunError>,
        /// What the recorded file holds.
        recording: Recording,
        /// What the run showed on its screen.
        screen: Recorder,
    }

    /// Records one run that looks nothing up to a real file, and reads the file
    /// back.
    fn ran(label: &str, rounds: &[RoundRecord], limits: &Limits, stop: &dyn Fn() -> bool) -> Ran {
        ran_with(
            label,
            &a_stream(rounds),
            limits,
            stop,
            &mut a_nameless_namer(),
        )
    }

    /// Records one run that asks this namer to a real file, and reads the file
    /// back.
    fn ran_with(
        label: &str,
        rounds: &Receiver<RoundRecord>,
        limits: &Limits,
        stop: &dyn Fn() -> bool,
        namer: &mut Namer,
    ) -> Ran {
        ran_seeing(label, rounds, limits, stop, namer, Recorder::new())
    }

    /// Records one run that shows itself on this screen, and reads the file
    /// back.
    fn ran_seeing(
        label: &str,
        rounds: &Receiver<RoundRecord>,
        limits: &Limits,
        stop: &dyn Fn() -> bool,
        namer: &mut Namer,
        mut screen: Recorder,
    ) -> Ran {
        let file = TempFile::absent(label);
        let outcome = ran_on(file.path(), rounds, limits, stop, namer, &mut screen);
        Ran {
            outcome,
            recording: Recording::read(file.path()).expect("the test file must read"),
            screen,
        }
    }

    /// Records one run whose screen the caller owns, into the file at `path`.
    ///
    /// The writer of the run drops before the call answers, so the caller reads
    /// a file that holds every record of the run.
    fn ran_on(
        path: &Path,
        rounds: &Receiver<RoundRecord>,
        limits: &Limits,
        stop: &dyn Fn() -> bool,
        namer: &mut Namer,
        screen: &mut dyn Screen,
    ) -> Result<Outcome, RunError> {
        let mut writer = Writer::append(path).expect("the test file must open");
        record(
            &a_run_record(),
            rounds,
            limits,
            stop,
            namer,
            &mut writer,
            screen,
        )
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

    /// The `type` value of every record that reached a sink, in write order.
    fn kinds_in(text: &str) -> Vec<&'static str> {
        text.lines()
            .map(|line| {
                let written = Record::from_line(line)
                    .expect("the record must parse")
                    .expect("the record must name a type that this build knows");
                kind_of(&written)
            })
            .collect()
    }

    /// The `name` record of every name of a recording, in file order.
    fn names_of(recording: &Recording) -> Vec<&NameRecord> {
        recording
            .records()
            .iter()
            .filter_map(|record| match record {
                Record::Name(name) => Some(name),
                _ => None,
            })
            .collect()
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
            &mut a_nameless_namer(),
            &mut writer,
            &mut Recorder::new(),
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
            &mut a_nameless_namer(),
            &mut writer,
            &mut Recorder::new(),
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

    // The `name` record of each address that a run sees.

    #[test]
    fn an_address_that_resolves_writes_one_name_record_of_the_run() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let ran = ran_with(
            "name-one",
            &a_stream(&rounds_of(&[1])),
            &after_rounds(1),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "name", "round", "end"],
            "the name of the round stands before the round"
        );
        let names = names_of(&ran.recording);
        assert_eq!(names.len(), 1, "the run named one address: {names:?}");
        assert_eq!(names[0].run, RunId::from(RUN), "the record names the run");
        assert_eq!(
            names[0].addr,
            address(FIRST_HOP),
            "the record names the address"
        );
        assert_eq!(names[0].host, FIRST_HOP_NAME, "the record holds the name");
    }

    #[test]
    fn an_address_in_every_round_writes_one_name_record_for_the_whole_run() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let ran = ran_with(
            "name-once",
            &a_stream(&rounds_of(&[1, 2, 3])),
            &after_rounds(3),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(seqs_of(&ran.recording), [1, 2, 3]);
        let names = names_of(&ran.recording);
        assert_eq!(
            names.len(),
            1,
            "one address takes one record in one run: {names:?}"
        );
        assert_eq!(names[0].addr, address(FIRST_HOP));
    }

    /// The acceptance of the reverse DNS work: a lookup that takes its time
    /// holds up no round of the run.
    ///
    /// The resolver answers `Pending` for the first two asks of the first hop
    /// and gives the name on the third, so the name lands on the third turn.
    /// Every round still reaches the file, and in its own turn.
    #[test]
    fn a_slow_lookup_holds_up_no_round_of_the_run() {
        let resolver = FakeResolver::new(&[(
            FIRST_HOP,
            &[Lookup::Pending, Lookup::Pending, named(FIRST_HOP_NAME)],
        )]);
        // The sender stands for the whole run, so the loop reaches its limit
        // and reads no closed channel.
        let (_sender, stream) = a_held_stream(&rounds_of(&[1, 2, 3]));
        let ran = ran_with(
            "name-slow",
            &stream,
            &after_rounds(3),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            seqs_of(&ran.recording),
            [1, 2, 3],
            "the slow lookup held up no round"
        );
        let names = names_of(&ran.recording);
        assert_eq!(names.len(), 1, "the slow lookup landed once: {names:?}");
        assert_eq!(names[0].addr, address(FIRST_HOP));
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "round", "name", "round", "end"],
            "the name stands after the rounds that ran before it arrived"
        );
    }

    #[test]
    fn a_lookup_that_never_finishes_still_records_every_round_and_closes_the_run() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending])]);
        let (_sender, stream) = a_held_stream(&rounds_of(&[1, 2, 3]));
        let ran = ran_with(
            "name-never",
            &stream,
            &after_rounds(3),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "round", "round", "end"]
        );
        assert!(
            names_of(&ran.recording).is_empty(),
            "a lookup that never finishes writes no record"
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Rounds);
        assert_eq!(end_of(&ran.recording).rounds, 3);
    }

    #[test]
    fn a_run_that_looks_nothing_up_writes_no_name_record() {
        let ran = ran(
            "name-none",
            &rounds_of(&[1, 2, 3]),
            &after_rounds(3),
            &|| false,
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "round", "round", "end"]
        );
        assert!(
            names_of(&ran.recording).is_empty(),
            "the resolver that looks nothing up names no address"
        );
    }

    /// The sink takes the `run` record, the name of the target, and the first
    /// round. The next write is the name of the first hop, which the second
    /// turn reads, so the write that fails is a `name` record.
    #[test]
    fn a_failed_write_of_a_name_record_stops_the_run() {
        let resolver = FakeResolver::new(&[
            (TARGET_ADDRESS, &[named(TARGET_NAME)]),
            (FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)]),
        ]);
        let sink = Sink::that_takes(3);
        let mut writer = Writer::to_sink(sink.clone());
        let outcome = record(
            &a_run_record(),
            &a_stream(&rounds_of(&[1, 2])),
            &after_rounds(2),
            &|| false,
            &mut a_namer(&resolver),
            &mut writer,
            &mut Recorder::new(),
        );
        assert!(
            matches!(outcome, Err(RunError::Write(_))),
            "a failed write of a name stops the run: {outcome:?}"
        );
        let text = sink.text();
        assert_eq!(
            kinds_in(&text),
            ["run", "name", "round"],
            "the run stopped on the name of the second turn: {text}"
        );
    }

    #[test]
    fn a_round_that_reports_no_hop_writes_no_name_record() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let ran = ran_with(
            "name-no-hop",
            &a_stream(&[a_silent_round(1)]),
            &after_rounds(1),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(kinds_of(&ran.recording), ["run", "round", "end"]);
        assert!(
            names_of(&ran.recording).is_empty(),
            "a round that reports no hop names no address"
        );
        assert_eq!(
            resolver.asks(),
            0,
            "a round that reports no hop asks the resolver nothing"
        );
    }

    /// A lookup that finishes between two rounds lands on the turn that saw no
    /// round, so the name reaches the file whatever the period of the rounds.
    #[test]
    fn a_name_that_arrives_between_two_rounds_reaches_the_file_on_the_turn_that_saw_no_round() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)])]);
        // The channel holds one round and the sender stands, so the second turn
        // of the loop waits out its poll and reads no round.
        let (_sender, stream) = a_held_stream(&rounds_of(&[1]));
        let asked = Cell::new(0_u64);
        let stop = || {
            let asked_before = asked.get();
            asked.set(asked_before + 1);
            asked_before >= STOP_AFTER
        };
        let ran = ran_with(
            "name-late",
            &stream,
            &NO_LIMIT,
            &stop,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "name", "end"],
            "the name stands after the one round that the run recorded"
        );
        assert_eq!(
            names_of(&ran.recording)[0].host,
            FIRST_HOP_NAME,
            "the record holds the name that arrived"
        );
    }

    /// The first ask of an address starts its lookup, so the name of the one
    /// round of a one round run arrives on the turn that stops the run. The run
    /// asks one last time before it closes, so the name still reaches the file.
    #[test]
    fn a_run_that_stops_after_its_last_round_still_writes_the_name_that_arrived() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)])]);
        let ran = ran_with(
            "name-drain-rounds",
            &a_stream(&rounds_of(&[1])),
            &after_rounds(1),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "name", "end"],
            "the name that arrived after the last round stands before the end"
        );
        assert_eq!(
            names_of(&ran.recording)[0].host,
            FIRST_HOP_NAME,
            "the record holds the name that arrived"
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Rounds);
        assert_eq!(outcome_of(&ran).rounds, 1);
    }

    /// A run that the user stops takes the same last ask, so a name that
    /// arrives on the stopping turn reaches the file.
    #[test]
    fn a_run_that_the_user_stops_still_writes_the_name_that_arrived() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)])]);
        // The sender stands for the whole run, so the loop stops on the user
        // and reads no closed channel.
        let (_sender, stream) = a_held_stream(&rounds_of(&[1]));
        let asked = Cell::new(0_u64);
        let stop = || {
            let asked_before = asked.get();
            asked.set(asked_before + 1);
            asked_before >= STOP_AFTER_ONE_TURN
        };
        let ran = ran_with(
            "name-drain-quit",
            &stream,
            &NO_LIMIT,
            &stop,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "name", "end"],
            "the name that arrived on the stopping turn stands before the end"
        );
        assert_eq!(
            names_of(&ran.recording)[0].host,
            FIRST_HOP_NAME,
            "the record holds the name that arrived"
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Quit);
        assert_eq!(outcome_of(&ran).rounds, 1);
    }

    /// One ask of the drain is not always enough. The drain asks again until
    /// the name arrives, and the grace holds the run open for it.
    ///
    /// The resolver answers `Pending` for the first two asks of the first hop.
    /// The round takes the first ask and the drain takes the second, so the
    /// name arrives on the third ask, which is the second ask of the drain. The
    /// drain sleeps between those two asks.
    #[test]
    fn a_name_that_the_first_ask_of_the_drain_does_not_reach_arrives_on_the_second() {
        let resolver = FakeResolver::new(&[(
            FIRST_HOP,
            &[Lookup::Pending, Lookup::Pending, named(FIRST_HOP_NAME)],
        )]);
        let ran = ran_with(
            "name-drain-again",
            &a_stream(&rounds_of(&[1])),
            &after_rounds_with_grace(1, A_GRACE),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "name", "end"],
            "the name of the second ask of the drain stands before the end"
        );
        assert_eq!(
            names_of(&ran.recording)[0].host,
            FIRST_HOP_NAME,
            "the record holds the name that the drain waited for"
        );
        assert_eq!(
            resolver.asks(),
            4,
            "the round asked about the two hops, and the drain asked twice more about the first one"
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Rounds);
        assert_eq!(outcome_of(&ran).rounds, 1);
    }

    /// The grace bounds the drain. A lookup that never finishes holds the run
    /// open for the grace and no longer, and the run closes with no name.
    #[test]
    fn a_lookup_that_never_finishes_lets_the_grace_close_the_run() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending])]);
        let ran = ran_with(
            "name-drain-grace",
            &a_stream(&rounds_of(&[1])),
            &after_rounds_with_grace(1, A_SHORT_GRACE),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "end"],
            "the run closed after the grace ran out"
        );
        assert!(
            names_of(&ran.recording).is_empty(),
            "a lookup that never finishes writes no record"
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Rounds);
        assert_eq!(outcome_of(&ran).rounds, 1);
    }

    // What the run shows on its screen. The one line that a headless screen
    // prints for a round is the work of `live::status_line`, and the tests of
    // that line stand beside it.

    #[test]
    fn a_run_of_three_rounds_hands_the_screen_three_rounds() {
        let ran = ran(
            "screen-three",
            &rounds_of(&[1, 2, 3]),
            &after_rounds(3),
            &|| false,
        );
        assert_eq!(ran.screen.seqs(), [1, 2, 3]);
        assert_eq!(outcome_of(&ran).rounds, 3);
    }

    #[test]
    fn a_run_that_records_no_round_hands_the_screen_nothing() {
        let ran = ran("screen-none", &rounds_of(&[1, 2]), &NO_LIMIT, &|| true);
        assert_eq!(outcome_of(&ran).rounds, 0);
        assert!(
            ran.screen.rounds.is_empty(),
            "the run showed no round: {:?}",
            ran.screen.seqs()
        );
    }

    /// The key of a live table stops the run, and the signal of a headless run
    /// stops it the same way. The stop closure of this run asks for nothing, so
    /// the screen is the one thing that stopped it.
    #[test]
    fn a_screen_that_asks_to_stop_ends_the_run_with_a_quit() {
        let ran = ran_seeing(
            "screen-quit",
            &a_stream(&rounds_of(&[1, 2])),
            &NO_LIMIT,
            &|| false,
            &mut a_nameless_namer(),
            Recorder::that_stops_after(SCREEN_STOPS_AFTER),
        );
        assert_eq!(outcome_of(&ran).reason, EndReason::Quit);
        assert_eq!(outcome_of(&ran).rounds, 1);
        assert_eq!(
            seqs_of(&ran.recording),
            [1],
            "the run recorded the round of the turn before the screen stopped it"
        );
        assert_eq!(end_of(&ran.recording).reason, EndReason::Quit);
    }

    #[test]
    fn every_round_reaches_the_screen_in_the_order_the_tracer_sent_them() {
        let ran = ran(
            "screen-order",
            &rounds_of(&[7, 3, 9]),
            &after_rounds(3),
            &|| false,
        );
        assert_eq!(ran.screen.seqs(), [7, 3, 9]);
    }

    /// A live table names the host of a row, so every name that the run writes
    /// reaches the screen as well as the file.
    #[test]
    fn a_name_that_arrives_reaches_the_screen() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let ran = ran_with(
            "screen-name",
            &a_stream(&rounds_of(&[1])),
            &after_rounds(1),
            &|| false,
            &mut a_namer(&resolver),
        );
        let names = &ran.screen.names;
        assert_eq!(names.len(), 1, "the run showed one name: {names:?}");
        assert_eq!(
            names[0].addr,
            address(FIRST_HOP),
            "the name that reached the screen names the address of the hop"
        );
        assert_eq!(
            names[0].host, FIRST_HOP_NAME,
            "and it holds the name that the lookup gave"
        );
    }

    /// The last ask of a run runs while that run closes, and the display of a
    /// run that closes shows nothing more. The name still reaches the file, so
    /// a replay of that file names the address.
    #[test]
    fn the_names_that_a_run_writes_while_it_closes_reach_no_screen() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)])]);
        let ran = ran_with(
            "screen-name-drain",
            &a_stream(&rounds_of(&[1])),
            &after_rounds(1),
            &|| false,
            &mut a_namer(&resolver),
        );
        assert_eq!(
            kinds_of(&ran.recording),
            ["run", "round", "name", "end"],
            "the name that arrived after the last round reached the file"
        );
        assert!(
            ran.screen.names.is_empty(),
            "and it reached no screen: {:?}",
            ran.screen.names
        );
    }

    /// The acceptance of the live table: a table that the user holds still
    /// records every round.
    ///
    /// The display is one view of the recording, and the pause key holds that
    /// view alone. The file behind it takes every round while the view stands
    /// still, and the fold behind it takes every round too, so the frame of the
    /// release names the rounds that arrived while the table held.
    ///
    /// The key of the first turn holds the table, and the key of the fourth
    /// turn lets it move again. Three rounds arrive between those two turns.
    #[test]
    fn a_run_whose_table_is_paused_keeps_writing_every_round() {
        // The name of the file stands in the header line of every frame, so
        // the label of it holds no word that a frame is read for.
        let file = TempFile::absent("held-table");
        let painted = Shared::new();
        let mut screen = Table::new(
            RunFacts {
                destination: TARGET_ARG.to_owned(),
                address: address(TARGET_ADDRESS),
                source: address(SOURCE_ADDRESS),
                interval: AN_INTERVAL,
                path: file.path().to_owned(),
            },
            painted.clone(),
            FakeKeys::of(&[&[Command::Pause], &[], &[], &[Command::Pause]]),
            Window::new(WIDTH, NO_ROWS),
        );
        let outcome = ran_on(
            file.path(),
            &a_stream(&rounds_of(&[1, 2, 3, 4])),
            &after_rounds(ROUNDS_OF_A_HELD_RUN),
            &|| false,
            &mut a_nameless_namer(),
            &mut screen,
        );
        let recording = Recording::read(file.path()).expect("the test file must read");

        assert_eq!(
            seqs_of(&recording),
            [1, 2, 3, 4],
            "every round of the run reached the file, and the pause held none of them back"
        );
        match &outcome {
            Ok(outcome) => assert_eq!(outcome.rounds, ROUNDS_OF_A_HELD_RUN),
            Err(error) => panic!("the run must reach its round limit: {error:?}"),
        }

        let text = painted.text();
        let drawn = frames(&text);
        assert_eq!(
            drawn.len(),
            FRAMES_OF_A_HELD_RUN,
            "the rounds that arrived while the table held drew nothing: {drawn:?}"
        );
        assert!(
            drawn[0].contains(PAUSED) && drawn[0].contains(&rounds_in_header(NO_ROUND)),
            "the first frame marks the pause, and the table under it folded no round: {:?}",
            drawn[0]
        );
        assert!(
            drawn[1].contains(&rounds_in_header(ROUNDS_WHILE_HELD)),
            "the frame of the release names the rounds that arrived while the table held: {:?}",
            drawn[1]
        );
        assert!(
            !drawn[1].contains(PAUSED),
            "and it carries no mark of a pause: {:?}",
            drawn[1]
        );
        assert!(
            drawn[2].contains(&rounds_in_header(ROUNDS_OF_A_HELD_RUN)),
            "the round that arrived after the release drew the frame of it: {:?}",
            drawn[2]
        );
    }
}
