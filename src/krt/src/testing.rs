//! The fixtures that the tests of more than one module of this crate build on.
//!
//! A test of the fold, and a test of the lines that the fold prints, both need
//! a round record to read. The record carries a run, a moment, and a duration
//! that no such test asserts on, so one set of values serves every one of them
//! and each test names only the part it cares about: the TTLs that the round
//! probed, and the hops that answered.
//!
//! This module compiles under `cfg(test)` alone, so nothing it holds reaches
//! the binary.

use crate::record::{Hop, RoundRecord, RunId, TtlRange};
use chrono::{DateTime, Utc};
use std::net::IpAddr;

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
