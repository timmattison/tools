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
//! This module compiles under `cfg(test)` alone, so nothing it holds reaches
//! the binary.

use crate::live::{Command, Keys};
use crate::names::{Lookup, Resolver};
use crate::record::{Hop, RoundRecord, RunId, TtlRange};
use chrono::{DateTime, Utc};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
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
