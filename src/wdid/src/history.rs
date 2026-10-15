//! The shell history, read through atuin.
//!
//! `atuin history list` obeys the `filter_mode` of the atuin configuration. A
//! configuration with `filter_mode = "session"` gives one row. Thus this module
//! runs `atuin search` with an explicit filter mode, and it gets every row.
//!
//! [`parse`] is a pure function over the bytes that the search writes. [`read`]
//! is the one function that starts a process.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::process::{Command, Stdio};

/// The program that holds the history.
pub const ATUIN: &str = "atuin";

/// The arguments of the one search that gives every row.
///
/// `atuin history list` obeys `filter_mode` of the atuin config, and a config
/// with `filter_mode = "session"` gives one row. This search gives all rows.
pub const SEARCH_ARGUMENTS: [&str; 10] = [
    "search",
    "--filter-mode",
    "global",
    "--include-duplicates",
    "--print0",
    "--timezone",
    "+0",
    "--format",
    "{time}\t{exit}\t{host}\t{user}\t{command}",
    "",
];

/// The byte that ends each record of the output.
const RECORD_SEPARATOR: u8 = 0;

/// The byte that separates the fields of a record.
const FIELD_SEPARATOR: u8 = b'\t';

/// The number of fields in a record. The last field is the command, and it
/// keeps every tab that it holds.
const FIELD_COUNT: usize = 5;

/// The format of the time field. The search states `--timezone +0`.
const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// The exit code that a shell gives when it cannot find a command.
pub const COMMAND_NOT_FOUND: i64 = 127;

/// The exit code of a process, as atuin records it.
///
/// atuin records -1 when it has no exit code, for example for `exit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit(i64);

impl Exit {
    /// Makes an exit code from the number that atuin recorded.
    #[must_use]
    pub const fn new(code: i64) -> Self {
        Self(code)
    }

    /// Gives the number that atuin recorded.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// True for 127: the shell did not find the command.
    #[must_use]
    pub const fn is_command_not_found(self) -> bool {
        self.0 == COMMAND_NOT_FOUND
    }
}

/// One row of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The time that the command started, in UTC (the search states `--timezone +0`).
    pub time: chrono::NaiveDateTime,
    /// The exit code of the command.
    pub exit: Exit,
    /// The host that ran the command.
    pub host: String,
    /// The user that ran the command.
    pub user: String,
    /// The text of the command line. It keeps every tab and newline.
    pub command: String,
}

/// The reasons that the history cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// The program does not exist.
    #[error("cannot start {program}: it is not installed or it is not on PATH. wdid reads the shell history through atuin")]
    NotInstalled {
        /// The program name or path that the spawn used.
        program: String,
    },
    /// The program exists, but the system cannot start it.
    #[error("cannot start {program}: {source}")]
    Spawn {
        /// The program name or path that the spawn used.
        program: String,
        /// The error of the system.
        source: std::io::Error,
    },
    /// The search ended with a non-zero status.
    #[error("atuin search failed ({status}): {stderr}")]
    Failed {
        /// The text of the exit status.
        status: String,
        /// The standard error of the search, trimmed.
        stderr: String,
    },
    /// The search gave no rows.
    #[error("atuin search gave no history rows. An empty history gives no report, because zero uses then means nothing")]
    Empty,
    /// One record of the output is not in the format that wdid asked for.
    #[error("atuin search gave a record that wdid cannot read (record {record}): {reason}")]
    Malformed {
        /// The number of the record, from 1.
        record: usize,
        /// The text that says what is wrong.
        reason: String,
    },
}

/// Parses the NUL-separated output of the search. Pure.
///
/// # Errors
/// Gives [`HistoryError::Malformed`] for a record that has fewer than five
/// fields, a time that is not `%Y-%m-%d %H:%M:%S`, or an exit code that is not
/// a number. This function never gives [`HistoryError::Empty`].
pub fn parse(output: &[u8]) -> Result<Vec<HistoryEntry>, HistoryError> {
    let mut records: Vec<&[u8]> = output.split(|byte| *byte == RECORD_SEPARATOR).collect();
    if records.last().is_some_and(|last| last.is_empty()) {
        records.pop();
    }
    records
        .into_iter()
        .enumerate()
        .map(|(index, record)| parse_record(record, index + 1))
        .collect()
}

/// Parses one record. `number` is the position of the record, from 1.
fn parse_record(record: &[u8], number: usize) -> Result<HistoryEntry, HistoryError> {
    let malformed = |reason: String| HistoryError::Malformed {
        record: number,
        reason,
    };
    let mut fields = record.splitn(FIELD_COUNT, |byte| *byte == FIELD_SEPARATOR);
    let (Some(time), Some(exit), Some(host), Some(user), Some(command)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(malformed(format!("it has fewer than {FIELD_COUNT} fields")));
    };
    let time_text = String::from_utf8_lossy(time);
    let time = chrono::NaiveDateTime::parse_from_str(&time_text, TIME_FORMAT)
        .map_err(|error| malformed(format!("the time {time_text:?} is not valid: {error}")))?;
    let exit_text = String::from_utf8_lossy(exit);
    let exit = exit_text
        .parse::<i64>()
        .map_err(|error| malformed(format!("the exit code {exit_text:?} is not valid: {error}")))?;
    Ok(HistoryEntry {
        time,
        exit: Exit::new(exit),
        host: String::from_utf8_lossy(host).into_owned(),
        user: String::from_utf8_lossy(user).into_owned(),
        command: String::from_utf8_lossy(command).into_owned(),
    })
}

/// Spawns `<program> SEARCH_ARGUMENTS...` and parses its output.
///
/// `program` is a name that the PATH resolves, or a path.
///
/// # Errors
/// Gives [`HistoryError::NotInstalled`] or [`HistoryError::Spawn`] when the
/// process does not start, [`HistoryError::Failed`] for a non-zero status,
/// [`HistoryError::Empty`] for zero rows, and [`HistoryError::Malformed`] from
/// [`parse`].
pub fn read(program: &OsStr) -> Result<Vec<HistoryEntry>, HistoryError> {
    let program_text = program.to_string_lossy().into_owned();
    let output = Command::new(program)
        .args(SEARCH_ARGUMENTS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| {
            if source.kind() == ErrorKind::NotFound {
                HistoryError::NotInstalled {
                    program: program_text.clone(),
                }
            } else {
                HistoryError::Spawn {
                    program: program_text.clone(),
                    source,
                }
            }
        })?;
    if !output.status.success() {
        return Err(HistoryError::Failed {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let entries = parse(&output.stdout)?;
    if entries.is_empty() {
        return Err(HistoryError::Empty);
    }
    Ok(entries)
}
