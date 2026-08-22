//! The privilege gate of a run, the wall in front of the tracer, and the
//! conversion of one round into one record.
//!
//! `krt` probes through the trippy crates. No other module of `krt` names a
//! type of those crates, so an upgrade of them breaks this one file and no
//! other. The guard `repo_guards::trippy_wall` holds that rule in place.
//!
//! The gate asks the platform whether a probe needs raw socket privileges, and
//! whether the process holds them. A platform that needs none runs
//! unprivileged, even when the process holds them, because a run that quietly
//! changes the way it probes records one thing and does another. A platform
//! that needs them and holds none stops, and the message names the remedy of
//! each platform.
//!
//! The conversion is the wall itself. The tracer hands one round over as a
//! borrowed value that lives for the length of one call, and
//! [`to_round_record`] copies every value that the record keeps. Nothing
//! borrowed crosses out of this module, so a later change of engine touches
//! this file alone. The run loop that drives the tracer arrives in a later
//! slice.

use crate::record::{self, RoundRecord, RunId, TtlRange};
use chrono::{DateTime, Utc};
use trippy_core::{IcmpPacketType, Round};

/// The remedy of a platform that needs raw socket privileges and holds none.
///
/// `main` writes every message as `krt: {reason}`, so the text carries no
/// program name. The two lines under the first one carry two spaces each, and
/// the remedy of each platform starts at the same column.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const MISSING_PRIVILEGES: &str = "\
this platform needs raw socket privileges to send probes.
  Linux:   sudo setcap 'cap_net_raw+p' $(which krt)
  Windows: run krt from an elevated prompt";

/// Acquires the privileges of the platform and decides the mode of a run.
///
/// # Errors
///
/// Returns [`PrivilegeError::Missing`] when the platform needs raw socket
/// privileges and the process does not hold them. Returns
/// [`PrivilegeError::Discovery`] when the platform will not report what it
/// holds.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
pub(crate) fn acquire_privilege() -> Result<record::Privilege, PrivilegeError> {
    let privilege = trippy_privilege::Privilege::acquire_privileges().map_err(|error| {
        PrivilegeError::Discovery {
            reason: error.to_string(),
        }
    })?;
    choose_privilege(privilege.has_privileges(), privilege.needs_privileges())
}

/// Decides the mode of a run from what the platform reports.
///
/// `has` is true when the process holds raw socket privileges. `needs` is true
/// when a probe of this platform needs them.
///
/// # Errors
///
/// Returns [`PrivilegeError::Missing`] when the platform needs the privileges
/// and the process holds none.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
fn choose_privilege(has: bool, needs: bool) -> Result<record::Privilege, PrivilegeError> {
    match (needs, has) {
        // macOS sends through an `IPPROTO_ICMP` socket with the `IP_HDRINCL`
        // socket option, so it needs no privileges. A process that holds them
        // there still runs unprivileged, because `krt` never changes the way it
        // probes without a word.
        (false, _) => Ok(record::Privilege::Unprivileged),
        (true, true) => Ok(record::Privilege::Privileged),
        (true, false) => Err(PrivilegeError::Missing),
    }
}

/// Why a run does not start.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum PrivilegeError {
    /// The platform needs raw socket privileges, and the process holds none.
    ///
    /// Linux supports an `IPPROTO_ICMP` socket and does not support the
    /// `IP_HDRINCL` socket option, so it needs `CAP_NET_RAW`. Windows needs an
    /// elevated token. The message names the remedy of each one.
    #[error("{MISSING_PRIVILEGES}")]
    Missing,
    /// The platform will not report the privileges that it holds.
    #[error("the platform will not report the privileges that it holds: {reason}")]
    Discovery {
        /// The reason that the platform gave.
        reason: String,
    },
}

/// The name that the schema records when a hop below the target answered that
/// the TTL of the probe ran out.
const TIME_EXCEEDED: &str = "time_exceeded";

/// The name that the schema records when the target answered the echo request.
const ECHO_REPLY: &str = "echo_reply";

/// The name that the schema records when a hop answered that it reaches no
/// destination.
const UNREACHABLE: &str = "unreachable";

/// The name that the schema records when the answer carried no ICMP message.
const NOT_APPLICABLE: &str = "not_applicable";

/// The ICMP message that answered one probe, in the words that `krt` owns.
///
/// The tracer names the same four messages, and three of its four variants
/// carry a code whose type its crate does not export. `krt` therefore keeps
/// this enum of its own. A test builds every variant of it and reads every
/// name back, so the whole name table is covered. [`icmp_kind`] is the one
/// step that a test cannot reach, and it holds no logic beyond one arm for
/// each variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IcmpKind {
    /// A hop below the target answered that the TTL of the probe ran out.
    TimeExceeded,
    /// The target answered the echo request of the probe.
    EchoReply,
    /// A hop answered that it reaches no destination.
    Unreachable,
    /// The answer carried no ICMP message. A UDP probe and a TCP probe both
    /// take such an answer.
    NotApplicable,
}

impl IcmpKind {
    /// The name that the schema records for this message.
    #[allow(
        unused_variables,
        reason = "the stub of the red step reads no message; the green step reads it"
    )]
    fn name(self) -> &'static str {
        NOT_APPLICABLE
    }
}

/// The ICMP message of one answer of the tracer, in the words that `krt` owns.
///
/// `trippy-core` does not export `IcmpPacketCode`. That type lives in a
/// private module of the crate, and the crate root re-exports
/// `IcmpPacketType` without it. Code outside `trippy-core` therefore matches
/// `TimeExceeded(_)` with a wildcard and cannot build one, so no test reaches
/// three of the four arms below.
///
/// The design answers that in two ways. This match holds one arm for each
/// variant and nothing else, so a reader checks the three arms that no test
/// reaches by eye. And every name that the schema records lives in
/// [`IcmpKind::name`], which `krt` owns, so a test builds all four of those
/// and reads each name back.
///
/// This fact about the API of the tracer is exactly what the wall of this
/// module exists to hold in one file.
fn icmp_kind(packet: IcmpPacketType) -> IcmpKind {
    match packet {
        IcmpPacketType::TimeExceeded(_) => IcmpKind::TimeExceeded,
        IcmpPacketType::EchoReply(_) => IcmpKind::EchoReply,
        IcmpPacketType::Unreachable(_) => IcmpKind::Unreachable,
        IcmpPacketType::NotApplicable => IcmpKind::NotApplicable,
    }
}

/// Converts one round of the tracer into an owned record.
///
/// The callback of the tracer holds the round for the length of one call, so
/// the conversion copies every value it keeps. Nothing borrowed leaves this
/// module.
///
/// `now` is a parameter and not a reading of the clock, so the conversion is
/// pure and a test drives every case of it.
///
/// `first_ttl` is the first TTL that the run probes. The last TTL of the range
/// is the largest TTL that answered, which the tracer reports and which
/// shrinks when the target moves closer, so a TTL beyond the end of the path
/// never counts as lost. A round that answered nothing reports zero there, and
/// the range then closes at the highest TTL that the round truly sent.
#[allow(
    dead_code,
    reason = "the tracer thread converts each round, and the tracer arrives in a later slice of issue #367"
)]
#[allow(
    unused_variables,
    reason = "the stub of the red step reads no round; the green step reads it"
)]
fn to_round_record(
    round: &Round<'_>,
    run: &RunId,
    seq: u64,
    now: DateTime<Utc>,
    first_ttl: u8,
) -> RoundRecord {
    RoundRecord {
        run: run.clone(),
        seq,
        ts: now,
        dur_ms: 0,
        ttl_range: TtlRange::from_first(first_ttl, first_ttl),
        reached: false,
        hops: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_privilege, icmp_kind, to_round_record, IcmpKind, PrivilegeError};
    use crate::record::{Privilege, Record, RoundRecord, RunId};
    use crate::PROGRAM;
    use chrono::{DateTime, Utc};
    use std::net::IpAddr;
    use std::time::{Duration, SystemTime};
    use trippy_core::{
        CompletionReason, Flags, IcmpPacketType, Port, Probe, ProbeComplete, ProbeStatus, Round,
        RoundId, Sequence, TimeToLive, TraceId,
    };

    /// The remedy, exactly as the design writes it.
    ///
    /// `main` writes every message as `krt: {reason}`, so the text carries no
    /// program name. The two lines under the first one carry two spaces each,
    /// and the remedy of each platform starts at the same column.
    const REMEDY: &str = "\
this platform needs raw socket privileges to send probes.
  Linux:   sudo setcap 'cap_net_raw+p' $(which krt)
  Windows: run krt from an elevated prompt";

    /// The first line that `main` writes for a platform without the privileges.
    const FIRST_LINE: &str = "krt: this platform needs raw socket privileges to send probes.";

    /// The reason of a platform that will not report what it holds.
    const A_REASON: &str = "the operating system refused the query";

    /// The mode of a run that the platform admits.
    fn mode(has: bool, needs: bool) -> Privilege {
        choose_privilege(has, needs).expect("the platform must admit a mode")
    }

    /// The fault of a platform that the gate stops.
    fn fault(has: bool, needs: bool) -> PrivilegeError {
        choose_privilege(has, needs).expect_err("the gate must stop the run")
    }

    #[test]
    fn a_platform_that_needs_no_privileges_runs_unprivileged() {
        assert_eq!(mode(false, false), Privilege::Unprivileged);
    }

    /// macOS needs no privileges, so a `sudo krt` on macOS still runs
    /// unprivileged. The design decides that case against the reflex: `krt`
    /// never changes the way it probes without a word.
    #[test]
    fn a_platform_that_needs_no_privileges_runs_unprivileged_even_with_them() {
        assert_eq!(mode(true, false), Privilege::Unprivileged);
    }

    #[test]
    fn a_platform_that_needs_privileges_and_holds_them_runs_privileged() {
        assert_eq!(mode(true, true), Privilege::Privileged);
    }

    #[test]
    fn a_platform_that_needs_privileges_and_holds_none_stops_the_run() {
        assert_eq!(fault(false, true), PrivilegeError::Missing);
    }

    #[test]
    fn the_message_of_a_missing_privilege_names_the_remedy_of_each_platform() {
        assert_eq!(PrivilegeError::Missing.to_string(), REMEDY);
    }

    #[test]
    fn the_line_that_main_writes_names_the_program_and_the_reason() {
        let error = PrivilegeError::Missing;
        let line = format!("{PROGRAM}: {error}");
        assert!(
            line.starts_with(FIRST_LINE),
            "the line names the program and the reason: {line}"
        );
    }

    #[test]
    fn a_platform_that_will_not_report_what_it_holds_names_the_reason() {
        let error = PrivilegeError::Discovery {
            reason: A_REASON.to_owned(),
        };
        let message = error.to_string();
        assert!(
            message.contains(A_REASON),
            "the message names the reason: {message}"
        );
    }

    // The conversion of one round of the tracer into one record. No test below
    // touches the network, and none of them needs a privilege. Each one builds
    // the probes of a round by hand.

    /// The identifier of the run that every test round belongs to.
    const RUN: &str = "2026-08-18T12:00:00.123Z";

    /// The number of the round that every test round carries.
    const SEQ: u64 = 142;

    /// The first TTL that every test run probes.
    const FIRST_TTL: u8 = 1;

    /// The largest TTL that a round of no answer reports.
    const NOTHING_ANSWERED: u8 = 0;

    /// The moment that the first probe of a test round leaves.
    const START: &str = "2026-08-18T12:34:56.789Z";

    /// A moment five milliseconds after `START`, for a probe that leaves later.
    const LATER: &str = "2026-08-18T12:34:56.794Z";

    /// The moment that a test converts a round at. It stands 1004 milliseconds
    /// after `START`.
    const NOW: &str = "2026-08-18T12:34:57.793Z";

    /// The milliseconds from `START` to `NOW`.
    const ROUND_DURATION_MS: u64 = 1004;

    /// The address of the first hop of a test round.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The address of the hop in the middle of a test path.
    const MIDDLE_HOP: &str = "10.0.0.1";

    /// The TTL of the hop in the middle of a test path.
    const MIDDLE_TTL: u8 = 7;

    /// The address of the target of a test round.
    const TARGET_ADDRESS: &str = "93.184.216.34";

    /// The TTL of the target of a test round.
    const TARGET_TTL: u8 = 14;

    /// The round trip time of the first hop of a test round, in microseconds.
    const FIRST_HOP_RTT_MICROS: u64 = 1230;

    /// The round trip time of the target of a test round, in microseconds.
    const TARGET_RTT_MICROS: u64 = 24_100;

    /// A round trip time of one millisecond and a half, in microseconds.
    const HALF_MILLISECOND_MORE_MICROS: u64 = 1500;

    /// The same round trip time, in milliseconds.
    const HALF_MILLISECOND_MORE_MS: f64 = 1.5;

    /// The sequence number that every test probe carries.
    const PROBE_SEQUENCE: u16 = 1;

    /// The trace identifier that every test probe carries.
    const PROBE_TRACE_ID: u16 = 4242;

    /// The source port that every test probe carries.
    const PROBE_SRC_PORT: u16 = 33_434;

    /// The destination port that every test probe carries.
    const PROBE_DEST_PORT: u16 = 80;

    /// The number of the round that every test probe belongs to.
    const PROBE_ROUND: usize = 1;

    /// The name that the schema records when a hop below the target answered
    /// that the TTL ran out.
    const TIME_EXCEEDED: &str = "time_exceeded";

    /// The name that the schema records when the target answered the echo
    /// request.
    const ECHO_REPLY: &str = "echo_reply";

    /// The name that the schema records when a hop answered that it reaches no
    /// destination.
    const UNREACHABLE: &str = "unreachable";

    /// The name that the schema records when the answer carried no ICMP
    /// message.
    const NOT_APPLICABLE: &str = "not_applicable";

    /// The one line that a converted round writes.
    ///
    /// Every hop of the line records `not_applicable`, because `trippy-core`
    /// does not export `IcmpPacketCode` and a test therefore builds no other
    /// kind of answer. `icmp_kind` names that limit, and the name table of
    /// `IcmpKind` covers the other three messages.
    const ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-18T12:00:00.123Z","seq":142,"ts":"2026-08-18T12:34:56.789Z","dur_ms":1004,"ttl_range":[1,14],"reached":true,"hops":[{"ttl":1,"addr":"192.168.1.1","rtt_ms":1.23,"icmp":"not_applicable"},{"ttl":14,"addr":"93.184.216.34","rtt_ms":24.1,"icmp":"not_applicable"}]}"#;

    /// Reads a moment that a test names, and converts it to UTC.
    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the test moment must parse")
            .with_timezone(&Utc)
    }

    /// The same moment, as the clock of the operating system reads it.
    fn clock(text: &str) -> SystemTime {
        SystemTime::from(utc(text))
    }

    /// Reads an address that a test names.
    fn address(text: &str) -> IpAddr {
        text.parse().expect("the test address must parse")
    }

    /// A probe that the round sent and that answered.
    ///
    /// The answer carries `IcmpPacketType::NotApplicable`, because
    /// `trippy-core` does not export `IcmpPacketCode` and each of the other
    /// three packet types carries one. No test builds them. See `icmp_kind`.
    fn completed(ttl: u8, addr: &str, sent: SystemTime, rtt: Duration) -> ProbeStatus {
        ProbeStatus::Complete(ProbeComplete {
            sequence: Sequence(PROBE_SEQUENCE),
            identifier: TraceId(PROBE_TRACE_ID),
            src_port: Port(PROBE_SRC_PORT),
            dest_port: Port(PROBE_DEST_PORT),
            ttl: TimeToLive(ttl),
            round: RoundId(PROBE_ROUND),
            sent,
            host: address(addr),
            received: sent + rtt,
            icmp_packet_type: IcmpPacketType::NotApplicable,
            tos: None,
            expected_udp_checksum: None,
            actual_udp_checksum: None,
            extensions: None,
        })
    }

    /// A probe that the round sent and that has no answer yet.
    fn awaited(ttl: u8, sent: SystemTime) -> ProbeStatus {
        ProbeStatus::Awaited(Probe {
            sequence: Sequence(PROBE_SEQUENCE),
            identifier: TraceId(PROBE_TRACE_ID),
            src_port: Port(PROBE_SRC_PORT),
            dest_port: Port(PROBE_DEST_PORT),
            ttl: TimeToLive(ttl),
            round: RoundId(PROBE_ROUND),
            sent,
            flags: Flags::empty(),
        })
    }

    /// The record of a round that a test built.
    fn record_at(
        probes: &[ProbeStatus],
        largest_ttl: u8,
        reason: CompletionReason,
        now: DateTime<Utc>,
    ) -> RoundRecord {
        let round = Round::new(probes, TimeToLive(largest_ttl), reason);
        to_round_record(&round, &RunId::from(RUN), SEQ, now, FIRST_TTL)
    }

    /// The record of a round that found the target, converted at `NOW`.
    fn record_of(probes: &[ProbeStatus], largest_ttl: u8) -> RoundRecord {
        record_at(probes, largest_ttl, CompletionReason::TargetFound, utc(NOW))
    }

    /// The record of a round that answered nothing, converted at `NOW`.
    fn a_silent_record(probes: &[ProbeStatus]) -> RoundRecord {
        record_at(
            probes,
            NOTHING_ANSWERED,
            CompletionReason::RoundTimeLimitExceeded,
            utc(NOW),
        )
    }

    /// One probe that answered, sent at `START`.
    fn an_answer(ttl: u8, addr: &str, rtt_micros: u64) -> ProbeStatus {
        completed(ttl, addr, clock(START), Duration::from_micros(rtt_micros))
    }

    /// The two probes of a round that answered the whole path: the first hop
    /// and the target.
    fn a_whole_path() -> [ProbeStatus; 2] {
        [
            an_answer(FIRST_TTL, FIRST_HOP, FIRST_HOP_RTT_MICROS),
            an_answer(TARGET_TTL, TARGET_ADDRESS, TARGET_RTT_MICROS),
        ]
    }

    /// The TTL of every hop of one record, in the order the record holds them.
    fn ttls_of(record: &RoundRecord) -> Vec<u8> {
        record.hops.iter().map(|hop| hop.ttl).collect()
    }

    /// The two numbers of the range of TTLs of one record.
    fn range_of(record: &RoundRecord) -> [u8; 2] {
        [record.ttl_range.first(), record.ttl_range.last()]
    }

    #[test]
    fn two_probes_that_answered_give_two_hops() {
        let record = record_of(&a_whole_path(), TARGET_TTL);
        assert_eq!(ttls_of(&record), [FIRST_TTL, TARGET_TTL]);
        assert_eq!(record.hops[0].addr, address(FIRST_HOP));
        assert_eq!(record.hops[1].addr, address(TARGET_ADDRESS));
        assert_eq!(record.hops[0].icmp, NOT_APPLICABLE);
        assert_eq!(record.hops[1].icmp, NOT_APPLICABLE);
    }

    /// The answers of a round arrive in the order the network returns them,
    /// and a record holds its hops in the order of the path.
    #[test]
    fn the_hops_come_out_in_the_order_of_the_path() {
        let probes = [
            an_answer(TARGET_TTL, TARGET_ADDRESS, TARGET_RTT_MICROS),
            an_answer(FIRST_TTL, FIRST_HOP, FIRST_HOP_RTT_MICROS),
            an_answer(MIDDLE_TTL, MIDDLE_HOP, FIRST_HOP_RTT_MICROS),
        ];
        let record = record_of(&probes, TARGET_TTL);
        assert_eq!(ttls_of(&record), [FIRST_TTL, MIDDLE_TTL, TARGET_TTL]);
    }

    /// The round trip time keeps the fraction of a millisecond. A conversion
    /// that read whole milliseconds records `1` here.
    #[test]
    fn the_round_trip_time_is_the_milliseconds_between_the_two_stamps() {
        let probes = [an_answer(
            FIRST_TTL,
            FIRST_HOP,
            HALF_MILLISECOND_MORE_MICROS,
        )];
        let record = record_of(&probes, FIRST_TTL);
        assert_eq!(ttls_of(&record), [FIRST_TTL]);
        assert_eq!(record.hops[0].rtt_ms, HALF_MILLISECOND_MORE_MS);
    }

    #[test]
    fn a_probe_that_awaits_an_answer_gives_no_hop() {
        let probes = [
            an_answer(FIRST_TTL, FIRST_HOP, FIRST_HOP_RTT_MICROS),
            awaited(MIDDLE_TTL, clock(START)),
        ];
        let record = record_of(&probes, FIRST_TTL);
        assert_eq!(
            ttls_of(&record),
            [FIRST_TTL],
            "the awaited probe answered nothing"
        );
    }

    #[test]
    fn a_probe_that_the_round_did_not_send_gives_no_hop() {
        let probes = [
            an_answer(FIRST_TTL, FIRST_HOP, FIRST_HOP_RTT_MICROS),
            ProbeStatus::NotSent,
        ];
        let record = record_of(&probes, FIRST_TTL);
        assert_eq!(
            ttls_of(&record),
            [FIRST_TTL],
            "a probe that never left answered nothing"
        );
    }

    #[test]
    fn the_ttl_range_ends_at_the_largest_ttl_that_answered() {
        let record = record_of(&a_whole_path(), TARGET_TTL);
        assert_eq!(range_of(&record), [FIRST_TTL, TARGET_TTL]);
    }

    /// A round that answered nothing reports a largest TTL of zero. The range
    /// then closes at the highest TTL that the round truly sent, so no TTL
    /// beyond the probes of the round counts as lost.
    #[test]
    fn the_ttl_range_falls_back_to_the_highest_ttl_that_the_round_sent() {
        let probes = [
            awaited(FIRST_TTL, clock(START)),
            awaited(2, clock(START)),
            awaited(3, clock(START)),
        ];
        let record = a_silent_record(&probes);
        assert_eq!(range_of(&record), [FIRST_TTL, 3]);
    }

    /// A probe that never left carries no TTL of its own, and the place it
    /// holds in the round is not one. A conversion that counted places records
    /// a range of four TTLs here.
    #[test]
    fn a_probe_that_the_round_did_not_send_never_widens_the_ttl_range() {
        let probes = [
            awaited(FIRST_TTL, clock(START)),
            awaited(2, clock(START)),
            ProbeStatus::NotSent,
            ProbeStatus::NotSent,
        ];
        let record = a_silent_record(&probes);
        assert_eq!(range_of(&record), [FIRST_TTL, 2]);
    }

    #[test]
    fn a_round_that_sent_nothing_probed_the_first_ttl_only() {
        let record = a_silent_record(&[ProbeStatus::NotSent, ProbeStatus::NotSent]);
        assert_eq!(range_of(&record), [FIRST_TTL, FIRST_TTL]);
    }

    #[test]
    fn a_round_that_found_the_target_reached_it() {
        let record = record_at(
            &a_whole_path(),
            TARGET_TTL,
            CompletionReason::TargetFound,
            utc(NOW),
        );
        assert!(record.reached, "the round found the target");
    }

    #[test]
    fn a_round_that_ran_out_of_time_reached_nothing() {
        let record = record_at(
            &a_whole_path(),
            TARGET_TTL,
            CompletionReason::RoundTimeLimitExceeded,
            utc(NOW),
        );
        assert!(!record.reached, "the round ran out of time");
    }

    #[test]
    fn the_moment_of_a_round_is_the_moment_the_first_probe_left() {
        let probes = [
            completed(
                TARGET_TTL,
                TARGET_ADDRESS,
                clock(LATER),
                Duration::from_micros(TARGET_RTT_MICROS),
            ),
            an_answer(FIRST_TTL, FIRST_HOP, FIRST_HOP_RTT_MICROS),
        ];
        let record = record_of(&probes, TARGET_TTL);
        assert_eq!(record.ts, utc(START));
    }

    #[test]
    fn the_duration_of_a_round_is_the_milliseconds_from_its_moment_to_now() {
        let record = record_of(&a_whole_path(), TARGET_TTL);
        assert_eq!(record.ts, utc(START));
        assert_eq!(record.dur_ms, ROUND_DURATION_MS);
    }

    #[test]
    fn a_round_of_no_probe_holds_the_moment_of_the_record_and_no_hop() {
        let record = a_silent_record(&[]);
        assert_eq!(record.ts, utc(NOW));
        assert_eq!(record.dur_ms, 0);
        assert!(
            record.hops.is_empty(),
            "the round holds no probe: {:?}",
            record.hops
        );
        assert_eq!(range_of(&record), [FIRST_TTL, FIRST_TTL]);
    }

    #[test]
    fn the_record_names_the_run_and_the_number_of_the_round() {
        let record = record_of(&a_whole_path(), TARGET_TTL);
        assert_eq!(record.run, RunId::from(RUN));
        assert_eq!(record.seq, SEQ);
    }

    #[test]
    fn the_name_table_holds_one_name_for_each_icmp_message() {
        for (kind, name) in [
            (IcmpKind::TimeExceeded, TIME_EXCEEDED),
            (IcmpKind::EchoReply, ECHO_REPLY),
            (IcmpKind::Unreachable, UNREACHABLE),
            (IcmpKind::NotApplicable, NOT_APPLICABLE),
        ] {
            assert_eq!(kind.name(), name, "{kind:?}");
        }
    }

    /// The one arm of `icmp_kind` that a test reaches. `trippy-core` does not
    /// export `IcmpPacketCode`, so no test builds the other three packet
    /// types.
    #[test]
    fn an_answer_that_carries_no_icmp_message_records_not_applicable() {
        assert_eq!(
            icmp_kind(IcmpPacketType::NotApplicable),
            IcmpKind::NotApplicable
        );
        let record = record_of(&a_whole_path(), TARGET_TTL);
        assert_eq!(ttls_of(&record), [FIRST_TTL, TARGET_TTL]);
        for hop in &record.hops {
            assert_eq!(hop.icmp, NOT_APPLICABLE);
        }
    }

    /// The whole conversion writes one line of the schema. A record that
    /// drifts from the schema then fails a test, and not a reader of a
    /// recorded file.
    #[test]
    fn a_converted_round_writes_one_line_of_the_schema() {
        let record = Record::Round(record_of(&a_whole_path(), TARGET_TTL));
        assert_eq!(
            record.to_line().expect("the record must become one line"),
            ROUND_LINE
        );
    }
}
