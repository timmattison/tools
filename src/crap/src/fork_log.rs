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

use std::collections::HashSet;
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

/// Returns the path of the fork log under a data directory.
///
/// The path is `data_dir/crap/forks.jsonl`. The function is pure: the caller
/// passes the data directory.
#[must_use]
pub fn fork_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR_NAME).join(LOG_FILE_NAME)
}

/// Reads the fork log.
///
/// A missing file is an empty log. A line that does not parse as a
/// [`ForkRecord`] (a torn or hand-edited line) is skipped and counted in
/// [`ForkLog::unreadable_lines`]. Blank lines are ignored and not counted.
///
/// # Errors
///
/// Returns an error when the file exists and cannot be read.
pub fn read_fork_log(log: &Path) -> std::io::Result<ForkLog> {
    let text = match std::fs::read_to_string(log) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut records = Vec::new();
    let mut unreadable_lines = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<ForkRecord>(line) {
            Ok(record) => records.push(record),
            Err(_) => unreadable_lines += 1,
        }
    }
    Ok(ForkLog {
        records,
        unreadable_lines,
    })
}

/// The records whose source is the session `id`, in log order.
///
/// These are the forks of `id` that the log knows.
#[must_use]
pub fn forks_of<'a>(records: &'a [ForkRecord], id: &str) -> Vec<&'a ForkRecord> {
    records
        .iter()
        .filter(|record| record.source_id == id)
        .collect()
}

/// The chain of sources of a session, from the session up to the oldest source
/// that the log knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChain<'a> {
    /// The record that made each link, newest first: the first record made the
    /// session that was asked about, the next record made its source, and so on.
    pub links: Vec<&'a ForkRecord>,
    /// The id that the log names a second time, when the chain loops. A
    /// hand-edited log can hold a loop. It is `None` for a normal chain.
    pub looped_at: Option<String>,
}

/// Follows `fork_id` to `source_id` from `id` up, and stops at a session that is
/// not a fork in the log, or at an id that was already visited.
///
/// When two records have the same `fork_id`, the first one in log order wins.
#[must_use]
pub fn source_chain<'a>(records: &'a [ForkRecord], id: &str) -> SourceChain<'a> {
    let mut visited: HashSet<&str> = HashSet::from([id]);
    let mut links = Vec::new();
    let mut looped_at = None;
    let mut current = id;
    while let Some(record) = records.iter().find(|record| record.fork_id == current) {
        links.push(record);
        if !visited.insert(record.source_id.as_str()) {
            looped_at = Some(record.source_id.clone());
            break;
        }
        current = record.source_id.as_str();
    }
    SourceChain { links, looped_at }
}

/// One fork of a session, with the answer to "is it saved".
#[derive(Debug, Clone, Copy)]
pub struct ForkLine<'a> {
    /// The log record of the fork.
    pub record: &'a ForkRecord,
    /// Whether a transcript of the fork exists now. The caller looks it up.
    pub saved: bool,
}

/// Formats the report of `crap --forks <id>`.
///
/// The report has a section for the forks of `id` and a section for its chain
/// of sources. A section with nothing in it does not print. When both are empty,
/// the report is one line that says so. The text has no colors and ends with a
/// newline.
#[must_use]
pub fn format_log_report(id: &str, forks: &[ForkLine<'_>], chain: &SourceChain<'_>) -> String {
    use std::fmt::Write;

    if forks.is_empty() && chain.links.is_empty() {
        return format!(
            "The fork log has no fork of {id}, and {id} is not a fork in the fork log.\n"
        );
    }
    let mut out = String::new();
    if !forks.is_empty() {
        let _ = writeln!(out, "Forks of {id} in the fork log:");
        for fork in forks {
            let status = if fork.saved { "saved" } else { "not saved" };
            let _ = writeln!(
                out,
                "  {}  {}  {status:<9}  {}",
                fork.record.fork_id, fork.record.time, fork.record.fork_dir
            );
        }
    }
    if !chain.links.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "Sources of {id} in the fork log:");
        for record in &chain.links {
            let kind = match record.kind {
                ForkKind::Here => "here",
                ForkKind::ForkAt => "fork-at",
            };
            let _ = writeln!(
                out,
                "  {} is a fork of {}, made {} ({kind}) in {}",
                record.fork_id, record.source_id, record.time, record.fork_dir
            );
        }
        match (&chain.looped_at, chain.links.last()) {
            (Some(repeated), _) => {
                let _ = writeln!(
                    out,
                    "  The chain stops: the fork log names {repeated} again."
                );
            }
            (None, Some(last)) => {
                let _ = writeln!(out, "  {} is not a fork in the fork log.", last.source_id);
            }
            (None, None) => {}
        }
    }
    out
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

    fn parse_lines(log: &Path) -> Vec<ForkRecord> {
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn two_appends_give_two_lines_in_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join(LOG_FILE_NAME);
        append_fork_record(&log, &sample("first", ForkKind::Here, None)).unwrap();
        append_fork_record(&log, &sample("second", ForkKind::ForkAt, Some("/s"))).unwrap();
        let ids: Vec<String> = parse_lines(&log).into_iter().map(|r| r.fork_id).collect();
        assert_eq!(ids, ["first", "second"]);
    }

    #[test]
    fn concurrent_appends_never_mix_lines() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 200;
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join(LOG_FILE_NAME);
        std::thread::scope(|scope| {
            for thread in 0..THREADS {
                let log = &log;
                scope.spawn(move || {
                    for n in 0..PER_THREAD {
                        let mut record = sample(&format!("t{thread}-n{n}"), ForkKind::Here, None);
                        record.fork_dir = "d".repeat(2_000);
                        append_fork_record(log, &record).unwrap();
                    }
                });
            }
        });
        let records = parse_lines(&log);
        assert_eq!(records.len(), THREADS * PER_THREAD);
        let ids: std::collections::HashSet<String> =
            records.into_iter().map(|r| r.fork_id).collect();
        let want: std::collections::HashSet<String> = (0..THREADS)
            .flat_map(|t| (0..PER_THREAD).map(move |n| format!("t{t}-n{n}")))
            .collect();
        assert_eq!(ids, want);
    }

    #[test]
    fn unwritable_path_gives_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let log = blocker.join("crap").join(LOG_FILE_NAME);
        assert!(append_fork_record(&log, &sample("f", ForkKind::Here, None)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn new_log_file_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join(LOG_FILE_NAME);
        append_fork_record(&log, &sample("f", ForkKind::Here, None)).unwrap();
        let mode = std::fs::metadata(&log).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn log_path_is_crap_forks_jsonl_under_the_data_dir() {
        assert_eq!(
            fork_log_path(Path::new("/d")),
            Path::new("/d/crap/forks.jsonl")
        );
    }

    #[test]
    fn reading_a_missing_log_gives_an_empty_log() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = read_fork_log(&dir.path().join("none.jsonl")).unwrap();
        assert!(log.records.is_empty());
        assert_eq!(log.unreadable_lines, 0);
    }

    #[test]
    fn reading_skips_and_counts_bad_lines_and_ignores_blank_ones() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        append_fork_record(&path, &sample("one", ForkKind::Here, None)).unwrap();
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(b"{\"time\":\"x\",\"sour\n\n   \n{\"not\":\"a record\"}\n")
                .unwrap();
        }
        append_fork_record(&path, &sample("two", ForkKind::ForkAt, Some("/s"))).unwrap();
        let log = read_fork_log(&path).unwrap();
        let ids: Vec<&str> = log.records.iter().map(|r| r.fork_id.as_str()).collect();
        assert_eq!(ids, ["one", "two"]);
        assert_eq!(log.unreadable_lines, 2);
    }

    fn link(source: &str, fork: &str) -> ForkRecord {
        let mut record = sample(fork, ForkKind::Here, None);
        record.source_id = source.to_string();
        record
    }

    fn fork_ids(records: &[&ForkRecord]) -> Vec<String> {
        records.iter().map(|r| r.fork_id.clone()).collect()
    }

    #[test]
    fn forks_of_returns_the_records_with_that_source_in_log_order() {
        let records = [
            link("a", "b"),
            link("x", "y"),
            link("a", "c"),
            link("b", "d"),
        ];
        assert_eq!(fork_ids(&forks_of(&records, "a")), ["b", "c"]);
        assert!(forks_of(&records, "d").is_empty());
    }

    #[test]
    fn source_chain_follows_each_fork_to_its_source() {
        let records = [link("a", "b"), link("b", "d")];
        let chain = source_chain(&records, "d");
        assert_eq!(fork_ids(&chain.links), ["d", "b"]);
        assert_eq!(chain.looped_at, None);
        assert!(source_chain(&records, "a").links.is_empty());
    }

    #[test]
    fn source_chain_takes_the_first_record_for_a_fork_id() {
        let records = [link("a", "b"), link("z", "b")];
        let chain = source_chain(&records, "b");
        assert_eq!(chain.links.len(), 1);
        assert_eq!(chain.links[0].source_id, "a");
    }

    #[test]
    fn source_chain_ends_on_a_loop_and_names_the_repeated_id() {
        let records = [link("b", "a"), link("a", "b")];
        let chain = source_chain(&records, "a");
        assert_eq!(fork_ids(&chain.links), ["a", "b"]);
        assert_eq!(chain.looped_at.as_deref(), Some("a"));
        let own = [link("a", "a")];
        let chain = source_chain(&own, "a");
        assert_eq!(chain.links.len(), 1);
        assert_eq!(chain.looped_at.as_deref(), Some("a"));
    }

    const ID_A: &str = "aaaaaaaa-0000-4000-8000-00000000000a";
    const ID_B: &str = "bbbbbbbb-0000-4000-8000-00000000000b";
    const ID_C: &str = "cccccccc-0000-4000-8000-00000000000c";

    fn made(source: &str, fork: &str, kind: ForkKind, dir: &str) -> ForkRecord {
        let mut record = link(source, fork);
        record.kind = kind;
        record.fork_dir = dir.to_string();
        record
    }

    #[test]
    fn report_for_forks_only() {
        let one = made(ID_A, ID_B, ForkKind::Here, "/w/one");
        let two = made(ID_A, ID_C, ForkKind::ForkAt, "/w/two");
        let forks = [
            ForkLine {
                record: &one,
                saved: true,
            },
            ForkLine {
                record: &two,
                saved: false,
            },
        ];
        let chain = SourceChain {
            links: Vec::new(),
            looped_at: None,
        };
        assert_eq!(
            format_log_report(ID_A, &forks, &chain),
            format!(
                "Forks of {ID_A} in the fork log:\n\
                 \x20 {ID_B}  2026-10-28T01:51:22Z  saved      /w/one\n\
                 \x20 {ID_C}  2026-10-28T01:51:22Z  not saved  /w/two\n"
            )
        );
    }

    #[test]
    fn report_for_a_chain_only() {
        let first = made(ID_B, ID_C, ForkKind::ForkAt, "/w/c");
        let second = made(ID_A, ID_B, ForkKind::Here, "/w/b");
        let chain = SourceChain {
            links: vec![&first, &second],
            looped_at: None,
        };
        assert_eq!(
            format_log_report(ID_C, &[], &chain),
            format!(
                "Sources of {ID_C} in the fork log:\n\
                 \x20 {ID_C} is a fork of {ID_B}, made 2026-10-28T01:51:22Z (fork-at) in /w/c\n\
                 \x20 {ID_B} is a fork of {ID_A}, made 2026-10-28T01:51:22Z (here) in /w/b\n\
                 \x20 {ID_A} is not a fork in the fork log.\n"
            )
        );
    }

    #[test]
    fn report_for_both_sections_has_a_blank_line_between() {
        let child = made(ID_B, ID_C, ForkKind::Here, "/w/c");
        let parent = made(ID_A, ID_B, ForkKind::Here, "/w/b");
        let forks = [ForkLine {
            record: &child,
            saved: true,
        }];
        let chain = SourceChain {
            links: vec![&parent],
            looped_at: None,
        };
        assert_eq!(
            format_log_report(ID_B, &forks, &chain),
            format!(
                "Forks of {ID_B} in the fork log:\n\
                 \x20 {ID_C}  2026-10-28T01:51:22Z  saved      /w/c\n\
                 \n\
                 Sources of {ID_B} in the fork log:\n\
                 \x20 {ID_B} is a fork of {ID_A}, made 2026-10-28T01:51:22Z (here) in /w/b\n\
                 \x20 {ID_A} is not a fork in the fork log.\n"
            )
        );
    }

    #[test]
    fn report_for_neither_is_one_line() {
        let chain = SourceChain {
            links: Vec::new(),
            looped_at: None,
        };
        assert_eq!(
            format_log_report(ID_A, &[], &chain),
            format!(
                "The fork log has no fork of {ID_A}, and {ID_A} is not a fork in the fork log.\n"
            )
        );
    }

    #[test]
    fn report_for_a_loop_says_where_the_chain_stops() {
        let first = made(ID_B, ID_A, ForkKind::Here, "/w/a");
        let second = made(ID_A, ID_B, ForkKind::Here, "/w/b");
        let chain = SourceChain {
            links: vec![&first, &second],
            looped_at: Some(ID_A.to_string()),
        };
        assert_eq!(
            format_log_report(ID_A, &[], &chain),
            format!(
                "Sources of {ID_A} in the fork log:\n\
                 \x20 {ID_A} is a fork of {ID_B}, made 2026-10-28T01:51:22Z (here) in /w/a\n\
                 \x20 {ID_B} is a fork of {ID_A}, made 2026-10-28T01:51:22Z (here) in /w/b\n\
                 \x20 The chain stops: the fork log names {ID_A} again.\n"
            )
        );
    }
}
