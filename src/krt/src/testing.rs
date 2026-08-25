//! The fixtures that the tests of more than one module of this crate build on.
//!
//! A test of the fold, and a test of the lines that the fold prints, both need
//! a round record to read. The record carries a run, a moment, and a duration
//! that no such test asserts on, so one set of values serves every one of them
//! and each test names only the part it cares about: the TTLs that the round
//! probed, and the hops that answered.
//!
//! The module also holds the fake resolver that a test of the fold and a test
//! of the run loop both program. The fake is a queue of answers for each
//! address, not a contract. The first ask of a real resolver starts the
//! lookup and answers no name, so a test that wants that behavior programs
//! `Lookup::Pending` first. A test of the fold programs a name first.
//!
//! The fake key source stands here for the same reason. A test of the live
//! table and a test of the run loop both script the keys of a turn, and one
//! script serves both.
//!
//! The sink that puts a second run between the writes of the first one stands
//! here for the same reason again. A test of the writer and a test of the
//! replay both need a file that two runs wrote at one moment, and one sink
//! makes that file for both of them.
//!
//! This module compiles under `cfg(test)` alone, so nothing it holds reaches
//! the binary.

use crate::live::{Command, Keys};
use crate::names::{Lookup, Resolver};
use crate::record::{Hop, Record, RecordFile, RoundRecord, RunId, TtlRange, Writer};
use chrono::{DateTime, Utc};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::IpAddr;
use std::path::Path;
use std::rc::Rc;

/// The identifier of the run that every test round belongs to.
const RUN: &str = "2026-08-18T12:00:00.000Z";

/// The moment of every test round.
const MOMENT: &str = "2026-08-18T12:00:01.000Z";

/// The time that every test round took, in milliseconds.
const ROUND_DURATION: u64 = 1000;

/// The name of the ICMP message of every test hop.
const TIME_EXCEEDED: &str = "time_exceeded";

/// Reads an address that a test names.
///
/// # Panics
///
/// Panics on text that holds no address. Such text is a mistake in the test,
/// not an answer the code under test can give.
pub(crate) fn address(text: &str) -> IpAddr {
    text.parse().expect("the test address must parse")
}

/// One round that probed the TTLs of the range, and that the named hops
/// answered.
///
/// Each hop is a TTL, the address that answered at it, and the round-trip time
/// of that answer.
///
/// # Panics
///
/// Panics on a range that no TTL pair holds, and on an address that no text
/// holds. Both are mistakes in the test.
pub(crate) fn round(first: u8, last: u8, hops: &[(u8, &str, f64)]) -> RoundRecord {
    RoundRecord {
        run: RunId::from(RUN),
        seq: 1,
        ts: DateTime::parse_from_rfc3339(MOMENT)
            .expect("the test moment must parse")
            .with_timezone(&Utc),
        dur_ms: ROUND_DURATION,
        ttl_range: TtlRange::new(first, last).expect("the test range must hold"),
        reached: false,
        hops: hops
            .iter()
            .map(|(ttl, addr, rtt_ms)| Hop {
                ttl: *ttl,
                addr: address(addr),
                rtt_ms: *rtt_ms,
                icmp: TIME_EXCEEDED.to_owned(),
            })
            .collect(),
    }
}

/// The answer of a lookup that finished with a name.
pub(crate) fn named(host: &str) -> Lookup {
    Lookup::Named(host.to_owned())
}

/// A resolver that a test programs: one answer for each ask of one address,
/// and the last answer of the list for every ask after the list runs out.
///
/// An address that the test named no answer for answers `Nameless`.
///
/// The count and the answers sit behind a `Cell` and a `RefCell`, because
/// [`Resolver::lookup`] takes the resolver by reference. The fake stays on
/// one thread.
pub(crate) struct FakeResolver {
    /// The answers that each address holds, the next answer first.
    answers: RefCell<HashMap<IpAddr, VecDeque<Lookup>>>,
    /// The number of asks that the resolver took.
    asks: Cell<usize>,
}

impl FakeResolver {
    /// A resolver that answers each address with the answers of its list.
    pub(crate) fn new(answers: &[(&str, &[Lookup])]) -> Rc<Self> {
        Rc::new(Self {
            answers: RefCell::new(
                answers
                    .iter()
                    .map(|(addr, list)| (address(addr), list.iter().cloned().collect()))
                    .collect(),
            ),
            asks: Cell::new(0),
        })
    }

    /// The number of asks that the resolver took.
    pub(crate) fn asks(&self) -> usize {
        self.asks.get()
    }

    /// The answer of one ask, and one step along the list of that address.
    fn answer(&self, addr: IpAddr) -> Lookup {
        self.asks.set(self.asks.get() + 1);
        let mut answers = self.answers.borrow_mut();
        let Some(queue) = answers.get_mut(&addr) else {
            return Lookup::Nameless;
        };
        // The last answer of a list stands for every ask after it, so the
        // list keeps that answer in the place of a step.
        let answer = if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().cloned()
        };
        answer.unwrap_or(Lookup::Nameless)
    }
}

impl Resolver for Rc<FakeResolver> {
    fn lookup(&self, addr: IpAddr) -> Lookup {
        self.answer(addr)
    }
}

/// A key source that hands back one list of commands for each turn.
///
/// A turn past the end of the script took no key.
pub(crate) struct FakeKeys {
    /// The commands of each turn, the next turn first.
    turns: VecDeque<Vec<Command>>,
}

impl FakeKeys {
    /// A key source of one script.
    pub(crate) fn of(script: &[&[Command]]) -> Self {
        Self {
            turns: script.iter().map(|turn| turn.to_vec()).collect(),
        }
    }
}

impl Keys for FakeKeys {
    fn presses(&mut self) -> Vec<Command> {
        self.turns.pop_front().unwrap_or_default()
    }
}

/// A sink that lets a second run append a whole record after every write of
/// the first run.
///
/// The path of a recorded file comes from the source address and the
/// destination, so two runs of one destination from one machine append to one
/// file. The two runs can meet at one place alone: the moment that the writer
/// of the first run releases the file, between two writes of one record.
///
/// A test that starts two threads and hopes that they meet there almost never
/// meets there. A writer that finds the lock of the file taken pauses for a
/// millisecond, and a whole record takes far less than a millisecond, so the
/// two runs take turns and pass each other by. Twenty runs of such a test met
/// at no such moment.
///
/// This sink takes the hope out. It puts the second run at every one of those
/// moments, so a test reads the worst meeting of two runs, and reads it every
/// time. The second run holds a real writer on the same path, and it therefore
/// takes the lock of the file as a second process takes it.
pub(crate) struct SecondRunBetweenWrites<W: Write> {
    /// The sink of the first run. Every write of that run goes here.
    first: W,
    /// The writer of the second run, on the path of the first one.
    second: Writer<RecordFile>,
    /// The records that the second run still has to append, the next one
    /// first.
    ///
    /// A queue that runs out appends nothing more. The number of writes that
    /// one record of the first run takes is what a test of this fixture
    /// measures, so the number of records that the second run appends is not
    /// known before the test runs.
    records: VecDeque<Record>,
}

impl<W: Write> SecondRunBetweenWrites<W> {
    /// Opens the writer of the second run on the path that the first run
    /// writes to.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file does not open for the second run.
    pub(crate) fn on(first: W, path: &Path, records: Vec<Record>) -> std::io::Result<Self> {
        Ok(Self {
            first,
            second: Writer::append(path)?,
            records: records.into(),
        })
    }
}

impl<W: Write> Write for SecondRunBetweenWrites<W> {
    /// Gives the whole buffer to the first run, then lets the second run
    /// append one whole record.
    ///
    /// The answer is the whole length of the buffer, so [`Write::write_all`]
    /// calls this function one time for one buffer. Each write of the first
    /// run therefore makes one meeting of the two runs, and no more than one.
    ///
    /// # Errors
    ///
    /// Returns the reason when the write of the first run fails, and when the
    /// record of the second run does not reach the file.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.first.write_all(buf)?;
        if let Some(record) = self.records.pop_front() {
            self.second.write(&record)?;
        }
        Ok(buf.len())
    }

    /// Flushes the sink of the first run.
    ///
    /// The writer of the second run flushes each of its own records already.
    ///
    /// # Errors
    ///
    /// Returns the reason when the flush of the sink of the first run fails.
    fn flush(&mut self) -> std::io::Result<()> {
        self.first.flush()
    }
}
