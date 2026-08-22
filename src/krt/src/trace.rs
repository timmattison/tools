//! The privilege gate of a run, the wall in front of the tracer, the tracer
//! that one configuration starts, and the conversion of one round into one
//! record.
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
//! The interface of the wall is one type and one function. [`TraceConfig`]
//! states one run in the words that `krt` owns, and [`spawn`] starts the
//! tracer of that run and gives back a receiver of completed rounds. A caller
//! outside this module therefore names no type of a trippy crate.
//!
//! The conversion is the wall itself. The tracer hands one round over as a
//! borrowed value that lives for the length of one call, and
//! [`to_round_record`] copies every value that the record keeps. Nothing
//! borrowed crosses out of this module, so a later change of engine touches
//! this file alone. The run loop that reads the receiver arrives in a later
//! slice.

use crate::record::{self, Hop, RoundRecord, RunId, TtlRange};
use chrono::{DateTime, Utc};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};
use trippy_core::{CompletionReason, IcmpPacketType, ProbeStatus, Round};

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

/// The source port that a UDP trace holds while the destination port varies.
///
/// A fixed source port to a varying destination port is the direction of a UDP
/// trace, and 33434 is the first port of the range that traceroute probes.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const UDP_SOURCE_PORT: u16 = 33_434;

/// The destination port that a TCP trace holds while the source port varies.
///
/// A varying source port to a fixed destination port is the direction of a TCP
/// trace, and 80 is the port of HTTP.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const TCP_DESTINATION_PORT: u16 = 80;

/// The number of the first round of a run. The schema counts from one.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const FIRST_ROUND: u64 = 1;

/// The configuration of one tracing run, in the words that `krt` owns.
///
/// No field holds a type of a trippy crate, so a caller states a whole run
/// from outside the wall of this module.
#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
pub(crate) struct TraceConfig {
    /// The address to probe. `main` resolves the destination of the command
    /// line to this address.
    pub(crate) target: IpAddr,
    /// The identifier of the run that every round record carries.
    pub(crate) run: RunId,
    /// The period of one round.
    pub(crate) interval: Duration,
    /// The first TTL that the run probes.
    pub(crate) first_ttl: u8,
    /// The last TTL that the run probes.
    pub(crate) max_ttl: u8,
    /// The protocol of a probe.
    pub(crate) protocol: crate::Protocol,
    /// The way a probe keeps or varies the flow of a packet.
    pub(crate) multipath: crate::Multipath,
    /// The privilege mode of the run.
    pub(crate) privilege: record::Privilege,
}

/// Why the tracer of a run does not start.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum TraceError {
    /// The tracer refused the configuration of the run.
    #[error("the tracer refused the configuration: {reason}")]
    Build {
        /// The reason that the tracer gave.
        reason: String,
    },
    /// The thread of the tracer did not start.
    #[error("the thread of the tracer did not start: {reason}")]
    Spawn {
        /// The reason that the platform gave.
        reason: String,
    },
}

/// Builds the tracer of one run from the configuration of `krt`.
///
/// # Errors
///
/// Returns [`TraceError::Build`] when the tracer refuses the configuration.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
fn tracer_of(config: &TraceConfig) -> Result<trippy_core::Tracer, TraceError> {
    trippy_core::Builder::new(config.target)
        .build()
        .map_err(|error| TraceError::Build {
            reason: error.to_string(),
        })
}

/// The number of the next round. The first round of a run is round one.
///
/// The callback of the tracer is `Fn` and not `FnMut`, so the count of the
/// rounds lives in an atomic and not in a number of the closure.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
fn next_seq(counter: &AtomicU64) -> u64 {
    counter.fetch_add(1, Ordering::Relaxed)
}

/// Starts the tracer on its own thread and gives back a receiver of completed
/// rounds.
///
/// The callback of the tracer holds a borrowed round. It converts that round
/// into an owned record before it sends, so nothing borrowed crosses the
/// channel and a later engine swap touches this file only.
///
/// The channel is unbounded, so a slow reader never stalls the tracer thread.
/// The tracer holds no way to stop, so its thread ends when the process ends.
///
/// # Errors
///
/// Returns [`TraceError::Build`] when the tracer refuses the configuration, and
/// [`TraceError::Spawn`] when the thread does not start.
#[allow(
    dead_code,
    reason = "main starts the tracer beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
pub(crate) fn spawn(config: &TraceConfig) -> Result<Receiver<RoundRecord>, TraceError> {
    let tracer = tracer_of(config)?;
    let (sender, receiver) = mpsc::channel();
    let run = config.run.clone();
    let first_ttl = config.first_ttl;
    let counter = AtomicU64::new(0);
    // `spawn_with` gives back the tracer and the handle of its thread. The run
    // loop reads a closed channel as a dead tracer, so it needs neither.
    let (_tracer, _thread) = tracer
        .spawn_with(move |round| {
            let record = to_round_record(round, &run, next_seq(&counter), Utc::now(), first_ttl);
            // A failed send means the reader of the channel is gone, and the
            // run is over. The thread ends when the process ends.
            drop(sender.send(record));
        })
        .map_err(|error| TraceError::Spawn {
            reason: error.to_string(),
        })?;
    Ok(receiver)
}

/// The number of milliseconds in one second.
///
/// `Duration::as_millis_f64` is not stable in the pinned toolchain, so the
/// conversion reads a duration as a fraction of a second and scales it by this
/// number.
const MILLIS_PER_SECOND: f64 = 1000.0;

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
    fn name(self) -> &'static str {
        match self {
            Self::TimeExceeded => TIME_EXCEEDED,
            Self::EchoReply => ECHO_REPLY,
            Self::Unreachable => UNREACHABLE,
            Self::NotApplicable => NOT_APPLICABLE,
        }
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
fn to_round_record(
    round: &Round<'_>,
    run: &RunId,
    seq: u64,
    now: DateTime<Utc>,
    first_ttl: u8,
) -> RoundRecord {
    let mut hops: Vec<Hop> = round.probes.iter().filter_map(to_hop).collect();
    // The answers of a round arrive in the order the network returns them, and
    // a record holds its hops in the order of the path.
    hops.sort_by_key(|hop| hop.ttl);

    let last_ttl = if round.largest_ttl.0 >= first_ttl {
        round.largest_ttl.0
    } else {
        round
            .probes
            .iter()
            .filter_map(sent_probe)
            .map(|probe| probe.ttl)
            .max()
            .unwrap_or(first_ttl)
    };

    let ts = round
        .probes
        .iter()
        .filter_map(sent_probe)
        .map(|probe| probe.sent)
        .min()
        .map_or(now, DateTime::<Utc>::from);
    // A difference below zero names a `now` before the first probe left, which
    // one round of one clock never gives. A record holds no negative duration,
    // so such a difference records zero.
    let dur_ms = u64::try_from((now - ts).num_milliseconds()).unwrap_or_default();

    // A `match` and not a `matches!`, so a new reason of the tracer breaks
    // this one file rather than recording `false` without a word.
    let reached = match round.reason {
        CompletionReason::TargetFound => true,
        CompletionReason::RoundTimeLimitExceeded => false,
    };

    RoundRecord {
        run: run.clone(),
        seq,
        ts,
        dur_ms,
        ttl_range: TtlRange::from_first(first_ttl, last_ttl),
        reached,
        hops,
    }
}

/// The hop that one status of the tracer records. A status that no hop
/// answered records none.
///
/// The schema holds one hop for each answer, so a probe that the round never
/// sent, that it skipped, that failed, and that still waits each record
/// nothing. `ttl_range` states which TTLs the round probed, so a reader still
/// parts a hop that did not answer from a TTL that the round never probed.
#[allow(
    dead_code,
    reason = "the tracer thread converts each round, and the tracer arrives in a later slice of issue #367"
)]
fn to_hop(status: &ProbeStatus) -> Option<Hop> {
    match status {
        ProbeStatus::Complete(probe) => Some(Hop {
            ttl: probe.ttl.0,
            addr: probe.host,
            rtt_ms: rtt_millis(probe.sent, probe.received),
            icmp: icmp_kind(probe.icmp_packet_type).name().to_owned(),
        }),
        ProbeStatus::NotSent
        | ProbeStatus::Skipped
        | ProbeStatus::Failed(_)
        | ProbeStatus::Awaited(_) => None,
    }
}

/// One probe that a round put on the wire.
///
/// A round that answered nothing still states which TTLs it probed, and the
/// probes that left are the answer. This value carries the two facts that such
/// a round reads from one of them.
#[derive(Debug, Clone, Copy)]
#[allow(
    dead_code,
    reason = "the tracer thread converts each round, and the tracer arrives in a later slice of issue #367"
)]
struct SentProbe {
    /// The TTL that the probe carried.
    ttl: u8,
    /// The moment that the probe left.
    sent: SystemTime,
}

/// The probe that one status of the tracer put on the wire. A status that put
/// none there gives `None`.
///
/// A probe left when the round awaits its answer, and when the answer already
/// arrived. A probe that the round never sent, that it skipped, and that
/// failed each left nothing on the wire, so none of them widens the range of
/// TTLs and none of them invents a hop that was lost.
#[allow(
    dead_code,
    reason = "the tracer thread converts each round, and the tracer arrives in a later slice of issue #367"
)]
fn sent_probe(status: &ProbeStatus) -> Option<SentProbe> {
    match status {
        ProbeStatus::Awaited(probe) => Some(SentProbe {
            ttl: probe.ttl.0,
            sent: probe.sent,
        }),
        ProbeStatus::Complete(probe) => Some(SentProbe {
            ttl: probe.ttl.0,
            sent: probe.sent,
        }),
        ProbeStatus::NotSent | ProbeStatus::Skipped | ProbeStatus::Failed(_) => None,
    }
}

/// The round trip time of one answer, in milliseconds.
///
/// An answer that the clock stamps before its probe left gives zero. The clock
/// of the operating system steps when the machine corrects its time, so the
/// two stamps of one probe can run backward, and a record holds no negative
/// round trip time.
#[allow(
    dead_code,
    reason = "the tracer thread converts each round, and the tracer arrives in a later slice of issue #367"
)]
fn rtt_millis(sent: SystemTime, received: SystemTime) -> f64 {
    received
        .duration_since(sent)
        .unwrap_or_default()
        .as_secs_f64()
        * MILLIS_PER_SECOND
}

#[cfg(test)]
mod tests {
    use super::{
        choose_privilege, icmp_kind, next_seq, to_round_record, tracer_of, IcmpKind,
        PrivilegeError, TraceConfig, TraceError,
    };
    use crate::record::{Privilege, Record, RoundRecord, RunId};
    use crate::{Multipath, Protocol, PROGRAM};
    use chrono::{DateTime, Utc};
    use std::net::IpAddr;
    use std::sync::atomic::AtomicU64;
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
    #[allow(
        clippy::float_cmp,
        reason = "1500 microseconds is 1.5 milliseconds, and both numbers are exact in binary, so the conversion gives this one and no other"
    )]
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

    // The tracer that one configuration builds. No test below touches the
    // network, and none of them needs a privilege. `Builder::build` reads the
    // configuration and builds a `Tracer`, and it opens no socket: the channel
    // of a trace opens in `run`, which `build` never calls. No test calls
    // `spawn`, because that call starts a thread that probes.
    //
    // Every value of the test configuration differs from the value that the
    // tracer holds by default, so a mapping that dropped the field fails here
    // rather than reading as a pass.

    /// The period of one round of a test run.
    const A_ROUND_PERIOD: Duration = Duration::from_millis(250);

    /// The first TTL that a test run probes.
    const A_FIRST_TTL: u8 = 2;

    /// The last TTL that a test run probes.
    const A_MAX_TTL: u8 = 20;

    /// The source port that a UDP trace holds while the destination port
    /// varies. It is the first port of the range that traceroute probes.
    const UDP_SOURCE_PORT: u16 = 33_434;

    /// The destination port that a TCP trace holds while the source port
    /// varies. It is the port of HTTP.
    const TCP_DESTINATION_PORT: u16 = 80;

    /// A last TTL above the largest one that the tracer takes.
    ///
    /// The command line of `krt` accepts a TTL up to 255, and `trippy_core`
    /// stops at `MAX_TTL`, which is 254. Such a run therefore reaches the
    /// build and the build refuses it.
    const A_TTL_THAT_THE_TRACER_REFUSES: u8 = 255;

    /// The configuration of a test run. Each test changes the one field that
    /// it reads.
    fn a_config() -> TraceConfig {
        TraceConfig {
            target: address(TARGET_ADDRESS),
            run: RunId::from(RUN),
            interval: A_ROUND_PERIOD,
            first_ttl: A_FIRST_TTL,
            max_ttl: A_MAX_TTL,
            protocol: Protocol::Icmp,
            multipath: Multipath::Classic,
            privilege: Privilege::Unprivileged,
        }
    }

    /// The tracer that one configuration builds.
    fn tracer_from(config: &TraceConfig) -> trippy_core::Tracer {
        tracer_of(config).expect("the tracer must take the configuration")
    }

    /// The tracer of a run that probes with this protocol.
    fn tracer_of_protocol(protocol: Protocol) -> trippy_core::Tracer {
        tracer_from(&TraceConfig {
            protocol,
            ..a_config()
        })
    }

    #[test]
    fn the_target_of_the_configuration_reaches_the_tracer() {
        assert_eq!(
            tracer_from(&a_config()).target_addr(),
            address(TARGET_ADDRESS)
        );
    }

    /// The round period of the tracer is the window between its shortest round
    /// and its longest one. `krt` names one period, so both ends take it.
    #[test]
    fn the_interval_becomes_the_shortest_round_and_the_longest_round() {
        let tracer = tracer_from(&a_config());
        assert_eq!(tracer.min_round_duration(), A_ROUND_PERIOD);
        assert_eq!(tracer.max_round_duration(), A_ROUND_PERIOD);
    }

    #[test]
    fn the_first_ttl_and_the_last_ttl_reach_the_tracer() {
        let tracer = tracer_from(&a_config());
        assert_eq!(tracer.first_ttl().0, A_FIRST_TTL);
        assert_eq!(tracer.max_ttl().0, A_MAX_TTL);
    }

    #[test]
    fn every_protocol_reaches_the_tracer() {
        for (protocol, expected) in [
            (Protocol::Icmp, trippy_core::Protocol::Icmp),
            (Protocol::Udp, trippy_core::Protocol::Udp),
            (Protocol::Tcp, trippy_core::Protocol::Tcp),
        ] {
            assert_eq!(
                tracer_of_protocol(protocol).protocol(),
                expected,
                "{protocol:?}"
            );
        }
    }

    /// The command line refuses a multipath mode other than `classic` beside
    /// ICMP, because ICMP carries no flow to vary. Each run below therefore
    /// probes with UDP.
    #[test]
    fn every_multipath_mode_reaches_the_tracer() {
        for (multipath, expected) in [
            (Multipath::Classic, trippy_core::MultipathStrategy::Classic),
            (Multipath::Paris, trippy_core::MultipathStrategy::Paris),
            (Multipath::Dublin, trippy_core::MultipathStrategy::Dublin),
        ] {
            let config = TraceConfig {
                protocol: Protocol::Udp,
                multipath,
                ..a_config()
            };
            assert_eq!(
                tracer_from(&config).multipath_strategy(),
                expected,
                "{multipath:?}"
            );
        }
    }

    #[test]
    fn every_privilege_mode_reaches_the_tracer() {
        for (privilege, expected) in [
            (
                Privilege::Unprivileged,
                trippy_core::PrivilegeMode::Unprivileged,
            ),
            (
                Privilege::Privileged,
                trippy_core::PrivilegeMode::Privileged,
            ),
        ] {
            let config = TraceConfig {
                privilege,
                ..a_config()
            };
            assert_eq!(
                tracer_from(&config).privilege_mode(),
                expected,
                "{privilege:?}"
            );
        }
    }

    #[test]
    fn an_icmp_trace_carries_no_port_direction() {
        assert_eq!(
            tracer_of_protocol(Protocol::Icmp).port_direction(),
            trippy_core::PortDirection::None
        );
    }

    /// `Builder::build` refuses a UDP trace whose port direction is `None`,
    /// and `None` is the direction that the builder holds by default, so this
    /// run reaches a tracer only because the mapping names a direction.
    #[test]
    fn a_udp_trace_fixes_the_source_port() {
        assert_eq!(
            tracer_of_protocol(Protocol::Udp).port_direction(),
            trippy_core::PortDirection::FixedSrc(Port(UDP_SOURCE_PORT))
        );
    }

    /// `Builder::build` refuses a TCP trace whose port direction is `None`, as
    /// it refuses such a UDP trace.
    #[test]
    fn a_tcp_trace_fixes_the_destination_port() {
        assert_eq!(
            tracer_of_protocol(Protocol::Tcp).port_direction(),
            trippy_core::PortDirection::FixedDest(Port(TCP_DESTINATION_PORT))
        );
    }

    /// `krt` owns the round limit and the time limit, and the run loop enforces
    /// them. A tracer that stopped itself would close the channel, and the run
    /// loop reads a closed channel as a dead tracer.
    #[test]
    fn the_tracer_holds_no_round_limit() {
        let limit = tracer_from(&a_config()).max_rounds();
        assert!(limit.is_none(), "the tracer holds a round limit: {limit:?}");
    }

    #[test]
    fn a_configuration_that_the_tracer_refuses_names_the_reason() {
        let config = TraceConfig {
            max_ttl: A_TTL_THAT_THE_TRACER_REFUSES,
            ..a_config()
        };
        let error = tracer_of(&config).expect_err("the tracer must refuse this last TTL");
        assert!(
            matches!(error, TraceError::Build { .. }),
            "the build of the tracer refused it: {error:?}"
        );
        let message = error.to_string();
        for part in [
            A_TTL_THAT_THE_TRACER_REFUSES.to_string(),
            trippy_core::MAX_TTL.to_string(),
        ] {
            assert!(
                message.contains(&part),
                "the message names `{part}`: {message}"
            );
        }
    }

    /// The schema says that the first round of a run is round one.
    #[test]
    fn the_first_round_of_a_run_is_round_one() {
        let counter = AtomicU64::new(0);
        assert_eq!(next_seq(&counter), 1);
        assert_eq!(next_seq(&counter), 2);
    }
}
