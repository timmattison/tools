//! The records of a recorded file, and the one line that each record writes.
//!
//! A recorded file holds one JSON object per line. The `type` field names the
//! record, and every record carries the identifier of the run it belongs to.
//! This slice builds the records, the two functions that turn a record into one
//! line and back, and the reader that loads a whole file. The writer and the
//! `replay` command arrive in the next slices.

// Nothing in `main.rs` reads these items yet.
#![allow(
    dead_code,
    reason = "the reader, the writer, and the replay command arrive in the next slices of issue #366"
)]

use crate::{Multipath, Protocol};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// The byte that ends one line of a recorded file.
const NEWLINE: u8 = b'\n';

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

/// Every record that one file holds, in the order that the file holds them.
///
/// The whole file lives in memory. A replay folds every round of a run, so a
/// reader that streamed would gain nothing here.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recording {
    /// The records of the file, in the order that the file holds them.
    records: Vec<Record>,
    /// The final line that was cut short, when the file holds one.
    truncated: Option<Truncated>,
}

impl Recording {
    /// Reads a recorded file.
    ///
    /// A line whose `type` this build does not know is skipped. A final line
    /// that no newline ended and that no parse read is reported through
    /// `truncated`, and every record before it still loads.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file does not open, when the read of the
    /// file fails, when a complete line is not UTF-8 text, and when a complete
    /// line does not parse.
    pub(crate) fn read(path: &Path) -> Result<Self, ReadError> {
        let file = File::open(path).map_err(|source| ReadError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mut reader = BufReader::new(file);
        let mut records = Vec::new();
        let mut truncated = None;
        let mut line = 0_usize;
        loop {
            let mut chunk = Vec::new();
            let read =
                reader
                    .read_until(NEWLINE, &mut chunk)
                    .map_err(|source| ReadError::Open {
                        path: path.to_path_buf(),
                        source,
                    })?;
            if read == 0 {
                break;
            }
            line += 1;
            // A chunk that carries no newline is the final chunk of the file.
            // The writer ends every record with a newline, so such a chunk is
            // as much of a record as the file holds.
            let complete = chunk.last() == Some(&NEWLINE);
            let bytes = chunk.len();
            let Ok(text) = String::from_utf8(chunk) else {
                if complete {
                    return Err(ReadError::NotText {
                        path: path.to_path_buf(),
                        line,
                    });
                }
                truncated = Some(Truncated { line, bytes });
                break;
            };
            let body = text.trim();
            if body.is_empty() {
                continue;
            }
            match Record::from_line(body) {
                Ok(Some(record)) => records.push(record),
                // A `type` value that this build does not know. Section 6.1 of
                // the design asks a reader to skip such a line.
                Ok(None) => {}
                Err(source) => {
                    if complete {
                        return Err(ReadError::Corrupt {
                            path: path.to_path_buf(),
                            line,
                            source,
                        });
                    }
                    truncated = Some(Truncated { line, bytes });
                    break;
                }
            }
        }
        Ok(Self { records, truncated })
    }

    /// Every record that the file holds.
    pub(crate) fn records(&self) -> &[Record] {
        &self.records
    }

    /// The final line that was cut short, when the file holds one.
    pub(crate) fn truncated(&self) -> Option<Truncated> {
        self.truncated
    }

    /// The identifier of every run that the file holds, in the order that the
    /// runs start.
    pub(crate) fn run_ids(&self) -> Vec<RunId> {
        let mut ids: Vec<RunId> = Vec::new();
        for id in self.records.iter().filter_map(Record::run_id) {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids
    }

    /// The records of one run. A run that the file does not hold gives `None`.
    pub(crate) fn run(&self, id: &RunId) -> Option<Run<'_>> {
        let mut run = Run {
            id: id.clone(),
            start: None,
            names: Vec::new(),
            rounds: Vec::new(),
            end: None,
        };
        let mut held = false;
        for record in self
            .records
            .iter()
            .filter(|candidate| candidate.run_id() == Some(id))
        {
            held = true;
            match record {
                Record::Run(start) => run.start = Some(start),
                Record::Name(name) => run.names.push(name),
                Record::Round(round) => run.rounds.push(round),
                Record::End(end) => run.end = Some(end),
                // An unknown record names no run, so the filter drops it.
                Record::Unknown => {}
            }
        }
        held.then_some(run)
    }

    /// The last run that the file holds.
    pub(crate) fn last_run(&self) -> Option<Run<'_>> {
        let id = self.run_ids().pop()?;
        self.run(&id)
    }
}

/// A final line that no newline ended and that no parse read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Truncated {
    /// The number of the line. The first line of a file is line one.
    line: usize,
    /// The number of bytes that the line holds.
    bytes: usize,
}

impl Truncated {
    /// The number of the line that was cut short.
    pub(crate) fn line(self) -> usize {
        self.line
    }

    /// The number of bytes that the cut line holds.
    pub(crate) fn bytes(self) -> usize {
        self.bytes
    }
}

impl fmt::Display for Truncated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "line {} is cut short at {} bytes",
            self.line, self.bytes
        )
    }
}

/// The records of one run inside a recording.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Run<'a> {
    /// The identifier of the run.
    id: RunId,
    /// The record that opened the run.
    start: Option<&'a RunRecord>,
    /// The names that the run read.
    names: Vec<&'a NameRecord>,
    /// The rounds that the run made.
    rounds: Vec<&'a RoundRecord>,
    /// The record that closed the run.
    end: Option<&'a EndRecord>,
}

impl<'a> Run<'a> {
    /// The identifier of the run.
    pub(crate) fn id(&self) -> &RunId {
        &self.id
    }

    /// The record that opened the run. A file that starts in the middle of a
    /// run holds none.
    pub(crate) fn start(&self) -> Option<&'a RunRecord> {
        self.start
    }

    /// The names that the run read.
    pub(crate) fn names(&self) -> &[&'a NameRecord] {
        &self.names
    }

    /// The rounds that the run made.
    pub(crate) fn rounds(&self) -> &[&'a RoundRecord] {
        &self.rounds
    }

    /// The record that closed the run. A run that still goes holds none.
    pub(crate) fn end(&self) -> Option<&'a EndRecord> {
        self.end
    }
}

/// Why a recorded file does not read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    /// The file does not open, or the read of the file fails.
    #[error("{}: {source}", path.display())]
    Open {
        /// The path of the file.
        path: PathBuf,
        /// The fault that the operating system reported.
        source: std::io::Error,
    },
    /// A complete line does not parse.
    #[error("{}: line {line} is not one record: {source}", path.display())]
    Corrupt {
        /// The path of the file.
        path: PathBuf,
        /// The number of the line. The first line of a file is line one.
        line: usize,
        /// The fault that the parser reported.
        source: serde_json::Error,
    },
    /// A complete line is not UTF-8 text.
    #[error("{}: line {line} is not utf-8 text", path.display())]
    NotText {
        /// The path of the file.
        path: PathBuf,
        /// The number of the line. The first line of a file is line one.
        line: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        EndReason, EndRecord, Family, Hop, NameRecord, Privilege, ReadError, Record, Recording,
        RoundRecord, Run, RunConfig, RunId, RunRecord, SourceKind, SourceLabel, Target, TtlRange,
    };
    use crate::{Multipath, Protocol};
    use chrono::{DateTime, Utc};
    use std::fs;
    use std::net::IpAddr;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

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

    /// The identifier of the second run of a file that holds two runs.
    const OTHER_RUN: &str = "2026-08-18T14:00:00.000Z";

    /// The identifier of a run that no test file holds.
    const ABSENT_RUN: &str = "2020-01-01T00:00:00.000Z";

    /// The sequence number that `ROUND_LINE` carries.
    const ROUND_SEQ: u64 = 142;

    /// The name of the first hop that `NAME_LINE` carries.
    const FIRST_HOST: &str = "router.lan";

    /// The name of the first hop of the second run of a file of two runs.
    const OTHER_HOST: &str = "core.example.net";

    /// A name that holds Japanese characters.
    ///
    /// A cut inside such a name falls inside one character, and a reader that
    /// indexes bytes panics on it.
    const JAPANESE_HOST: &str = "ルーター.lan";

    /// The reason that `END_LINE` carries.
    const END_REASON: &str = "quit";

    /// The reason that the second run of a file of two runs carries.
    const OTHER_REASON: &str = "duration";

    /// A line whose `type` value this build does not know.
    const WEATHER_LINE: &str = r#"{"type":"weather","sky":"clear"}"#;

    /// The start of an `end` line, as a cut final line writes it. The text
    /// holds 13 bytes.
    const CUT_CHUNK: &str = r#"{"type":"end""#;

    /// A line that is not JSON.
    const NOT_JSON_LINE: &str = "this is not json";

    /// Two bytes that no UTF-8 text holds.
    const NOT_TEXT: [u8; 2] = [0xff, 0xfe];

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
        std::env::temp_dir().join(format!("krt-{label}-{process}-{nanos}.jsonl"))
    }

    /// A file that one test makes. The file goes away when the test ends, and
    /// also when the test panics.
    struct TempFile {
        /// The path of the file.
        path: PathBuf,
    }

    impl TempFile {
        /// Writes the bytes to a new file that no other run reaches.
        fn new(label: &str, contents: &[u8]) -> Self {
            let path = temp_path(label);
            fs::write(&path, contents).expect("the test file must be written");
            Self { path }
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

    /// The `host` field that a `name` line writes.
    fn host_field(host: &str) -> String {
        format!(r#""host":"{host}""#)
    }

    /// The `seq` field that a `round` line writes.
    fn seq_field(seq: u64) -> String {
        format!(r#""seq":{seq}"#)
    }

    /// The `reason` field that an `end` line writes.
    fn reason_field(reason: &str) -> String {
        format!(r#""reason":"{reason}""#)
    }

    /// The `run` line of the run that the test names.
    fn run_line(id: &str) -> String {
        RUN_LINE.replace(RUN, id)
    }

    /// One `name` line of the run that the test names, with the name that the
    /// test names.
    fn name_line(id: &str, host: &str) -> String {
        NAME_LINE
            .replace(RUN, id)
            .replace(&host_field(FIRST_HOST), &host_field(host))
    }

    /// One `round` line of the run that the test names, with the sequence
    /// number that the test names.
    fn round_line(id: &str, seq: u64) -> String {
        ROUND_LINE
            .replace(RUN, id)
            .replace(&seq_field(ROUND_SEQ), &seq_field(seq))
    }

    /// The `end` line of the run that the test names, with the reason that the
    /// test names.
    fn end_line(id: &str, reason: &str) -> String {
        END_LINE
            .replace(RUN, id)
            .replace(&reason_field(END_REASON), &reason_field(reason))
    }

    /// Joins the lines of a file. Every line ends with a newline.
    fn file_of(lines: &[String]) -> String {
        let mut text = String::new();
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// A file that holds two runs. The first run makes two rounds, and the
    /// second run makes one.
    fn two_runs() -> String {
        file_of(&[
            run_line(RUN),
            name_line(RUN, FIRST_HOST),
            round_line(RUN, 1),
            round_line(RUN, 2),
            end_line(RUN, END_REASON),
            run_line(OTHER_RUN),
            name_line(OTHER_RUN, OTHER_HOST),
            round_line(OTHER_RUN, 7),
            end_line(OTHER_RUN, OTHER_REASON),
        ])
    }

    /// Reads the recording of a file that a test made.
    fn recording_of(file: &TempFile) -> Recording {
        Recording::read(file.path()).expect("the test file must read")
    }

    /// The `type` value of every record of a recording, in file order.
    fn kinds_of(recording: &Recording) -> Vec<&'static str> {
        recording.records().iter().map(kind_of).collect()
    }

    /// The sequence number of every round of a run.
    fn seqs_of(run: &Run<'_>) -> Vec<u64> {
        run.rounds().iter().map(|round| round.seq).collect()
    }

    /// The name that every `name` record of a run carries.
    fn hosts_of<'a>(run: &Run<'a>) -> Vec<&'a str> {
        run.names()
            .iter()
            .copied()
            .map(|name| name.host.as_str())
            .collect()
    }

    #[test]
    fn a_file_of_two_runs_holds_every_record_in_file_order() {
        let file = TempFile::new("two-runs", two_runs().as_bytes());
        let recording = recording_of(&file);
        assert_eq!(
            kinds_of(&recording),
            ["run", "name", "round", "round", "end", "run", "name", "round", "end"]
        );
        let first = RunId::from(RUN);
        let other = RunId::from(OTHER_RUN);
        let ids: Vec<&RunId> = recording
            .records()
            .iter()
            .filter_map(Record::run_id)
            .collect();
        assert_eq!(
            ids,
            [&first, &first, &first, &first, &first, &other, &other, &other, &other]
        );
        assert_eq!(recording.records()[0], a_run_record());
        assert_eq!(recording.records()[1], a_name_record());
        assert_eq!(recording.records()[4], an_end_record());
        assert_eq!(recording.truncated(), None);
    }

    #[test]
    fn a_file_of_two_runs_names_both_runs_in_the_order_they_start() {
        let file = TempFile::new("run-ids", two_runs().as_bytes());
        assert_eq!(
            recording_of(&file).run_ids(),
            [RunId::from(RUN), RunId::from(OTHER_RUN)]
        );
    }

    #[test]
    fn the_last_run_is_the_second_run_of_a_file_of_two_runs() {
        let file = TempFile::new("last-run", two_runs().as_bytes());
        let recording = recording_of(&file);
        let run = recording.last_run().expect("the file holds two runs");
        assert_eq!(run.id(), &RunId::from(OTHER_RUN));
        assert_eq!(seqs_of(&run), [7]);
    }

    #[test]
    fn a_named_run_is_the_first_run_of_a_file_of_two_runs() {
        let file = TempFile::new("named-run", two_runs().as_bytes());
        let recording = recording_of(&file);
        let run = recording
            .run(&RunId::from(RUN))
            .expect("the file holds the first run");
        assert_eq!(run.id(), &RunId::from(RUN));
        assert_eq!(seqs_of(&run), [1, 2]);
    }

    #[test]
    fn a_run_that_the_file_does_not_hold_is_no_run() {
        let file = TempFile::new("absent-run", two_runs().as_bytes());
        assert!(recording_of(&file).run(&RunId::from(ABSENT_RUN)).is_none());
    }

    #[test]
    fn a_run_holds_the_names_the_rounds_and_the_end_of_that_run_only() {
        let file = TempFile::new("one-run-only", two_runs().as_bytes());
        let recording = recording_of(&file);

        let first = recording
            .run(&RunId::from(RUN))
            .expect("the file holds the first run");
        assert_eq!(hosts_of(&first), [FIRST_HOST]);
        assert_eq!(seqs_of(&first), [1, 2]);
        assert_eq!(
            first.start().expect("the first run starts").run,
            RunId::from(RUN)
        );
        assert_eq!(
            first.end().expect("the first run ends").reason,
            EndReason::Quit
        );

        let other = recording
            .run(&RunId::from(OTHER_RUN))
            .expect("the file holds the second run");
        assert_eq!(hosts_of(&other), [OTHER_HOST]);
        assert_eq!(seqs_of(&other), [7]);
        assert_eq!(
            other.start().expect("the second run starts").run,
            RunId::from(OTHER_RUN)
        );
        assert_eq!(
            other.end().expect("the second run ends").reason,
            EndReason::Duration
        );
    }

    #[test]
    fn a_type_that_this_build_does_not_know_is_skipped_by_the_reader() {
        let text = file_of(&[
            run_line(RUN),
            WEATHER_LINE.to_owned(),
            end_line(RUN, END_REASON),
        ]);
        let file = TempFile::new("weather", text.as_bytes());
        let recording = recording_of(&file);
        assert_eq!(kinds_of(&recording), ["run", "end"]);
        assert_eq!(recording.truncated(), None);
    }

    #[test]
    fn a_final_line_that_is_cut_short_names_the_line_and_the_byte_count() {
        let cut = round_line(RUN, 2);
        let head = cut.len() / 2;
        let mut bytes = file_of(&[run_line(RUN), round_line(RUN, 1)]).into_bytes();
        bytes.extend_from_slice(&cut.as_bytes()[..head]);
        let file = TempFile::new("cut-record", &bytes);
        let recording = recording_of(&file);
        let truncated = recording.truncated().expect("the file holds a cut line");
        assert_eq!(truncated.line(), 3);
        assert_eq!(truncated.bytes(), head);
        assert_eq!(kinds_of(&recording), ["run", "round"]);
    }

    #[test]
    fn a_final_line_that_is_cut_inside_a_character_names_the_line_and_the_byte_count() {
        let cut = name_line(RUN, JAPANESE_HOST);
        let head = cut.find(JAPANESE_HOST).expect("the line holds the name") + 1;
        let mut bytes = file_of(&[run_line(RUN)]).into_bytes();
        bytes.extend_from_slice(&cut.as_bytes()[..head]);
        let file = TempFile::new("cut-character", &bytes);
        let recording = recording_of(&file);
        let truncated = recording.truncated().expect("the file holds a cut line");
        assert_eq!(truncated.line(), 2);
        assert_eq!(truncated.bytes(), head);
        assert_eq!(kinds_of(&recording), ["run"]);
    }

    #[test]
    fn a_file_that_ends_with_a_newline_holds_no_cut_line() {
        let file = TempFile::new("whole-file", two_runs().as_bytes());
        assert_eq!(recording_of(&file).truncated(), None);
    }

    #[test]
    fn a_final_line_that_no_newline_ended_and_that_parses_is_one_record() {
        let mut text = file_of(&[run_line(RUN)]);
        text.push_str(&end_line(RUN, END_REASON));
        let file = TempFile::new("no-final-newline", text.as_bytes());
        let recording = recording_of(&file);
        assert_eq!(kinds_of(&recording), ["run", "end"]);
        assert_eq!(recording.truncated(), None);
    }

    #[test]
    fn a_complete_line_that_is_not_json_names_the_line() {
        let text = file_of(&[
            run_line(RUN),
            NOT_JSON_LINE.to_owned(),
            end_line(RUN, END_REASON),
        ]);
        let file = TempFile::new("corrupt", text.as_bytes());
        let error = Recording::read(file.path()).expect_err("a corrupt line must fail");
        match &error {
            ReadError::Corrupt { path, line, .. } => {
                assert_eq!(*line, 2);
                assert_eq!(path, file.path());
            }
            other => panic!("a corrupt line must report a corrupt line: {other:?}"),
        }
        assert!(
            error.to_string().contains("line 2"),
            "the message names the line: {error}"
        );
    }

    #[test]
    fn a_complete_line_that_is_not_text_names_the_line() {
        let mut bytes = file_of(&[run_line(RUN)]).into_bytes();
        bytes.extend_from_slice(&NOT_TEXT);
        bytes.push(b'\n');
        bytes.extend_from_slice(end_line(RUN, END_REASON).as_bytes());
        bytes.push(b'\n');
        let file = TempFile::new("not-text", &bytes);
        let error = Recording::read(file.path()).expect_err("a line that is not text must fail");
        match &error {
            ReadError::NotText { path, line } => {
                assert_eq!(*line, 2);
                assert_eq!(path, file.path());
            }
            other => panic!("a line that is not text must report that fault: {other:?}"),
        }
        assert!(
            error.to_string().contains("line 2"),
            "the message names the line: {error}"
        );
    }

    #[test]
    fn a_file_that_is_absent_names_the_path() {
        let path = temp_path("absent");
        let error = Recording::read(&path).expect_err("an absent file must fail");
        assert!(
            matches!(error, ReadError::Open { .. }),
            "an absent file reports the open fault: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains(&path.display().to_string()),
            "the message names the path: {message}"
        );
    }

    #[test]
    fn an_empty_file_holds_no_record_and_no_run() {
        let file = TempFile::new("empty", b"");
        let recording = recording_of(&file);
        assert!(recording.records().is_empty(), "the file holds no record");
        assert!(recording.run_ids().is_empty(), "the file holds no run");
        assert!(recording.last_run().is_none(), "the file holds no last run");
        assert_eq!(recording.truncated(), None);
    }

    #[test]
    fn a_file_that_holds_no_run_record_still_names_the_run() {
        let text = file_of(&[round_line(RUN, 1), round_line(RUN, 2)]);
        let file = TempFile::new("no-run-record", text.as_bytes());
        let recording = recording_of(&file);
        assert_eq!(recording.run_ids(), [RunId::from(RUN)]);
        let run = recording.last_run().expect("the file holds one run");
        assert!(run.start().is_none(), "the file holds no run record");
        assert!(run.end().is_none(), "the file holds no end record");
        assert_eq!(seqs_of(&run), [1, 2]);
    }

    #[test]
    fn a_blank_line_between_two_records_is_no_fault() {
        let text = file_of(&[run_line(RUN), String::new(), end_line(RUN, END_REASON)]);
        let file = TempFile::new("blank-line", text.as_bytes());
        let recording = recording_of(&file);
        assert_eq!(kinds_of(&recording), ["run", "end"]);
        assert_eq!(recording.truncated(), None);
    }

    #[test]
    fn the_message_of_a_cut_line_names_the_line_and_the_byte_count() {
        let mut text = file_of(&[run_line(RUN)]);
        text.push_str(CUT_CHUNK);
        let file = TempFile::new("cut-message", text.as_bytes());
        let recording = recording_of(&file);
        let truncated = recording.truncated().expect("the file holds a cut line");
        assert_eq!(truncated.to_string(), "line 2 is cut short at 13 bytes");
    }
}
