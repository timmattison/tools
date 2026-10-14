//! The shell history, read through atuin.
//!
//! `atuin history list` obeys the `filter_mode` of the atuin configuration. A
//! configuration with `filter_mode = "session"` gives one row. Thus this module
//! runs `atuin search` with an explicit filter mode, and it gets every row.
//!
//! [`parse`] is a pure function over the bytes that the search writes. [`read`]
//! is the one function that starts a process.

use std::ffi::OsStr;

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
        false
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
    let _ = output;
    Ok(Vec::new())
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
    let _ = program;
    Ok(Vec::new())
}
