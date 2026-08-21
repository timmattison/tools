//! The records of a recorded file, and the one line that each record writes.
//!
//! A recorded file holds one JSON object per line. The `type` field names the
//! record, and every record carries the identifier of the run it belongs to.
//! This slice builds the records and the two functions that turn a record into
//! one line and back. The reader, the writer, and the `replay` command arrive
//! in the next slices.

// Nothing in `main.rs` reads these items yet.
#![allow(
    dead_code,
    reason = "the reader, the writer, and the replay command arrive in the next slices of issue #366"
)]

use crate::{Multipath, Protocol};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;

/// Writes a moment as RFC 3339, to the millisecond, in UTC.
///
/// `2026-08-18T13:00:00.000Z` keeps its three digits, so every line of a file
/// carries the same width, and a reader who opens the file by hand reads one
/// shape.
fn format_millis(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Reads and writes the timestamp of a record.
///
/// The writer holds to `format_millis`. The reader takes any RFC 3339 text and
/// converts it to UTC, so a file that another tool wrote still loads.
mod rfc3339_millis {
    use super::format_millis;
    use chrono::{DateTime, Utc};
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Writes the moment as RFC 3339, to the millisecond, in UTC.
    ///
    /// # Errors
    ///
    /// Returns the reason when the serializer refuses the text.
    pub(super) fn serialize<S>(ts: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format_millis(*ts))
    }

    /// Reads a moment from RFC 3339 text, and converts it to UTC.
    ///
    /// # Errors
    ///
    /// Returns the reason when the value is not text, and when the text is not
    /// RFC 3339.
    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|moment| moment.with_timezone(&Utc))
            .map_err(D::Error::custom)
    }
}

/// The identifier of one run: the RFC 3339 start time, to the millisecond, in
/// UTC.
///
/// The identifier is text, and every comparison is an exact text comparison, so
/// a run keeps the identifier that the file holds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RunId(String);

impl RunId {
    /// Builds the identifier of the run that starts at this moment.
    pub(crate) fn at(start: DateTime<Utc>) -> Self {
        Self(format_millis(start))
    }

    /// Reads the identifier as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for RunId {
    fn from(text: &str) -> Self {
        Self(text.to_owned())
    }
}

/// One line of a recorded file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Record {
    /// The record that opens a run.
    Run(RunRecord),
    /// The name of one address, from reverse DNS.
    Name(NameRecord),
    /// One round of probes.
    Round(RoundRecord),
    /// The record that closes a run.
    End(EndRecord),
    /// A `type` value that this build does not know.
    ///
    /// A reader skips such a line, so `from_line` reports it as `None`.
    #[serde(other)]
    Unknown,
}

impl Record {
    /// Writes the record as one line of a recorded file. The text carries no
    /// newline.
    ///
    /// # Errors
    ///
    /// Returns the reason when the record does not become JSON.
    pub(crate) fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Reads one line of a recorded file.
    ///
    /// `None` names a `type` that this build does not know. The reader skips
    /// such a line, so no caller sees it.
    ///
    /// # Errors
    ///
    /// Returns the reason when the line is not JSON, when the line names no
    /// `type`, and when a known record does not parse.
    pub(crate) fn from_line(line: &str) -> Result<Option<Record>, serde_json::Error> {
        match serde_json::from_str(line)? {
            Record::Unknown => Ok(None),
            known => Ok(Some(known)),
        }
    }

    /// The identifier of the run that this record belongs to. An unknown record
    /// has none.
    pub(crate) fn run_id(&self) -> Option<&RunId> {
        match self {
            Self::Run(record) => Some(&record.run),
            Self::Name(record) => Some(&record.run),
            Self::Round(record) => Some(&record.run),
            Self::End(record) => Some(&record.run),
            Self::Unknown => None,
        }
    }
}

/// The record that opens a run. It states what the run traces, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunRecord {
    /// The identifier of the run.
    pub(crate) run: RunId,
    /// The build string of the `krt` that made the run.
    pub(crate) krt: String,
    /// The address that the probes leave from.
    pub(crate) source: SourceLabel,
    /// The destination of the run.
    pub(crate) target: Target,
    /// The configuration of the run.
    pub(crate) config: RunConfig,
    /// The name of the machine that made the run.
    pub(crate) host: String,
}

/// The source address of a run, and how `krt` found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceLabel {
    /// The source address.
    pub(crate) addr: IpAddr,
    /// How `krt` found the address.
    pub(crate) kind: SourceKind,
}

/// How `krt` found the source address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    /// The user named the address on the command line.
    Override,
    /// The address is the public address of the machine.
    Public,
    /// The address is an address of a local interface.
    Local,
}

/// The destination of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Target {
    /// The destination as the user typed it.
    pub(crate) arg: String,
    /// The address that the destination resolved to.
    pub(crate) addr: IpAddr,
    /// The IP version of the address.
    pub(crate) family: Family,
}

/// The IP version that a run resolved to.
///
/// `AddressFamily` in `main.rs` is the version that the user asked for, and it
/// admits `auto`. This one is the answer, so it admits no `auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Family {
    /// IP version 4.
    Ipv4,
    /// IP version 6.
    Ipv6,
}

/// The configuration of one run, as the run records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunConfig {
    /// The period of one round, in milliseconds.
    pub(crate) interval_ms: u64,
    /// The protocol of a probe.
    pub(crate) protocol: Protocol,
    /// The first TTL that the run probes.
    pub(crate) first_ttl: u8,
    /// The last TTL that the run probes.
    pub(crate) max_ttl: u8,
    /// The way a probe keeps or varies the flow of a packet.
    pub(crate) multipath: Multipath,
    /// The privilege mode of the run.
    pub(crate) privilege: Privilege,
    /// True when the run reads the name of each hop.
    pub(crate) dns: bool,
}

/// The privilege mode of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Privilege {
    /// The run sends its probes through a datagram socket.
    Unprivileged,
    /// The run sends its probes through a raw socket.
    Privileged,
}

/// The name of one address, from reverse DNS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NameRecord {
    /// The identifier of the run.
    pub(crate) run: RunId,
    /// The moment that the name arrived.
    #[serde(with = "rfc3339_millis")]
    pub(crate) ts: DateTime<Utc>,
    /// The address that the name belongs to.
    pub(crate) addr: IpAddr,
    /// The name of the address.
    pub(crate) host: String,
}

/// One round of probes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoundRecord {
    /// The identifier of the run.
    pub(crate) run: RunId,
    /// The number of the round. The first round of a run is round one.
    pub(crate) seq: u64,
    /// The moment that the round started.
    #[serde(with = "rfc3339_millis")]
    pub(crate) ts: DateTime<Utc>,
    /// The time that the round took, in milliseconds.
    pub(crate) dur_ms: u64,
    /// The TTLs that the round probed.
    pub(crate) ttl_range: TtlRange,
    /// True when one probe of the round reached the target.
    pub(crate) reached: bool,
    /// The hops that answered.
    ///
    /// A hop that did not answer is absent, and `ttl_range` states which TTLs
    /// the round probed.
    pub(crate) hops: Vec<Hop>,
}

/// One hop that answered a probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Hop {
    /// The TTL of the probe that this hop answered.
    pub(crate) ttl: u8,
    /// The address of the hop.
    pub(crate) addr: IpAddr,
    /// The round trip time in milliseconds.
    pub(crate) rtt_ms: f64,
    /// The name of the ICMP message that answered.
    pub(crate) icmp: String,
}

/// The TTLs that one round probed, as the two numbers of a JSON array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "[u8; 2]", into = "[u8; 2]")]
pub(crate) struct TtlRange {
    /// The first TTL of the round.
    first: u8,
    /// The last TTL of the round.
    last: u8,
}

impl TtlRange {
    /// Builds the range of the TTLs that one round probed.
    ///
    /// # Errors
    ///
    /// Returns the reason when the first TTL is above the last one.
    pub(crate) fn new(first: u8, last: u8) -> Result<Self, TtlRangeError> {
        if first > last {
            return Err(TtlRangeError { first, last });
        }
        Ok(Self { first, last })
    }

    /// The first TTL of the round.
    pub(crate) fn first(self) -> u8 {
        self.first
    }

    /// The last TTL of the round.
    pub(crate) fn last(self) -> u8 {
        self.last
    }

    /// True when the round probed this TTL.
    pub(crate) fn contains(self, ttl: u8) -> bool {
        (self.first..=self.last).contains(&ttl)
    }
}

impl TryFrom<[u8; 2]> for TtlRange {
    type Error = TtlRangeError;

    fn try_from(pair: [u8; 2]) -> Result<Self, Self::Error> {
        let [first, last] = pair;
        Self::new(first, last)
    }
}

impl From<TtlRange> for [u8; 2] {
    fn from(range: TtlRange) -> Self {
        [range.first, range.last]
    }
}

/// The fault of a range of TTLs that runs backward.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the ttl range [{first}, {last}] runs backward: the first ttl is above the last one")]
pub(crate) struct TtlRangeError {
    /// The first TTL of the range.
    first: u8,
    /// The last TTL of the range.
    last: u8,
}

/// The record that closes a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EndRecord {
    /// The identifier of the run.
    pub(crate) run: RunId,
    /// The moment that the run stopped.
    #[serde(with = "rfc3339_millis")]
    pub(crate) ts: DateTime<Utc>,
    /// The number of rounds that the run made.
    pub(crate) rounds: u64,
    /// Why the run stopped.
    pub(crate) reason: EndReason,
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EndReason {
    /// The user stopped the run.
    Quit,
    /// The run reached the time limit.
    Duration,
    /// The run reached the round limit.
    Rounds,
    /// A fault stopped the run.
    Error,
}

#[cfg(test)]
mod tests {
    use super::{
        EndReason, EndRecord, Family, Hop, NameRecord, Privilege, Record, RoundRecord, RunConfig,
        RunId, RunRecord, SourceKind, SourceLabel, Target, TtlRange,
    };
    use crate::{Multipath, Protocol};
    use chrono::{DateTime, Utc};
    use std::net::IpAddr;

    /// The identifier of the run that every test record belongs to.
    const RUN: &str = "2026-08-18T12:00:00.123Z";

    /// The address of the first hop of every test round.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The address of the target of every test record.
    const TARGET_ADDRESS: &str = "93.184.216.34";

    /// The `run` line, as the design writes it.
    const RUN_LINE: &str = r#"{"type":"run","run":"2026-08-18T12:00:00.123Z","krt":"0.1.0 (abc1234, clean)","source":{"addr":"1.2.3.4","kind":"public"},"target":{"arg":"example.com","addr":"93.184.216.34","family":"ipv4"},"config":{"interval_ms":1000,"protocol":"icmp","first_ttl":1,"max_ttl":30,"multipath":"classic","privilege":"unprivileged","dns":true},"host":"tims-mac"}"#;

    /// The `name` line, as the design writes it.
    const NAME_LINE: &str = r#"{"type":"name","run":"2026-08-18T12:00:00.123Z","ts":"2026-08-18T12:00:02.001Z","addr":"192.168.1.1","host":"router.lan"}"#;

    /// The `round` line, as the design writes it.
    ///
    /// The design writes the round trip time of the last hop as `24.10`, and
    /// `serde_json` writes the same number as `24.1`.
    const ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-18T12:00:00.123Z","seq":142,"ts":"2026-08-18T12:34:56.789Z","dur_ms":1004,"ttl_range":[1,14],"reached":true,"hops":[{"ttl":1,"addr":"192.168.1.1","rtt_ms":1.23,"icmp":"time_exceeded"},{"ttl":14,"addr":"93.184.216.34","rtt_ms":24.1,"icmp":"echo_reply"}]}"#;

    /// The `end` line, as the design writes it.
    const END_LINE: &str = r#"{"type":"end","run":"2026-08-18T12:00:00.123Z","ts":"2026-08-18T13:00:00.000Z","rounds":1420,"reason":"quit"}"#;

    /// Reads an address that a test names.
    fn address(text: &str) -> IpAddr {
        text.parse().expect("the test address must parse")
    }

    /// Reads a moment that a test names, and converts it to UTC.
    fn moment(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the test moment must parse")
            .with_timezone(&Utc)
    }

    /// The record that opens the run of the design.
    fn a_run_record() -> Record {
        Record::Run(RunRecord {
            run: RunId::from(RUN),
            krt: "0.1.0 (abc1234, clean)".to_owned(),
            source: SourceLabel {
                addr: address("1.2.3.4"),
                kind: SourceKind::Public,
            },
            target: Target {
                arg: "example.com".to_owned(),
                addr: address(TARGET_ADDRESS),
                family: Family::Ipv4,
            },
            config: RunConfig {
                interval_ms: 1000,
                protocol: Protocol::Icmp,
                first_ttl: 1,
                max_ttl: 30,
                multipath: Multipath::Classic,
                privilege: Privilege::Unprivileged,
                dns: true,
            },
            host: "tims-mac".to_owned(),
        })
    }

    /// The name of the first hop of the run of the design.
    fn a_name_record() -> Record {
        Record::Name(NameRecord {
            run: RunId::from(RUN),
            ts: moment("2026-08-18T12:00:02.001Z"),
            addr: address(FIRST_HOP),
            host: "router.lan".to_owned(),
        })
    }

    /// One round of the run of the design.
    fn a_round_record() -> Record {
        Record::Round(RoundRecord {
            run: RunId::from(RUN),
            seq: 142,
            ts: moment("2026-08-18T12:34:56.789Z"),
            dur_ms: 1004,
            ttl_range: TtlRange::new(1, 14).expect("the test range must hold"),
            reached: true,
            hops: vec![
                Hop {
                    ttl: 1,
                    addr: address(FIRST_HOP),
                    rtt_ms: 1.23,
                    icmp: "time_exceeded".to_owned(),
                },
                Hop {
                    ttl: 14,
                    addr: address(TARGET_ADDRESS),
                    rtt_ms: 24.10,
                    icmp: "echo_reply".to_owned(),
                },
            ],
        })
    }

    /// The record that closes the run of the design.
    fn an_end_record() -> Record {
        Record::End(EndRecord {
            run: RunId::from(RUN),
            ts: moment("2026-08-18T13:00:00.000Z"),
            rounds: 1420,
            reason: EndReason::Quit,
        })
    }

    /// Writes one record as one line.
    fn line_of(record: &Record) -> String {
        record.to_line().expect("the record must become one line")
    }

    /// Reads one line as one record that this build knows.
    fn record_of(line: &str) -> Record {
        Record::from_line(line)
            .expect("the line must parse")
            .expect("the line must name a record that this build knows")
    }

    #[test]
    fn a_run_record_writes_the_run_line() {
        assert_eq!(line_of(&a_run_record()), RUN_LINE);
    }

    #[test]
    fn a_name_record_writes_the_name_line() {
        assert_eq!(line_of(&a_name_record()), NAME_LINE);
    }

    #[test]
    fn a_round_record_writes_the_round_line() {
        assert_eq!(line_of(&a_round_record()), ROUND_LINE);
    }

    #[test]
    fn an_end_record_writes_the_end_line() {
        assert_eq!(line_of(&an_end_record()), END_LINE);
    }

    #[test]
    fn the_run_line_reads_back_as_the_run_record() {
        assert_eq!(record_of(RUN_LINE), a_run_record());
    }

    #[test]
    fn the_name_line_reads_back_as_the_name_record() {
        assert_eq!(record_of(NAME_LINE), a_name_record());
    }

    #[test]
    fn the_round_line_reads_back_as_the_round_record() {
        assert_eq!(record_of(ROUND_LINE), a_round_record());
    }

    #[test]
    fn the_end_line_reads_back_as_the_end_record() {
        assert_eq!(record_of(END_LINE), an_end_record());
    }

    /// The design writes a round trip time of `24.10`, and a reader must take
    /// that text as the number that `serde_json` writes as `24.1`.
    #[test]
    fn a_round_trip_time_with_a_trailing_zero_reads_back_as_the_same_number() {
        let line = ROUND_LINE.replace("24.1,", "24.10,");
        assert_eq!(record_of(&line), a_round_record());
    }

    #[test]
    fn a_type_that_this_build_does_not_know_reads_as_no_record() {
        let unknown = Record::from_line(r#"{"type":"future","run":"x"}"#)
            .expect("an unknown type is no fault");
        assert_eq!(unknown, None);
    }

    #[test]
    fn a_line_without_a_type_is_a_fault() {
        assert!(Record::from_line(r#"{"run":"x"}"#).is_err());
    }

    #[test]
    fn a_line_that_is_not_json_is_a_fault() {
        assert!(Record::from_line("this is not json").is_err());
    }

    #[test]
    fn a_hop_that_did_not_answer_is_absent_from_the_round() {
        let record = Record::Round(RoundRecord {
            run: RunId::from(RUN),
            seq: 1,
            ts: moment("2026-08-18T12:34:56.789Z"),
            dur_ms: 1004,
            ttl_range: TtlRange::new(1, 3).expect("the test range must hold"),
            reached: false,
            hops: vec![Hop {
                ttl: 1,
                addr: address(FIRST_HOP),
                rtt_ms: 1.23,
                icmp: "time_exceeded".to_owned(),
            }],
        });
        let line = line_of(&record);
        assert!(
            line.contains(r#""ttl_range":[1,3]"#),
            "the line states which TTLs the round probed: {line}"
        );
        assert_eq!(
            line.matches(r#""ttl":"#).count(),
            1,
            "the line holds one hop: {line}"
        );
        for absent in [r#""ttl":2"#, r#""ttl":3"#] {
            assert!(!line.contains(absent), "the line holds no {absent}: {line}");
        }
    }

    #[test]
    fn a_range_that_runs_backward_is_a_fault() {
        let message = TtlRange::new(5, 3)
            .expect_err("a range that runs backward must fail")
            .to_string();
        for number in ["5", "3"] {
            assert!(
                message.contains(number),
                "the message names `{number}`: {message}"
            );
        }
    }

    #[test]
    fn a_range_holds_every_ttl_from_the_first_one_to_the_last_one() {
        let range = TtlRange::new(1, 30).expect("the test range must hold");
        for held in [1, 15, 30] {
            assert!(range.contains(held), "the range holds {held}");
        }
        for outside in [0, 31] {
            assert!(!range.contains(outside), "the range holds no {outside}");
        }
        assert_eq!(range.first(), 1);
        assert_eq!(range.last(), 30);
    }

    #[test]
    fn a_line_with_a_range_that_runs_backward_is_a_fault() {
        let line = ROUND_LINE.replace(r#""ttl_range":[1,14]"#, r#""ttl_range":[5,3]"#);
        assert!(Record::from_line(&line).is_err());
    }

    #[test]
    fn a_moment_at_a_whole_second_writes_three_fractional_digits() {
        let record = Record::End(EndRecord {
            run: RunId::from(RUN),
            ts: moment("2026-08-18T13:00:00Z"),
            rounds: 1420,
            reason: EndReason::Quit,
        });
        let line = line_of(&record);
        assert!(
            line.contains(r#""ts":"2026-08-18T13:00:00.000Z""#),
            "the line writes three fractional digits: {line}"
        );
    }

    #[test]
    fn a_moment_with_another_offset_reads_back_as_the_same_moment() {
        let line = END_LINE.replace("2026-08-18T13:00:00.000Z", "2026-08-18T14:00:00.000+01:00");
        assert_eq!(record_of(&line), an_end_record());
    }

    #[test]
    fn a_run_identifier_holds_the_start_time_to_the_millisecond() {
        assert_eq!(RunId::at(moment(RUN)).as_str(), RUN);
    }

    #[test]
    fn a_run_identifier_holds_the_start_time_in_utc() {
        assert_eq!(
            RunId::at(moment("2026-08-18T13:00:00.123+01:00")),
            RunId::from(RUN)
        );
    }

    #[test]
    fn a_run_identifier_at_a_whole_second_holds_three_fractional_digits() {
        assert_eq!(
            RunId::at(moment("2026-08-18T13:00:00Z")).as_str(),
            "2026-08-18T13:00:00.000Z"
        );
    }

    #[test]
    fn every_record_names_the_run_it_belongs_to() {
        let expected = RunId::from(RUN);
        for record in [
            a_run_record(),
            a_name_record(),
            a_round_record(),
            an_end_record(),
        ] {
            assert_eq!(record.run_id(), Some(&expected), "{record:?}");
        }
    }

    #[test]
    fn an_unknown_record_names_no_run() {
        assert_eq!(Record::Unknown.run_id(), None);
    }
}
