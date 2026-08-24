//! The records of a recorded file, and the one line that each record writes.
//!
//! A recorded file holds one JSON object per line. The `type` field names the
//! record, and every record carries the identifier of the run it belongs to.
//! This module builds the records, the two functions that turn a record into
//! one line and back, the reader that loads a whole file, and the writer that
//! appends to one. The `replay` command reads through this module. The run loop
//! writes the `run` record, one `round` record for each round, and the `end`
//! record through the writer, and every write flushes. Two runs of one
//! destination from one machine append to one file, so every write of the
//! writer holds the exclusive lock of the file. A write that fails part way
//! gives back the bytes that it already put on the file, still under that lock,
//! so a fault leaves no fragment of a record for the next run to append onto.

use crate::{Multipath, Protocol};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay writes the identifier through `Display`, so the tests of this module are the one reader of the text today"
        )
    )]
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

    /// The TTLs that a round probed, from the first TTL upward.
    ///
    /// A last TTL below the first names no TTL that the round probed, so the
    /// range closes at the first TTL.
    ///
    /// This constructor never fails, and [`TtlRange::new`] does. A round that
    /// answered nothing reports a last TTL of zero, and the conversion of a
    /// round has already clamped that case away, so a `Result` there would
    /// carry an arm that no input reaches. A reader of a recorded file still
    /// needs the refusal, because a file states the two numbers on its own.
    pub(crate) fn from_first(first: u8, last: u8) -> Self {
        Self {
            first,
            last: last.max(first),
        }
    }

    /// The first TTL of the round.
    pub(crate) fn first(self) -> u8 {
        self.first
    }

    /// The last TTL of the round.
    pub(crate) fn last(self) -> u8 {
        self.last
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
        let file = File::open(path).map_err(|source| ReadError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut reader = BufReader::new(file);
        let mut records = Vec::new();
        let mut truncated = None;
        let mut line = 0_usize;
        loop {
            let mut chunk = Vec::new();
            let read = reader
                .read_until(NEWLINE, &mut chunk)
                .map_err(|source| ReadError::Io {
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay reads one run, so the tests of this module are the one reader of the whole file today"
        )
    )]
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay writes the whole warning through `Display`, so the tests of this module are the one reader of the two numbers today"
        )
    )]
    pub(crate) fn line(self) -> usize {
        self.line
    }

    /// The number of bytes that the cut line holds.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay writes the whole warning through `Display`, so the tests of this module are the one reader of the two numbers today"
        )
    )]
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no line of the table that a replay prints names why the run stopped: the header line holds what every row of the table has in common, and the reason a run stopped belongs to the run and not to a hop. The tests of this module are the one reader of the record"
        )
    )]
    pub(crate) fn end(&self) -> Option<&'a EndRecord> {
        self.end
    }
}

/// Why a recorded file does not read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    /// The file does not open, or a read of the file fails.
    ///
    /// The two faults share one variant, because the operating system reports
    /// both of them the same way and neither one names a record. A read that
    /// fails in the middle of a file therefore names no line, unlike every
    /// other fault of this reader, which names the line it read.
    #[error("{}: {source}", path.display())]
    Io {
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

/// Appends records to a recorded file.
///
/// The file opens in append mode, so one source and one destination keep one
/// file across many runs.
///
/// Two runs therefore write to one file at the same time. Each write of a run
/// takes the exclusive lock of the file, writes, and releases the lock, so the
/// bytes of one write stay together. The wait for the lock is bounded, and a
/// wait that runs out fails and names the file.
///
/// A write that fails part way gives its bytes back before it releases the
/// lock. The file therefore holds every byte of a record or no byte of it, and
/// a disk that fills costs the record that met the fault and no other record.
///
/// One record is one append. The rule holds at every size of a record, and
/// under every setting of a run. The lock covers one append, so a record that
/// left as two appends would release the lock in the middle of itself, and the
/// other run would put a whole record of its own into that gap. See
/// [`Writer::write`], which builds the line and its newline as one buffer, and
/// [`RecordFile::write`], which turns that one buffer into one append.
///
/// Every record reaches the operating system before the call returns, so a
/// `kill -9` loses at most one round. The flush is enough. A `kill -9` ends the
/// process, and the operating system keeps the bytes that the process already
/// gave it. An `fsync` guards against a power loss, which is a different fault,
/// and not the fault that this guarantee names.
///
/// The sink of a run is the open file, and that is the sink a caller takes when
/// it names none.
pub(crate) struct Writer<W: Write = RecordFile> {
    /// The sink that takes every record. A run writes to the open file, in
    /// append mode.
    sink: W,
}

impl Writer<RecordFile> {
    /// Opens the file for appending, and makes the file when it is absent.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file does not open.
    pub(crate) fn append(path: &Path) -> std::io::Result<Self> {
        Self::append_within(path, LOCK_WAIT)
    }

    /// Opens the file for appending, with the bound on the wait that the caller
    /// names.
    ///
    /// A run takes the bound of [`LOCK_WAIT`] through [`Writer::append`]. A
    /// test names a bound of a few milliseconds, so it proves the refusal of a
    /// file that another writer holds without a wait of five seconds.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file does not open.
    fn append_within(path: &Path, wait: Duration) -> std::io::Result<Self> {
        Ok(Self {
            sink: RecordFile::append(path, wait)?,
        })
    }
}

/// The bound on the wait for the lock of a recorded file.
///
/// A run writes one record in microseconds, and it takes one round each second
/// by default, so two runs of one file hold the lock for a very short moment. A
/// wait of five seconds therefore names a file that some other process holds
/// and does not release, and not the ordinary meeting of two runs.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The pause between two tries for the lock of a recorded file.
///
/// A holder keeps the lock for the length of one write, which takes
/// microseconds. A pause of one millisecond therefore gives the holder time to
/// finish. A shorter pause turns the wait into a spin, and a longer one adds
/// delay to every record that meets a holder.
const LOCK_RETRY_PAUSE: Duration = Duration::from_millis(1);

/// A recorded file that takes one record as one append.
///
/// The path of a recorded file comes from the source address and the
/// destination, so two runs of one destination from one machine append to one
/// file. Each write takes the exclusive lock of the file, writes, and releases
/// the lock. A record of one run therefore never lands inside a record of
/// another run.
///
/// A write that fails part way keeps the bytes that it already put on the file.
/// Such a write leaves a fragment that carries no newline, and the next record
/// of the other run lands on the same line. One line then holds half of one
/// record and the whole of another, the reader refuses that line, and both runs
/// lose every record of the file. The file therefore reads its own length under
/// the lock, and it cuts the file back to that length when the write fails. See
/// [`append_whole`].
///
/// The type holds no buffer of its own, so every write goes straight to the
/// file. A buffer would break the guarantee: it decides on its own where one
/// write stops, and a record that leaves as two writes is two appends with a
/// gap between them.
pub(crate) struct RecordFile {
    /// The open file, in append mode.
    file: File,
    /// The path of the file, for the message of a wait that ran out.
    path: PathBuf,
    /// The bound on the wait for the lock of the file. A wait that runs past it
    /// fails, so a run never stops forever on a holder that keeps the lock.
    wait: Duration,
}

impl RecordFile {
    /// Opens the file for appending, and makes the file when it is absent.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file does not open.
    fn append(path: &Path, wait: Duration) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            wait,
        })
    }

    /// Takes the exclusive lock of the file, inside the bound on the wait.
    ///
    /// The function tries the lock first, so a file that no other writer holds
    /// costs no pause at all. A file that another writer holds costs one pause
    /// for each try, until the bound runs out.
    ///
    /// The wait is bounded on purpose. A wait without a bound stops the run for
    /// as long as some other process keeps the lock, and a run that stops
    /// forever records nothing and says nothing.
    ///
    /// # Errors
    ///
    /// Returns the reason when the operating system refuses the lock, and a
    /// `TimedOut` fault that names the path when the wait runs out.
    fn lock(&self) -> std::io::Result<LockGuard<'_>> {
        let start = Instant::now();
        loop {
            match self.file.try_lock() {
                Ok(()) => return Ok(LockGuard { file: &self.file }),
                // Another writer holds the lock. Give it time to finish.
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(fault)) => return Err(fault),
            }
            if start.elapsed() >= self.wait {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "{}: another writer holds the file, and the wait of {} milliseconds ran out",
                        self.path.display(),
                        self.wait.as_millis()
                    ),
                ));
            }
            std::thread::sleep(LOCK_RETRY_PAUSE);
        }
    }
}

/// The exclusive lock of one recorded file, for the length of one append.
///
/// The lock goes away with this value. A write that fails therefore releases
/// the lock as well as a write that holds, and no fault leaves the file locked
/// against the next run.
///
/// The bytes of a write that failed go away before the lock does.
/// [`append_whole`] cuts the file back inside the span that this guard holds,
/// so the next writer of the file takes the lock and finds no fragment of the
/// record that met the fault.
struct LockGuard<'a> {
    /// The file that holds the lock.
    ///
    /// The borrow is not mutable, because `&File` writes as well as `&mut File`
    /// does. The guard therefore stands beside the write that it protects.
    file: &'a File,
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        // A release that fails leaves the lock with this process, and the
        // operating system gives the lock back when the process closes the
        // file. There is nothing more to do here, and a write that already
        // reached the file must not fail on the release.
        let _ = self.file.unlock();
    }
}

/// A sink that a recorded file appends to.
///
/// The file of a run is the one sink of the binary, and an append that fails
/// after some of its bytes reached the file is the one fault that matters here.
/// A test cannot ask a real file for that fault: a disk that fills and a device
/// that goes away are faults of the machine, and a test starts neither one.
/// This trait is therefore the seam. A test writes a sink that takes part of a
/// buffer and then fails, and it reads what [`append_whole`] leaves on that
/// sink.
trait Appendable {
    /// The number of bytes that the sink holds.
    ///
    /// # Errors
    ///
    /// Returns the reason when the sink does not report the number.
    fn length(&self) -> std::io::Result<u64>;

    /// Appends the whole buffer to the sink.
    ///
    /// # Errors
    ///
    /// Returns the reason when the append fails. An append that fails part way
    /// keeps the bytes that it already put on the sink.
    fn append(&mut self, buf: &[u8]) -> std::io::Result<()>;

    /// Drops every byte of the sink after this number of bytes.
    ///
    /// # Errors
    ///
    /// Returns the reason when the sink does not shorten.
    fn shorten(&mut self, length: u64) -> std::io::Result<()>;
}

impl Appendable for &File {
    /// The number of bytes that the file holds, from the metadata of the file.
    ///
    /// # Errors
    ///
    /// Returns the reason when the metadata does not read.
    fn length(&self) -> std::io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    /// Appends the whole buffer to the file.
    ///
    /// # Errors
    ///
    /// Returns the reason when the write fails.
    fn append(&mut self, buf: &[u8]) -> std::io::Result<()> {
        Write::write_all(self, buf)
    }

    /// Cuts the file back to this number of bytes.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file does not cut.
    fn shorten(&mut self, length: u64) -> std::io::Result<()> {
        self.set_len(length)
    }
}

/// Appends the whole buffer to the sink, or leaves the sink as it was.
///
/// The function reads the length of the sink first. An append that fails part
/// way keeps the bytes that it already put on the sink, so the function cuts
/// the sink back to that length before it returns the fault. The caller holds
/// the exclusive lock of the file across the whole span, so no other writer
/// appends inside it, and the cut therefore drops the bytes of the failed
/// append and no other byte.
///
/// The bytes must go away together. A fragment that stays carries no newline,
/// and the next record of another run then lands on the same line. That line
/// holds half of one record and the whole of another, and the reader refuses
/// the file for it.
///
/// A cut that fails leaves the fragment, and the caller reads the fault of the
/// append and not the fault of the cut. The fault of the append is the one that
/// says why the record did not reach the file.
///
/// # Errors
///
/// Returns the reason when the length does not read, and when the append fails.
fn append_whole(sink: &mut impl Appendable, buf: &[u8]) -> std::io::Result<()> {
    let length = sink.length()?;
    if let Err(fault) = sink.append(buf) {
        let _ = sink.shorten(length);
        return Err(fault);
    }
    Ok(())
}

impl Write for RecordFile {
    /// Appends the whole buffer to the file as one write, under the exclusive
    /// lock of the file.
    ///
    /// The answer is the whole length of the buffer, so [`Write::write_all`]
    /// calls this function one time. A record that reaches this function as one
    /// buffer therefore becomes one append, and that is what keeps the records
    /// of two runs of one file apart.
    ///
    /// A write that fails part way cuts the file back to the length that this
    /// function read under the lock. The file therefore keeps no byte of a
    /// record that did not reach it whole, and the next writer of the file
    /// appends onto the last whole line. See [`append_whole`].
    ///
    /// The lock goes away with the guard, at the end of this function. A second
    /// writer of the same file therefore goes on as soon as the bytes are on
    /// the file.
    ///
    /// # Errors
    ///
    /// Returns the reason when the lock does not come inside the bound on the
    /// wait, when the length of the file does not read, and when the write
    /// fails. A write that fails leaves the file as it was.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _guard = self.lock()?;
        append_whole(&mut &self.file, buf)?;
        Ok(buf.len())
    }

    /// Flushes the file.
    ///
    /// The type holds no buffer of its own, so every byte of a write is already
    /// with the operating system when the write returns. The call goes to the
    /// file, which has nothing to empty.
    ///
    /// # Errors
    ///
    /// Returns the reason when the flush of the file fails.
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl<W: Write> Writer<W> {
    /// Sends every record to this sink.
    ///
    /// A run writes to a file, and a file fails a write only when the disk
    /// fills or the device goes away. A test drives the writer through a sink
    /// that fails on demand, so it covers the fault that a file rarely gives.
    ///
    /// No run builds a writer this way, so the build of the tool holds none.
    #[cfg(test)]
    pub(crate) fn to_sink(sink: W) -> Self {
        Self { sink }
    }

    /// Appends one record and its newline as one write, then flushes.
    ///
    /// The line and the newline go into one buffer, and that buffer makes one
    /// call. This is the rule of the writer, and not a detail of it. A
    /// [`RecordFile`] takes the exclusive lock of the file for one write and
    /// releases it after that write, so a record that left as two writes would
    /// release the lock in the middle of itself. Two runs of one destination
    /// from one machine append to one file, and the second run would then put a
    /// whole record of its own into that gap. One line of the file would hold
    /// two records, and the reader refuses such a line and with it the whole
    /// file.
    ///
    /// [`Write::write_all`] makes one call for one buffer, because
    /// [`RecordFile::write`] answers with the whole length of the buffer it
    /// took. So one record is one buffer, one buffer is one call, and one call
    /// is one append.
    ///
    /// # Errors
    ///
    /// Returns the reason when the record does not become JSON, when the append
    /// fails, and when the flush fails. A [`RecordFile`] fails the append when
    /// the lock of the file does not come inside the bound on the wait, and it
    /// leaves the file as it was when the append itself fails.
    pub(crate) fn write(&mut self, record: &Record) -> std::io::Result<()> {
        let mut line = record
            .to_line()
            .map_err(std::io::Error::other)?
            .into_bytes();
        line.push(NEWLINE);
        self.sink.write_all(&line)?;
        self.sink.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_whole, Appendable, EndReason, EndRecord, Family, Hop, NameRecord, Privilege,
        ReadError, Record, RecordFile, Recording, RoundRecord, Run, RunConfig, RunId, RunRecord,
        SourceKind, SourceLabel, Target, TtlRange, Writer, LOCK_WAIT, NEWLINE,
    };
    use crate::testing::{address, SecondRunBetweenWrites};
    use crate::{Multipath, Protocol};
    use chrono::{DateTime, Utc};
    use std::cell::RefCell;
    use std::fs;
    use std::io::Write;
    use std::net::IpAddr;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    fn a_range_keeps_the_first_ttl_and_the_last_one_it_was_made_with() {
        let range = TtlRange::new(1, 30).expect("the test range must hold");
        assert_eq!(range.first(), 1);
        assert_eq!(range.last(), 30);
    }

    #[test]
    fn a_range_from_the_first_ttl_spans_to_the_last_one() {
        let range = TtlRange::from_first(1, 14);
        assert_eq!(range.first(), 1);
        assert_eq!(range.last(), 14);
    }

    /// A round that answered nothing reports a last TTL of zero, which runs
    /// below every first TTL. Such a range names no TTL that the round probed,
    /// so it closes at the first TTL.
    #[test]
    fn a_range_whose_last_ttl_runs_below_the_first_one_closes_at_the_first_one() {
        let range = TtlRange::from_first(3, 1);
        assert_eq!(range.first(), 3);
        assert_eq!(range.last(), 3);
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

        /// Holds a path that no file uses yet, and that no other run reaches.
        ///
        /// The file that a test makes at the path goes away with this value,
        /// and a path that stays empty is no fault.
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
            matches!(error, ReadError::Io { .. }),
            "an absent file reports the fault of the operating system: {error:?}"
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

    /// The character that ends one line of a recorded file.
    const NEWLINE_CHAR: char = super::NEWLINE as char;

    /// One round of the run of the design, with the sequence number that the
    /// test names.
    fn a_round_record_of(seq: u64) -> Record {
        match a_round_record() {
            Record::Round(mut round) => {
                round.seq = seq;
                Record::Round(round)
            }
            other => panic!("the round record must be a round record: {other:?}"),
        }
    }

    /// One record of every type that this build knows, in schema order.
    fn every_record() -> Vec<Record> {
        vec![
            a_run_record(),
            a_name_record(),
            a_round_record(),
            an_end_record(),
        ]
    }

    /// Opens a writer on the path that a test names.
    fn writer_on(path: &Path) -> Writer {
        Writer::append(path).expect("the test file must open for appending")
    }

    /// Appends one record through a writer.
    fn write_record(writer: &mut Writer, record: &Record) {
        writer.write(record).expect("the record must be written");
    }

    /// Appends every record through one writer, and closes the writer.
    fn write_all(path: &Path, records: &[Record]) {
        let mut writer = writer_on(path);
        for record in records {
            write_record(&mut writer, record);
        }
    }

    /// The text that a test file holds.
    fn text_of(file: &TempFile) -> String {
        fs::read_to_string(file.path()).expect("the test file must read")
    }

    #[test]
    fn a_second_writer_appends_to_the_file_and_keeps_what_the_first_one_wrote() {
        let file = TempFile::absent("append");
        write_all(file.path(), &[a_run_record()]);
        write_all(file.path(), &[an_end_record()]);
        let recording = recording_of(&file);
        assert_eq!(kinds_of(&recording), ["run", "end"]);
        assert_eq!(
            recording.records(),
            [a_run_record(), an_end_record()].as_slice()
        );
    }

    #[test]
    fn a_writer_makes_the_file_when_the_file_is_absent() {
        let file = TempFile::absent("make-file");
        assert!(
            !file.path().exists(),
            "the test starts with no file: {}",
            file.path().display()
        );
        let writer = writer_on(file.path());
        assert!(
            file.path().exists(),
            "the writer makes the file: {}",
            file.path().display()
        );
        drop(writer);
    }

    /// The writer flushes after every record, so a `kill -9` loses at most one
    /// round. A writer that only emptied its buffer on drop would leave the
    /// file empty here, and would still pass every other test of the writer.
    #[test]
    fn a_record_reaches_the_file_before_the_writer_drops() {
        let file = TempFile::absent("flush");
        let mut writer = writer_on(file.path());
        write_record(&mut writer, &a_run_record());
        let recording = recording_of(&file);
        assert_eq!(recording.records(), [a_run_record()].as_slice());
        assert_eq!(recording.truncated(), None);
        drop(writer);
    }

    #[test]
    fn every_record_writes_one_line_that_a_newline_ends() {
        let file = TempFile::absent("one-line-each");
        let records = every_record();
        write_all(file.path(), &records);
        let text = text_of(&file);
        let lines: Vec<&str> = text.split_inclusive(NEWLINE_CHAR).collect();
        assert_eq!(lines.len(), records.len(), "the file holds one line each");
        for (line, record) in lines.iter().zip(&records) {
            assert!(
                line.ends_with(NEWLINE_CHAR),
                "a newline ends the line: {line}"
            );
            let mut expected = line_of(record);
            expected.push(NEWLINE_CHAR);
            assert_eq!(*line, expected);
        }
    }

    #[test]
    fn the_reader_reads_back_every_record_the_writer_wrote() {
        let file = TempFile::absent("round-trip");
        let records = every_record();
        write_all(file.path(), &records);
        let recording = recording_of(&file);
        assert_eq!(kinds_of(&recording), ["run", "name", "round", "end"]);
        assert_eq!(recording.records(), records.as_slice());
        assert_eq!(recording.truncated(), None);
    }

    #[test]
    fn a_run_of_many_records_keeps_the_order_the_writer_wrote() {
        let file = TempFile::absent("order");
        let mut records = vec![a_run_record()];
        records.extend((1..=3_u64).map(a_round_record_of));
        records.push(an_end_record());
        write_all(file.path(), &records);
        let recording = recording_of(&file);
        assert_eq!(
            kinds_of(&recording),
            ["run", "round", "round", "round", "end"]
        );
        assert_eq!(recording.records(), records.as_slice());
        let run = recording.last_run().expect("the file holds one run");
        assert_eq!(seqs_of(&run), [1, 2, 3]);
    }

    /// The bound on the wait for the lock that the test of a held file names.
    ///
    /// The test holds the lock for the whole write, so the writer waits the
    /// whole bound. A few milliseconds keep the test fast, and the bound of a
    /// run stays at five seconds.
    const SHORT_WAIT: Duration = Duration::from_millis(5);

    /// Opens a second handle on a file that a test made, and takes the
    /// exclusive lock of it.
    ///
    /// Two handles of one path contend for the lock inside one process, so the
    /// test needs no second process. The caller releases the lock at the end of
    /// the test.
    fn a_held_file(path: &Path) -> fs::File {
        let holder = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("the test file must open for appending");
        holder.lock().expect("the test must take the lock");
        holder
    }

    /// A run that cannot take the lock inside the bound must stop, and it must
    /// name the file that it could not take. A writer that wrote anyway would
    /// put its record inside the record of the holder.
    #[test]
    fn a_writer_that_cannot_take_the_lock_names_the_file_and_writes_no_record() {
        let file = TempFile::absent("held-file");
        let holder = a_held_file(file.path());

        let mut writer = Writer::append_within(file.path(), SHORT_WAIT)
            .expect("the test file must open for appending");
        let message = writer
            .write(&a_run_record())
            .expect_err("a writer that cannot take the lock must fail")
            .to_string();
        assert!(
            message.contains(&file.path().display().to_string()),
            "the message names the path: {message}"
        );
        assert_eq!(
            fs::read(file.path()).expect("the test file must read"),
            Vec::<u8>::new(),
            "a writer that cannot take the lock writes no record"
        );

        holder.unlock().expect("the test must release the lock");
    }

    #[test]
    fn a_path_whose_directory_is_absent_is_a_fault() {
        let directory = TempFile::absent("absent-directory");
        let path = directory.path().join("record.jsonl");
        assert!(
            Writer::append(&path).is_err(),
            "a path under an absent directory must fail: {}",
            path.display()
        );
    }

    /// A sink that remembers the buffer of every write that reached it.
    ///
    /// The writer holds the exclusive lock of the file for one write and no
    /// longer, so the number of writes that one record takes is the whole of
    /// the guarantee. A file cannot report that number, because a file holds
    /// the bytes and not the writes that carried them. This sink holds the
    /// writes, so a test reads the guarantee directly.
    #[derive(Clone, Default)]
    struct Appends {
        /// The buffer of every write, in the order that the writes came.
        ///
        /// The handle is shared, because a writer takes its sink, and the test
        /// reads the writes after that.
        calls: Rc<RefCell<Vec<Vec<u8>>>>,
    }

    impl Appends {
        /// The buffer of every write that reached the sink.
        fn calls(&self) -> Vec<Vec<u8>> {
            self.calls.borrow().clone()
        }
    }

    impl Write for Appends {
        /// Takes the whole buffer, and remembers it.
        ///
        /// The answer is the whole length of the buffer, as the answer of a
        /// recorded file is, so [`Write::write_all`] calls this function one
        /// time for one buffer. The sink therefore counts the writes that the
        /// writer makes, and not the writes that `write_all` makes.
        ///
        /// # Errors
        ///
        /// Never fails. The sink holds every buffer in memory.
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls.borrow_mut().push(buf.to_vec());
            Ok(buf.len())
        }

        /// Empties nothing, because the sink holds no buffer of its own.
        ///
        /// # Errors
        ///
        /// Never fails.
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A sink that fills up part way through an append.
    ///
    /// The sink has room for a number of bytes. An append that needs more room
    /// than that takes the room that is left and then fails, as a disk that
    /// fills does. The three methods are honest: the sink reports the bytes it
    /// holds, it keeps the bytes of an append that failed, and it drops the
    /// bytes after a length that a caller names.
    struct SinkThatFills {
        /// The bytes that the sink holds, in the order that they came.
        held: Vec<u8>,
        /// The number of bytes that the sink still takes before it fills.
        room: usize,
    }

    impl SinkThatFills {
        /// A sink that holds these bytes and has room for this many more.
        fn with_room(held: &[u8], room: usize) -> Self {
            Self {
                held: held.to_vec(),
                room,
            }
        }

        /// The bytes that the sink holds.
        fn held(&self) -> Vec<u8> {
            self.held.clone()
        }
    }

    impl Appendable for SinkThatFills {
        /// The number of bytes that the sink holds.
        ///
        /// # Errors
        ///
        /// Never fails. The sink counts the bytes in memory.
        fn length(&self) -> std::io::Result<u64> {
            Ok(u64::try_from(self.held.len())
                .expect("the sink holds fewer bytes than a u64 counts"))
        }

        /// Takes the bytes that the room holds, and fails when the buffer needs
        /// more.
        ///
        /// # Errors
        ///
        /// Returns a `StorageFull` fault when the buffer needs more room than
        /// the sink has. The sink keeps the bytes that it already took, as a
        /// file keeps the bytes of a write that failed part way.
        fn append(&mut self, buf: &[u8]) -> std::io::Result<()> {
            let taken = buf.len().min(self.room);
            self.held.extend_from_slice(&buf[..taken]);
            self.room -= taken;
            if taken < buf.len() {
                return Err(std::io::Error::from(std::io::ErrorKind::StorageFull));
            }
            Ok(())
        }

        /// Drops every byte of the sink after this number of bytes.
        ///
        /// # Errors
        ///
        /// Never fails. The sink cuts the bytes in memory.
        fn shorten(&mut self, length: u64) -> std::io::Result<()> {
            self.held
                .truncate(usize::try_from(length).expect("the length fits the memory of the test"));
            Ok(())
        }
    }

    /// The line that the sink already holds before the append that fails.
    ///
    /// The line is whole, and it ends with a newline, as every line of a
    /// recorded file does.
    const A_LINE_ALREADY_HELD: &[u8] = b"{\"type\":\"end\",\"run\":\"2026-08-18T12:00:00.123Z\"}\n";

    /// The number of bytes of the next record that fit before the sink fills.
    ///
    /// The number is smaller than the record, so the append takes part of the
    /// record and then fails. That is the one fault that matters: the sink
    /// holds a fragment of a record, and the fragment carries no newline.
    const ROOM_LEFT: usize = 5;

    /// A write that fails part way keeps the bytes that it already wrote, and
    /// the guard then releases the lock. The file ends with a fragment that
    /// carries no newline.
    ///
    /// One run survives that: the fragment is the last line, the reader marks
    /// the file truncated, and the file still reads. Two runs do not. The
    /// second run takes the lock and appends its next record onto the same
    /// line, and that line then holds half of one record and the whole of
    /// another. The reader answers `Corrupt` for such a line, so `replay`
    /// refuses the whole file and both runs lose every record in it.
    #[test]
    fn a_write_that_fails_part_way_leaves_the_file_as_it_was() {
        let mut record = a_run_record()
            .to_line()
            .expect("the record must become JSON")
            .into_bytes();
        record.push(NEWLINE);
        assert!(
            record.len() > ROOM_LEFT,
            "the record must need more room than the sink has"
        );
        let mut sink = SinkThatFills::with_room(A_LINE_ALREADY_HELD, ROOM_LEFT);

        let fault = append_whole(&mut sink, &record)
            .expect_err("a sink that fills part way through the buffer must fail the append");

        assert_eq!(
            fault.kind(),
            std::io::ErrorKind::StorageFull,
            "the fault of the sink reaches the caller"
        );
        assert_eq!(
            String::from_utf8_lossy(&sink.held()),
            String::from_utf8_lossy(A_LINE_ALREADY_HELD),
            "a write that fails part way leaves the file as it was"
        );
    }

    /// The first TTL that a run probes.
    const FIRST_TTL: u8 = 1;

    /// The last TTL that a run probes.
    ///
    /// The tracer takes no TTL above this one, so a round of this range is the
    /// widest range of TTLs that a run records.
    const LAST_TTL: u8 = 254;

    /// The number of routers that answer at one TTL of the largest round.
    ///
    /// The `paris` and the `dublin` modes vary the flow of a probe, so more
    /// than one router answers at one TTL. The hops of such a round therefore
    /// outnumber the TTLs that the round probed.
    const HOPS_AT_ONE_TTL: usize = 2;

    /// The smallest number of bytes that the line of the largest round holds.
    ///
    /// The bound guards the two tests that write the largest round. Both of
    /// them need a record that takes a long moment to write, because that
    /// moment is the moment that a second run meets the first one in. A later
    /// schema that shrank a record to a few hundred bytes would leave the two
    /// runs almost no moment to meet in, and the tests would then pass because
    /// they raced nothing.
    const LARGEST_ROUND_BYTES: usize = 49_000;

    /// The first six groups of every hop address of the first run of a shared
    /// file.
    ///
    /// Each group holds four hex digits and no group is zero, so `Display`
    /// compresses nothing and the address writes its full 39 characters. A
    /// wide address makes a wide record.
    const A_HOP_PREFIX: &str = "abcd:ef01:2345:6789:abcd:ef01";

    /// The first six groups of every hop address of the second run of a shared
    /// file.
    ///
    /// The prefix differs from `A_HOP_PREFIX`, so a reader of the file tells
    /// the hops of one run from the hops of the other.
    const OTHER_HOP_PREFIX: &str = "bcde:f012:3456:789a:bcde:f012";

    /// The number of rounds that each run of a shared file records.
    const ROUNDS_OF_A_SHARED_FILE: u64 = 8;

    /// The address of one hop of the largest round.
    ///
    /// The prefix names the run. The two groups at the end hold the TTL and
    /// the number of the router at that TTL, and each of them starts with a
    /// digit that is not zero, so `Display` writes all four digits of it.
    fn hop_address(prefix: &str, ttl: u8, router: usize) -> IpAddr {
        address(&format!("{prefix}:1{ttl:03x}:2{router:03x}"))
    }

    /// The largest round that one run can record.
    ///
    /// Two settings reach past the size of an ordinary round. The run probes
    /// every TTL up to the last one that the tracer takes, and a multipath
    /// mode reports more than one router at each of those TTLs. Each hop
    /// carries an address of the full width of IP version 6.
    ///
    /// # Panics
    ///
    /// Panics when the line of the round holds fewer bytes than
    /// [`LARGEST_ROUND_BYTES`]. Such a round no longer races anything, and a
    /// test that raced nothing passes for the wrong reason.
    fn the_largest_round_record(run: &str, seq: u64, prefix: &str) -> Record {
        let mut hops = Vec::new();
        for ttl in FIRST_TTL..=LAST_TTL {
            for router in 0..HOPS_AT_ONE_TTL {
                hops.push(Hop {
                    ttl,
                    addr: hop_address(prefix, ttl, router),
                    rtt_ms: 1.23,
                    icmp: TIME_EXCEEDED.to_owned(),
                });
            }
        }
        let record = Record::Round(RoundRecord {
            run: RunId::from(run),
            seq,
            ts: moment("2026-08-18T12:34:56.789Z"),
            dur_ms: 1004,
            ttl_range: TtlRange::new(FIRST_TTL, LAST_TTL).expect("the test range must hold"),
            reached: true,
            hops,
        });
        let bytes = line_of(&record).len();
        assert!(
            bytes >= LARGEST_ROUND_BYTES,
            "the largest round holds {LARGEST_ROUND_BYTES} bytes at least, and this one holds {bytes}"
        );
        record
    }

    /// The number of hops that the largest round holds.
    fn hops_of_the_largest_round() -> usize {
        usize::from(LAST_TTL - FIRST_TTL + 1) * HOPS_AT_ONE_TTL
    }

    /// A record must reach the sink of the writer as one write.
    ///
    /// The writer takes the exclusive lock of the file for one write and
    /// releases it after that write. A record that leaves as two writes
    /// therefore releases the lock in the middle of itself, and a whole record
    /// of another run lands in the gap.
    #[test]
    fn one_record_leaves_the_writer_as_one_append() {
        let sink = Appends::default();
        let record = the_largest_round_record(RUN, 1, A_HOP_PREFIX);
        let mut writer = Writer::to_sink(sink.clone());
        writer.write(&record).expect("the record must be written");

        let calls = sink.calls();
        assert_eq!(calls.len(), 1, "one record reaches the sink as one write");
        let written = calls.first().expect("the record reaches the sink");
        let mut expected = line_of(&record).into_bytes();
        expected.push(super::NEWLINE);
        assert_eq!(
            written.len(),
            expected.len(),
            "the one write carries the whole line and the newline"
        );
        assert!(
            written == &expected,
            "the one write carries the line first and the newline after it"
        );
    }

    /// The address of every hop of one round, as the file holds them.
    fn hop_addresses_of(round: &RoundRecord) -> Vec<String> {
        round.hops.iter().map(|hop| hop.addr.to_string()).collect()
    }

    /// Two runs of one destination from one machine append to one file at one
    /// moment. Every line of that file must hold one record and no more.
    ///
    /// A record that leaves the writer as two writes releases the lock of the
    /// file in the middle of itself, and the second run then appends a whole
    /// record into the gap. The line of the first run holds two records after
    /// that, the reader refuses the whole file, and both runs are lost.
    ///
    /// The second run appends after every write of the first one, so the test
    /// reads that meeting every time. See [`SecondRunBetweenWrites`] for why
    /// two threads read it almost never.
    #[test]
    fn two_runs_that_share_one_file_leave_every_line_whole() {
        let file = TempFile::absent("two-runs-one-file");
        let other: Vec<Record> = (1..=ROUNDS_OF_A_SHARED_FILE)
            .map(|seq| the_largest_round_record(OTHER_RUN, seq, OTHER_HOP_PREFIX))
            .collect();
        let first = RecordFile::append(file.path(), LOCK_WAIT)
            .expect("the test file must open for appending");
        let sink = SecondRunBetweenWrites::on(first, file.path(), other)
            .expect("the second run must open the same file");
        let mut writer = Writer::to_sink(sink);
        for seq in 1..=ROUNDS_OF_A_SHARED_FILE {
            writer
                .write(&the_largest_round_record(RUN, seq, A_HOP_PREFIX))
                .expect("the record must be written");
        }
        drop(writer);

        let recording = Recording::read(file.path())
            .expect("a file that two runs shared must hold one record on each line");
        assert_eq!(
            recording.truncated(),
            None,
            "every run ended its last line with a newline"
        );
        assert_eq!(
            recording.run_ids(),
            [RunId::from(RUN), RunId::from(OTHER_RUN)],
            "the file holds the rounds of both runs"
        );

        let expected: Vec<u64> = (1..=ROUNDS_OF_A_SHARED_FILE).collect();
        for (run, prefix) in [(RUN, A_HOP_PREFIX), (OTHER_RUN, OTHER_HOP_PREFIX)] {
            let held = recording
                .run(&RunId::from(run))
                .expect("the file holds the run");
            assert_eq!(
                seqs_of(&held),
                expected,
                "the file holds every round of {run}"
            );
            for round in held.rounds() {
                let addresses = hop_addresses_of(round);
                assert_eq!(
                    addresses.len(),
                    hops_of_the_largest_round(),
                    "round {} of {run} holds every hop that the run wrote",
                    round.seq
                );
                for addr in &addresses {
                    assert!(
                        addr.starts_with(prefix),
                        "round {} of {run} holds the hops of that run alone: {addr}",
                        round.seq
                    );
                }
            }
        }
    }

    // The golden fixture. The repository holds one recorded file of two runs,
    // and the tests below read it. A schema change that nobody wanted moves
    // the records away from the fixture, and the comparison fails. The three
    // mutation tests prove that the comparison can fail, because a guard that
    // matches anything is worse than no guard.

    /// The golden fixture, as the repository holds it.
    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/two-runs.jsonl");

    /// The number of lines that the golden fixture holds.
    ///
    /// One of the lines names a `type` that this build does not know, so the
    /// reader gives one record less than this.
    const FIXTURE_LINES: usize = 9;

    /// The identifier of the second run of the golden fixture.
    const SECOND_RUN: &str = "2026-08-19T09:30:00.000Z";

    /// The build string of the second run of the golden fixture.
    const KRT_DIRTY: &str = "0.1.0 (abc1234, dirty)";

    /// The source address of the second run of the golden fixture.
    const SECOND_SOURCE: &str = "2001:db8::1";

    /// The destination of the second run, as the user typed it.
    const SECOND_TARGET: &str = "example.org";

    /// The address of the destination of the second run.
    const SECOND_TARGET_ADDRESS: &str = "93.184.216.35";

    /// The address of the first hop of the second run.
    const SECOND_FIRST_HOP: &str = "10.0.0.1";

    /// The name of the machine that made both runs of the golden fixture.
    const MACHINE: &str = "tims-mac";

    /// The ICMP message that a hop below the target answers with.
    const TIME_EXCEEDED: &str = "time_exceeded";

    /// The ICMP message that the target answers with.
    const ECHO_REPLY: &str = "echo_reply";

    /// The `type` field of the one line that this build does not know.
    const WEATHER_TYPE: &str = r#""type":"weather""#;

    /// The name of the round trip time field, as the fixture writes it.
    const RTT_FIELD: &str = r#""rtt_ms""#;

    /// The name of the round trip time field, renamed.
    const RENAMED_RTT_FIELD: &str = r#""rtt_millis""#;

    /// The duration of the first round of the first run, as the fixture holds
    /// it.
    const ROUND_DURATION: &str = r#""dur_ms":1004"#;

    /// The duration of the first round of the first run, changed.
    const CHANGED_ROUND_DURATION: &str = r#""dur_ms":9999"#;

    /// The range of TTLs that both rounds of the first run probed.
    const FIXTURE_TTL_RANGE: &str = r#""ttl_range":[1,3]"#;

    /// The same range of TTLs, turned around, so that it runs backward.
    const BACKWARD_TTL_RANGE: &str = r#""ttl_range":[3,1]"#;

    /// One hop that answered a probe of the golden fixture.
    fn hop_at(ttl: u8, addr: &str, rtt_ms: f64, icmp: &str) -> Hop {
        Hop {
            ttl,
            addr: address(addr),
            rtt_ms,
            icmp: icmp.to_owned(),
        }
    }

    /// The first round of the first run of the golden fixture.
    ///
    /// Every TTL that the round probed answered, and the last one is the
    /// target.
    fn a_first_fixture_round() -> Record {
        Record::Round(RoundRecord {
            run: RunId::from(RUN),
            seq: 1,
            ts: moment("2026-08-18T12:00:01.127Z"),
            dur_ms: 1004,
            ttl_range: TtlRange::new(1, 3).expect("the fixture range must hold"),
            reached: true,
            hops: vec![
                hop_at(1, FIRST_HOP, 1.23, TIME_EXCEEDED),
                hop_at(3, TARGET_ADDRESS, 24.10, ECHO_REPLY),
            ],
        })
    }

    /// The second round of the first run of the golden fixture.
    ///
    /// The hop at TTL 3 did not answer, so the round holds one hop, and the
    /// range of TTLs still names the three TTLs that the round probed.
    fn a_second_fixture_round() -> Record {
        Record::Round(RoundRecord {
            run: RunId::from(RUN),
            seq: 2,
            ts: moment("2026-08-18T12:00:02.131Z"),
            dur_ms: 1002,
            ttl_range: TtlRange::new(1, 3).expect("the fixture range must hold"),
            reached: false,
            hops: vec![hop_at(1, FIRST_HOP, 1.41, TIME_EXCEEDED)],
        })
    }

    /// The record that closes the first run of the golden fixture.
    fn a_first_fixture_end() -> Record {
        Record::End(EndRecord {
            run: RunId::from(RUN),
            ts: moment("2026-08-18T12:00:03.000Z"),
            rounds: 2,
            reason: EndReason::Quit,
        })
    }

    /// The record that opens the second run of the golden fixture.
    fn a_second_fixture_run() -> Record {
        Record::Run(RunRecord {
            run: RunId::from(SECOND_RUN),
            krt: KRT_DIRTY.to_owned(),
            source: SourceLabel {
                addr: address(SECOND_SOURCE),
                kind: SourceKind::Local,
            },
            target: Target {
                arg: SECOND_TARGET.to_owned(),
                addr: address(SECOND_TARGET_ADDRESS),
                family: Family::Ipv4,
            },
            config: RunConfig {
                interval_ms: 500,
                protocol: Protocol::Udp,
                first_ttl: 1,
                max_ttl: 20,
                multipath: Multipath::Paris,
                privilege: Privilege::Privileged,
                dns: false,
            },
            host: MACHINE.to_owned(),
        })
    }

    /// The one round of the second run of the golden fixture.
    fn a_second_fixture_run_round() -> Record {
        Record::Round(RoundRecord {
            run: RunId::from(SECOND_RUN),
            seq: 1,
            ts: moment("2026-08-19T09:30:00.503Z"),
            dur_ms: 503,
            ttl_range: TtlRange::new(1, 2).expect("the fixture range must hold"),
            reached: true,
            hops: vec![
                hop_at(1, SECOND_FIRST_HOP, 0.87, TIME_EXCEEDED),
                hop_at(2, SECOND_TARGET_ADDRESS, 12.5, ECHO_REPLY),
            ],
        })
    }

    /// The record that closes the second run of the golden fixture.
    fn a_second_fixture_end() -> Record {
        Record::End(EndRecord {
            run: RunId::from(SECOND_RUN),
            ts: moment("2026-08-19T09:30:00.510Z"),
            rounds: 1,
            reason: EndReason::Rounds,
        })
    }

    /// The records that the golden fixture holds, in file order.
    ///
    /// The fixture holds nine lines, and the reader skips the one line whose
    /// `type` this build does not know, so eight records remain.
    ///
    /// The first two records are the two lines that the design writes, and the
    /// tests above already compare them against `RUN_LINE` and `NAME_LINE`.
    fn expected_records() -> Vec<Record> {
        vec![
            a_run_record(),
            a_name_record(),
            a_first_fixture_round(),
            a_second_fixture_round(),
            a_first_fixture_end(),
            a_second_fixture_run(),
            a_second_fixture_run_round(),
            a_second_fixture_end(),
        ]
    }

    /// The text of the golden fixture.
    fn fixture_text() -> String {
        fs::read_to_string(FIXTURE).expect("the golden fixture must read")
    }

    /// The records of the golden fixture.
    fn fixture_recording() -> Recording {
        Recording::read(Path::new(FIXTURE)).expect("the golden fixture must read")
    }

    /// Writes a copy of the golden fixture, with one text replaced.
    ///
    /// The path of the copy keys on the process and on the nanosecond, so two
    /// runs of one test stay apart. The copy goes away with the value.
    fn mutated_fixture(label: &str, from: &str, to: &str) -> TempFile {
        let text = fixture_text();
        assert!(text.contains(from), "the golden fixture holds `{from}`");
        TempFile::new(label, text.replace(from, to).as_bytes())
    }

    #[test]
    fn the_golden_fixture_parses_to_the_records_it_names() {
        let recording = fixture_recording();
        assert_eq!(recording.records(), expected_records());
        assert_eq!(recording.truncated(), None);
    }

    /// The committed fixture is text that the writer of this build produces.
    /// The fixture therefore cannot drift into a shape that the tool does not
    /// write.
    #[test]
    fn the_golden_fixture_holds_the_lines_that_this_build_writes() {
        let text = fixture_text();
        assert!(
            text.ends_with(NEWLINE_CHAR),
            "a newline ends the golden fixture"
        );
        assert_eq!(text.lines().count(), FIXTURE_LINES);

        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.contains(WEATHER_TYPE))
            .collect();
        assert_eq!(
            lines.len(),
            FIXTURE_LINES - 1,
            "the reader skips the one line whose type this build does not know"
        );

        let recording = fixture_recording();
        assert_eq!(recording.records().len(), lines.len());
        for (record, line) in recording.records().iter().zip(&lines) {
            assert_eq!(line_of(record), *line);
        }
    }

    #[test]
    fn the_golden_fixture_holds_two_runs_and_each_one_holds_its_own_records() {
        let recording = fixture_recording();
        assert_eq!(
            recording.run_ids(),
            [RunId::from(RUN), RunId::from(SECOND_RUN)]
        );

        let first = recording
            .run(&RunId::from(RUN))
            .expect("the golden fixture holds the first run");
        assert_eq!(seqs_of(&first), [1, 2]);
        assert_eq!(hosts_of(&first), [FIRST_HOST]);
        assert_eq!(
            first.end().expect("the first run ends").reason,
            EndReason::Quit
        );

        let second = recording
            .run(&RunId::from(SECOND_RUN))
            .expect("the golden fixture holds the second run");
        assert_eq!(seqs_of(&second), [1]);
        assert!(hosts_of(&second).is_empty(), "the second run reads no name");
        assert_eq!(
            second.end().expect("the second run ends").reason,
            EndReason::Rounds
        );

        let last = recording
            .last_run()
            .expect("the golden fixture holds two runs");
        assert_eq!(last, second);
    }

    /// A TTL that the round probed and that did not answer is absent from the
    /// hops, and the range of TTLs still holds it.
    #[test]
    fn a_hop_that_did_not_answer_is_absent_from_the_round_of_the_golden_fixture() {
        let recording = fixture_recording();
        let run = recording
            .run(&RunId::from(RUN))
            .expect("the golden fixture holds the first run");
        let round = *run
            .rounds()
            .get(1)
            .expect("the first run of the golden fixture makes two rounds");
        assert_eq!(round.seq, 2);
        assert_eq!(round.hops.len(), 1, "one hop of the round answered");
        assert_eq!(round.ttl_range.first(), 1);
        assert_eq!(
            round.ttl_range.last(),
            3,
            "the round probed ttl 3, the last ttl of its range"
        );
        assert!(
            !round.hops.iter().any(|answer| answer.ttl == 3),
            "the hop at ttl 3 did not answer, so the round holds no hop at ttl 3"
        );
    }

    /// A copy of the fixture with one field renamed must not read. A guard that
    /// accepts a renamed field is a guard that matches anything.
    #[test]
    fn a_copy_of_the_golden_fixture_with_a_renamed_field_does_not_read() {
        let copy = mutated_fixture("renamed-field", RTT_FIELD, RENAMED_RTT_FIELD);
        assert!(
            Recording::read(copy.path()).is_err(),
            "a reader that takes {RENAMED_RTT_FIELD} in place of {RTT_FIELD} matches anything"
        );
    }

    /// A copy of the fixture with one value changed must read, and must give
    /// records that differ from the ones the fixture names.
    ///
    /// This half proves that the comparison itself fires. The renamed field
    /// alone proves less, because the parser refuses the rename before any
    /// comparison runs.
    #[test]
    fn a_copy_of_the_golden_fixture_with_a_changed_value_reads_back_as_other_records() {
        let copy = mutated_fixture("changed-value", ROUND_DURATION, CHANGED_ROUND_DURATION);
        let recording = Recording::read(copy.path()).expect("a changed number is still one record");
        assert_eq!(
            recording.records().len(),
            expected_records().len(),
            "the copy still holds every record of the golden fixture"
        );
        assert_ne!(
            recording.records(),
            expected_records(),
            "a comparison that takes {CHANGED_ROUND_DURATION} in place of {ROUND_DURATION} compares nothing"
        );
    }

    /// A copy of the fixture with a range that runs backward must not read. The
    /// guard of the range fires through the parse of a file, and not only
    /// through the constructor.
    #[test]
    fn a_copy_of_the_golden_fixture_with_a_backward_range_does_not_read() {
        let copy = mutated_fixture("backward-range", FIXTURE_TTL_RANGE, BACKWARD_TTL_RANGE);
        assert!(
            Recording::read(copy.path()).is_err(),
            "a reader that takes the range {BACKWARD_TTL_RANGE} matches anything"
        );
    }
}
