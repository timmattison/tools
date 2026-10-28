//! The fork log: one JSON line for each session that `crap` forks.
//!
//! The log is the file `forks.jsonl` in the `crap` directory under the data
//! directory of the user. Each line is one [`ForkRecord`]. The log only grows.
//! A writer appends one line with one `write` call, so two forks at the same
//! time cannot mix their lines.
//!
//! The functions in this module take paths as arguments and never read the
//! environment. A test thus uses a temporary path and never touches the real
//! log.

use std::path::{Path, PathBuf};

/// Name of the directory under the data directory of the user.
pub const LOG_DIR_NAME: &str = "crap";
/// Name of the log file in that directory.
pub const LOG_FILE_NAME: &str = "forks.jsonl";

/// The way a fork was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForkKind {
    /// `crap --here <id>`: the fork of a session into the current directory.
    Here,
    /// A fork made for a cross-user resume.
    ForkAt,
}

/// One line of the fork log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForkRecord {
    /// Time of the fork, in RFC 3339 and UTC, for example `2026-10-28T01:51:22Z`.
    /// The caller formats it.
    pub time: String,
    /// Id of the session that was forked.
    pub source_id: String,
    /// Id of the new session.
    pub fork_id: String,
    /// Account that owns the source transcript.
    pub source_user: String,
    /// Working directory that the source transcript records. It is JSON `null`
    /// when the transcript records none.
    pub source_dir: Option<String>,
    /// Directory where the fork runs.
    pub fork_dir: String,
    /// The way the fork was made.
    pub kind: ForkKind,
}

/// The records of a log, and the count of lines that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkLog {
    /// The records that parsed, in file order.
    pub records: Vec<ForkRecord>,
    /// The count of non-blank lines that did not parse as a record.
    pub unreadable_lines: usize,
}

/// Appends one record to the log as one line.
///
/// The function creates the parent directory when it is missing. It opens the
/// file with `O_APPEND`. On unix, a new file gets mode `0o600`, because the log
/// names session ids and directories of the user. The mode applies only when
/// the file is created.
///
/// The function builds the whole line (JSON and `\n`) in one buffer and writes
/// it with one `write` call. With `O_APPEND`, one `write` of a small buffer
/// lands as one unit, so two forks at the same time cannot mix their lines.
/// `write_all` could split the line over two calls.
///
/// # Errors
///
/// Returns an error when the directory cannot be created, when the file cannot
/// be opened, when the record cannot be serialized, or when the write fails or
/// is short.
pub fn append_fork_record(log: &Path, record: &ForkRecord) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    line.push('\n');

    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(log)?;

    let written = file.write(line.as_bytes())?;
    if written != line.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            format!(
                "short write to the fork log: {written} of {} bytes",
                line.len()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fork_id: &str, kind: ForkKind, source_dir: Option<&str>) -> ForkRecord {
        ForkRecord {
            time: "2026-10-28T01:51:22Z".to_string(),
            source_id: "source-1".to_string(),
            fork_id: fork_id.to_string(),
            source_user: "alice".to_string(),
            source_dir: source_dir.map(str::to_string),
            fork_dir: "/work/here".to_string(),
            kind,
        }
    }

    #[test]
    fn append_writes_one_json_line_with_seven_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("deep").join("er").join(LOG_FILE_NAME);

        append_fork_record(&log, &sample("f1", ForkKind::Here, None)).unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
        let value: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let object = value.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut want = vec![
            "time",
            "source_id",
            "fork_id",
            "source_user",
            "source_dir",
            "fork_dir",
            "kind",
        ];
        want.sort_unstable();
        assert_eq!(keys, want);
        assert_eq!(object["kind"], "here");
        assert!(object["source_dir"].is_null());

        let second = dir.path().join("second.jsonl");
        append_fork_record(&second, &sample("f2", ForkKind::ForkAt, Some("/src"))).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&second).unwrap().trim()).unwrap();
        assert_eq!(value["kind"], "fork-at");
        assert_eq!(value["source_dir"], "/src");
    }
}
