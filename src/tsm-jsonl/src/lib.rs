//! Header-first JSONL append and read for tsm session log files.
//!
//! A tsm session log file `<session-id>.jsonl` has this structure:
//! - Line 1: exactly one [`Header`] record (JSON object with `type: "header"`)
//! - Lines 2..N: zero or more [`PrecmdRecord`] lines (JSON object with `type: "precmd"`)
//!
//! Every line is a single JSON object terminated by `\n`. UTF-8.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors emitted by the tsm-jsonl crate.
#[derive(Debug, Error)]
pub enum JsonlError {
    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// `append_record` was called on a file that contains no header.
    #[error("cannot append record: file has no header")]
    MissingHeader,

    /// `append_header` was called on a file that already contains data.
    #[error("cannot append header: file already contains data")]
    DuplicateHeader,

    /// `read_all` could not parse line 1 as a [`Header`].
    #[error("malformed first line (expected Header): {line}")]
    MalformedFirstLine { line: String },

    /// `read_all` could not parse a non-header line as a [`PrecmdRecord`].
    #[error("malformed record on line {line_number}: {line}")]
    MalformedRecordLine { line_number: u64, line: String },
}

/// Discriminator tag for the header record. Always serializes to `"header"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeaderKind {
    /// The single legal value.
    #[serde(rename = "header")]
    Header,
}

/// Discriminator tag for a precmd record. Always serializes to `"precmd"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrecmdKind {
    /// The single legal value.
    #[serde(rename = "precmd")]
    Precmd,
}

/// Zellij session tuple stub.
///
/// All three fields are [`None`] outside of Zellij. Future Zellij integration will populate
/// these from environment variables and `zellij action` queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TupleStub {
    /// Zellij session name, if any.
    pub zellij_session: Option<String>,
    /// Zellij tab name, if any.
    pub tab: Option<String>,
    /// Zellij pane ordinal, rendered as a string (e.g. `"1"`), if any.
    pub pane_ordinal_str: Option<String>,
}

/// Header line written once per session log file (always line 1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    /// Discriminator; serialized as `type`. Always `"header"` on the wire.
    #[serde(rename = "type")]
    pub kind: HeaderKind,
    /// Schema version (start at 1).
    pub schema_version: u32,
    /// tsm binary version string (e.g. `"0.1.0 (abc1234, clean)"`).
    pub tsm_version: String,
    /// Hostname.
    pub hostname: String,
    /// Terminal program (env `$TERM_PROGRAM` or `"unknown"`).
    pub terminal_program: String,
    /// Session tuple stub for this slice (Zellij integration is later).
    pub tuple: TupleStub,
    /// RFC3339 timestamp string for the moment this header was written.
    pub created_at: String,
}

/// Per-prompt record written on every precmd invocation after the header.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrecmdRecord {
    /// Discriminator; serialized as `type`. Always `"precmd"` on the wire.
    #[serde(rename = "type")]
    pub kind: PrecmdKind,
    /// RFC3339 timestamp.
    pub at: String,
    /// Working directory at prompt return.
    pub cwd: String,
    /// Exit code of the just-finished command.
    pub exit_code: i32,
    /// Verbatim text of the last command (may be empty for the first prompt of a session).
    pub last_command: String,
    /// Map of env var names to values, *after* redaction (the redactor lives in `tsm record`).
    pub env: BTreeMap<String, String>,
    /// Sorted list of env var names whose values were redacted.
    pub redacted_keys: Vec<String>,
}

/// Full parsed contents of a session log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLog {
    /// The header on line 1.
    pub header: Header,
    /// All precmd records after the header, in file order.
    pub records: Vec<PrecmdRecord>,
}

/// Returns the byte length of `path`, or `None` if the file does not exist.
///
/// Any I/O error other than [`NotFound`](std::io::ErrorKind::NotFound) is propagated.
fn file_size_if_exists(path: &Path) -> Result<Option<u64>, JsonlError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(JsonlError::Io(e)),
    }
}

/// Serialize one record and append it (plus a trailing `\n`) to `path` in a single
/// `write_all` call so the append stays atomic for lines ≤ `PIPE_BUF` bytes on POSIX.
fn append_line<T: Serialize>(path: &Path, value: &T) -> Result<(), JsonlError> {
    let mut buf = serde_json::to_vec(value)?;
    buf.push(b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&buf)?;
    Ok(())
}

/// Append a header to `path`.
///
/// The file must be empty (size 0 or nonexistent). Errors if the file already has any content.
///
/// # Errors
///
/// Returns [`JsonlError::DuplicateHeader`] if `path` already has data, or [`JsonlError::Io`] /
/// [`JsonlError::Json`] for underlying failures.
pub fn append_header(path: &Path, header: &Header) -> Result<(), JsonlError> {
    // Enforce empty-file invariant before opening for append. If the file exists with size > 0,
    // refuse and do NOT touch it.
    if let Some(size) = file_size_if_exists(path)? {
        if size > 0 {
            return Err(JsonlError::DuplicateHeader);
        }
    }
    append_line(path, header)
}

/// Append a precmd record to `path`.
///
/// The file must already contain a header. Errors otherwise.
///
/// # Errors
///
/// Returns [`JsonlError::MissingHeader`] if `path` is empty or absent, or [`JsonlError::Io`] /
/// [`JsonlError::Json`] for underlying failures.
pub fn append_record(path: &Path, record: &PrecmdRecord) -> Result<(), JsonlError> {
    // Header-first invariant: file must exist and be non-empty. We use the size as a cheap proxy
    // for "has a header" — fully validating line 1 on every append would defeat the point of
    // append-only logging. read_all enforces the structural invariant on read.
    match file_size_if_exists(path)? {
        Some(size) if size > 0 => append_line(path, record),
        _ => Err(JsonlError::MissingHeader),
    }
}

/// Read an entire session log; returns the parsed header and all subsequent records.
///
/// # Errors
///
/// Returns [`JsonlError::MalformedFirstLine`] if line 1 is missing or unparseable as a
/// [`Header`], [`JsonlError::MalformedRecordLine`] if any subsequent line cannot be parsed as a
/// [`PrecmdRecord`], or [`JsonlError::Io`] for underlying failures.
pub fn read_all(path: &Path) -> Result<SessionLog, JsonlError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Line 1: must be a Header. An empty file yields None and is treated as MalformedFirstLine.
    let first_line = match lines.next() {
        Some(Ok(line)) => line,
        Some(Err(e)) => return Err(JsonlError::Io(e)),
        None => {
            return Err(JsonlError::MalformedFirstLine {
                line: String::new(),
            });
        }
    };
    let header: Header =
        serde_json::from_str(&first_line).map_err(|_| JsonlError::MalformedFirstLine {
            line: first_line.clone(),
        })?;

    // Lines 2..N: each must parse as a PrecmdRecord. Track the 1-indexed line number for
    // error reporting (line 1 = header, so the first record is line 2).
    let mut records = Vec::new();
    let mut line_number: u64 = 1;
    for line_result in lines {
        line_number = line_number.saturating_add(1);
        let line = line_result?;
        let record: PrecmdRecord =
            serde_json::from_str(&line).map_err(|_| JsonlError::MalformedRecordLine {
                line_number,
                line: line.clone(),
            })?;
        records.push(record);
    }

    Ok(SessionLog { header, records })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn sample_header() -> Header {
        Header {
            kind: HeaderKind::Header,
            schema_version: 1,
            tsm_version: "0.1.0 (abc1234, clean)".to_string(),
            hostname: "test-host".to_string(),
            terminal_program: "iTerm.app".to_string(),
            tuple: TupleStub {
                zellij_session: None,
                tab: None,
                pane_ordinal_str: None,
            },
            created_at: "2026-05-13T12:00:00Z".to_string(),
        }
    }

    fn sample_record(seq: u32) -> PrecmdRecord {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), format!("/usr/bin:/bin:{seq}"));
        env.insert("HOME".to_string(), "/Users/test".to_string());
        PrecmdRecord {
            kind: PrecmdKind::Precmd,
            at: format!("2026-05-13T12:00:{seq:02}Z"),
            cwd: format!("/tmp/dir-{seq}"),
            exit_code: 0,
            last_command: format!("echo hello {seq}"),
            env,
            redacted_keys: vec!["AWS_SECRET_ACCESS_KEY".to_string()],
        }
    }

    #[test]
    fn append_header_to_empty_file_writes_one_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"").expect("create empty file");
        let header = sample_header();
        append_header(&path, &header).expect("append_header");
        let contents = fs::read_to_string(&path).expect("read");
        let expected = serde_json::to_string(&header).expect("serialize") + "\n";
        assert_eq!(contents, expected);
    }

    #[test]
    fn append_header_to_nonexistent_file_creates_it() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        assert!(!path.exists());
        let header = sample_header();
        append_header(&path, &header).expect("append_header");
        assert!(path.exists());
        let contents = fs::read_to_string(&path).expect("read");
        let expected = serde_json::to_string(&header).expect("serialize") + "\n";
        assert_eq!(contents, expected);
    }

    #[test]
    fn append_header_to_nonempty_file_returns_duplicate_header() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let header = sample_header();
        append_header(&path, &header).expect("first append_header");
        let before = fs::read(&path).expect("read before");
        let err = append_header(&path, &header).expect_err("second append_header must fail");
        assert!(
            matches!(err, JsonlError::DuplicateHeader),
            "expected DuplicateHeader, got: {err:?}"
        );
        let after = fs::read(&path).expect("read after");
        assert_eq!(before, after, "file must not be mutated");
    }

    #[test]
    fn append_record_before_header_returns_missing_header() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let record = sample_record(1);
        let err = append_record(&path, &record).expect_err("append_record on empty must fail");
        assert!(
            matches!(err, JsonlError::MissingHeader),
            "expected MissingHeader, got: {err:?}"
        );
        // File must remain empty or nonexistent.
        let exists_and_empty = !path.exists() || fs::metadata(&path).expect("metadata").len() == 0;
        assert!(exists_and_empty, "file must remain empty or absent");
    }

    #[test]
    fn append_record_after_header_writes_one_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let header = sample_header();
        let record = sample_record(1);
        append_header(&path, &header).expect("append_header");
        append_record(&path, &record).expect("append_record");
        let contents = fs::read_to_string(&path).expect("read");
        let header_line = serde_json::to_string(&header).expect("serialize header");
        let record_line = serde_json::to_string(&record).expect("serialize record");
        let expected = format!("{header_line}\n{record_line}\n");
        assert_eq!(contents, expected);
    }

    #[test]
    fn multiple_records_after_header_each_append() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let header = sample_header();
        append_header(&path, &header).expect("append_header");
        let r1 = sample_record(1);
        let r2 = sample_record(2);
        let r3 = sample_record(3);
        append_record(&path, &r1).expect("append r1");
        append_record(&path, &r2).expect("append r2");
        append_record(&path, &r3).expect("append r3");
        let contents = fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 4, "expected 4 lines (header + 3 records)");
        assert_eq!(lines[0], serde_json::to_string(&header).expect("ser h"));
        assert_eq!(lines[1], serde_json::to_string(&r1).expect("ser r1"));
        assert_eq!(lines[2], serde_json::to_string(&r2).expect("ser r2"));
        assert_eq!(lines[3], serde_json::to_string(&r3).expect("ser r3"));
    }

    #[test]
    fn read_all_round_trips_header_only() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let header = sample_header();
        append_header(&path, &header).expect("append_header");
        let log = read_all(&path).expect("read_all");
        assert_eq!(log.header, header);
        assert!(log.records.is_empty());
    }

    #[test]
    fn read_all_round_trips_header_and_records() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let header = sample_header();
        append_header(&path, &header).expect("append_header");
        let r1 = sample_record(1);
        let r2 = sample_record(2);
        let r3 = sample_record(3);
        append_record(&path, &r1).expect("append r1");
        append_record(&path, &r2).expect("append r2");
        append_record(&path, &r3).expect("append r3");
        let log = read_all(&path).expect("read_all");
        assert_eq!(log.header, header);
        assert_eq!(log.records, vec![r1, r2, r3]);
    }

    #[test]
    fn read_all_on_empty_file_returns_malformed_first_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"").expect("write empty");
        let err = read_all(&path).expect_err("read_all on empty must fail");
        assert!(
            matches!(err, JsonlError::MalformedFirstLine { .. }),
            "expected MalformedFirstLine, got: {err:?}"
        );
    }

    #[test]
    fn read_all_with_garbage_first_line_returns_malformed_first_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"this is not json\n").expect("write garbage");
        let err = read_all(&path).expect_err("read_all on garbage must fail");
        match err {
            JsonlError::MalformedFirstLine { line } => {
                assert_eq!(line, "this is not json");
            }
            other => panic!("expected MalformedFirstLine, got: {other:?}"),
        }
    }

    #[test]
    fn read_all_with_garbage_record_line_returns_malformed_record_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let header = sample_header();
        let r1 = sample_record(1);
        append_header(&path, &header).expect("append_header");
        append_record(&path, &r1).expect("append r1");
        // Manually append a garbage line as line 3.
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open append");
            f.write_all(b"not a record\n").expect("write garbage");
        }
        let err = read_all(&path).expect_err("read_all with bad record must fail");
        match err {
            JsonlError::MalformedRecordLine { line_number, line } => {
                assert_eq!(line_number, 3, "garbage is on line 3");
                assert_eq!(line, "not a record");
            }
            other => panic!("expected MalformedRecordLine, got: {other:?}"),
        }
    }

    #[test]
    fn header_wire_format_has_type_header() {
        let header = sample_header();
        let value = serde_json::to_value(&header).expect("to_value");
        assert_eq!(value["type"], "header");
    }

    #[test]
    fn precmd_wire_format_has_type_precmd() {
        let record = sample_record(1);
        let value = serde_json::to_value(&record).expect("to_value");
        assert_eq!(value["type"], "precmd");
    }

    #[test]
    fn cannot_deserialize_header_as_precmd_record() {
        let header = sample_header();
        let header_json = serde_json::to_string(&header).expect("serialize header");
        let result: Result<PrecmdRecord, _> = serde_json::from_str(&header_json);
        assert!(
            result.is_err(),
            "header JSON must not parse as PrecmdRecord, got: {result:?}"
        );
    }
}
