//! Black-box coverage for `krt replay`, driving the real binary.
//!
//! A replay reads a recorded file and prints one summary line for one run. The
//! tests read the line that the binary printed, so they cover the whole path
//! from the command line to standard output.
//!
//! The committed fixture holds two runs. It covers the default selection of the
//! last run, and the selection of the other run by `--run`. Every other file is
//! built as text in the test that needs it, and it goes away when that test
//! ends. No test touches the network.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap here is an assertion about the harness, not an unhandled error: the spawn of the freshly built binary, the decode of the output that binary wrote, and the write of a file under the temporary directory. A failure of any one is a broken harness, and a panic names it at once"
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The name that starts every message that `krt` writes to standard error.
const PROGRAM: &str = "krt";

/// The name of the command that folds a recorded file.
const REPLAY: &str = "replay";

/// The flag that picks which run of the file to fold.
const RUN_FLAG: &str = "--run";

/// The exit code of a failure.
const EXIT_FAILURE: i32 = 1;

/// The recorded file the repository holds, which carries two runs.
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/two-runs.jsonl");

/// The identifier of the first run of the fixture.
const FIRST_RUN: &str = "2026-08-18T12:00:00.123Z";

/// The identifier of the second run of the fixture, which is the last run.
const SECOND_RUN: &str = "2026-08-19T09:30:00.000Z";

/// An identifier that the fixture does not hold.
const ABSENT_RUN: &str = "1999-01-01T00:00:00.000Z";

/// The summary of the first run of the fixture.
///
/// The run makes two rounds. The first round answers at TTL 1 and TTL 3, and
/// the second round answers at TTL 1, so two TTLs answered in all.
const FIRST_RUN_SUMMARY: &str =
    "2026-08-18T12:00:00.123Z  example.com (93.184.216.34)  2 rounds  2 hops  reached\n";

/// The summary of the second run of the fixture.
const SECOND_RUN_SUMMARY: &str =
    "2026-08-19T09:30:00.000Z  example.org (93.184.216.35)  1 round  2 hops  reached\n";

/// The `run` line of every file that a test builds.
const BUILT_RUN_LINE: &str = r#"{"type":"run","run":"2026-08-20T00:00:00.000Z","krt":"0.1.0 (abc1234, clean)","source":{"addr":"1.2.3.4","kind":"public"},"target":{"arg":"example.net","addr":"198.51.100.7","family":"ipv4"},"config":{"interval_ms":1000,"protocol":"icmp","first_ttl":1,"max_ttl":30,"multipath":"classic","privilege":"unprivileged","dns":true},"host":"tims-mac"}"#;

/// The `round` line of every file that a test builds.
///
/// Two TTLs answered, and the round reached the target.
const BUILT_ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":0.5,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":9.5,"icmp":"echo_reply"}]}"#;

/// The summary of a built file that holds the `run` record.
const BUILT_SUMMARY: &str =
    "2026-08-20T00:00:00.000Z  example.net (198.51.100.7)  1 round  2 hops  reached\n";

/// The summary of a built file that holds no `run` record.
const BUILT_SUMMARY_WITHOUT_A_TARGET: &str =
    "2026-08-20T00:00:00.000Z  unknown  1 round  2 hops  reached\n";

/// The `round` line of a run that did not reach the target.
///
/// One TTL answered, so the summary of this round holds the singular `1 hop`.
const BUILT_MISSED_ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":false,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":0.5,"icmp":"time_exceeded"}]}"#;

/// The summary of a built file whose run did not reach the target.
const BUILT_SUMMARY_NEVER_REACHED: &str =
    "2026-08-20T00:00:00.000Z  example.net (198.51.100.7)  1 round  1 hop  never reached\n";

/// The start of a `round` line that a `kill -9` cut short.
const CUT_CHUNK: &str = r#"{"type":"round""#;

/// What the warning of a cut final line says about the cut.
const CUT_SHORT: &str = "is cut short at";

/// A line that is not JSON.
const NOT_JSON_LINE: &str = "this is not json";

/// The line that a message names for the corrupt line of a built file.
const CORRUPT_LINE: &str = "line 2";

/// The reason that a message carries when the file holds no run to fold.
const NO_RUN: &str = "the file holds no run";

/// Invoke the freshly built `krt` binary.
fn krt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_krt"))
}

/// What one run of the binary wrote, and how it exited.
struct Output {
    /// True when the binary exited with success.
    success: bool,
    /// The exit code, when the binary exited on its own.
    code: Option<i32>,
    /// The text the binary wrote to standard output.
    stdout: String,
    /// The text the binary wrote to standard error.
    stderr: String,
}

/// Runs `krt` with the arguments and reads what it wrote.
fn run(arguments: &[&str]) -> Output {
    let output = krt().args(arguments).output().unwrap();
    Output {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Asserts that the arguments make `krt` exit with success.
fn success(arguments: &[&str]) -> Output {
    let result = run(arguments);
    assert!(
        result.success,
        "`krt {}` must exit with success, but it exited with {:?}; stderr: {}",
        arguments.join(" "),
        result.code,
        result.stderr
    );
    result
}

/// Asserts that the arguments make `krt` fail, and reads the message.
///
/// A failure writes nothing to standard output and exits with code one.
fn failure(arguments: &[&str]) -> String {
    let result = run(arguments);
    let line = arguments.join(" ");
    assert!(
        !result.success,
        "`krt {line}` must fail, but it succeeded; stdout: {}",
        result.stdout
    );
    assert_eq!(
        result.code,
        Some(EXIT_FAILURE),
        "`krt {line}` must exit with code {EXIT_FAILURE}"
    );
    assert_eq!(
        result.stdout, "",
        "`krt {line}` must write nothing to standard output"
    );
    assert!(
        !result.stderr.is_empty(),
        "`krt {line}` must write the reason to standard error"
    );
    result.stderr
}

/// Builds a path under the temporary directory that no other run reaches.
///
/// Two runs of one test can overlap, because `cargo test` runs on many threads
/// and more than one `cargo test` can run at once. The process identifier and
/// the nanosecond keep the two runs apart.
fn temp_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must stand after the epoch")
        .as_nanos();
    let process = std::process::id();
    std::env::temp_dir().join(format!("krt-replay-{label}-{process}-{nanos}.jsonl"))
}

/// A file that one test makes. The file goes away when the test ends, and also
/// when the test panics.
struct TempFile {
    /// The path of the file.
    path: PathBuf,
}

impl TempFile {
    /// Writes the text to a new file that no other run reaches.
    fn new(label: &str, contents: &str) -> Self {
        let path = temp_path(label);
        fs::write(&path, contents).expect("the test file must be written");
        Self { path }
    }

    /// The path of the file, as a command line carries it.
    fn arg(&self) -> String {
        self.path.display().to_string()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Joins the lines of a file. Every line ends with a newline.
fn file_of(lines: &[&str]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}

#[test]
fn a_replay_prints_the_summary_of_the_last_run() {
    let result = success(&[REPLAY, FIXTURE]);
    assert_eq!(result.stdout, SECOND_RUN_SUMMARY);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );
}

#[test]
fn a_named_run_prints_the_summary_of_that_run() {
    let result = success(&[REPLAY, FIXTURE, RUN_FLAG, FIRST_RUN]);
    assert_eq!(result.stdout, FIRST_RUN_SUMMARY);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );
}

#[test]
fn a_run_that_the_file_does_not_hold_fails_and_names_every_run_of_the_file() {
    let stderr = failure(&[REPLAY, FIXTURE, RUN_FLAG, ABSENT_RUN]);
    for id in [ABSENT_RUN, FIRST_RUN, SECOND_RUN] {
        assert!(stderr.contains(id), "the message names `{id}`: {stderr}");
    }
}

#[test]
fn a_file_that_is_absent_fails_and_names_the_path() {
    let path = temp_path("absent").display().to_string();
    let stderr = failure(&[REPLAY, path.as_str()]);
    assert!(
        stderr.contains(path.as_str()),
        "the message names the path: {stderr}"
    );
}

#[test]
fn a_file_that_holds_no_run_fails_and_names_the_path() {
    let file = TempFile::new("empty", "");
    let path = file.arg();
    let stderr = failure(&[REPLAY, path.as_str()]);
    assert!(
        stderr.contains(path.as_str()),
        "the message names the path: {stderr}"
    );
    assert!(
        stderr.contains(NO_RUN),
        "the message says the file holds no run: {stderr}"
    );
}

/// A file that holds no run lists no run, even when `--run` named one.
///
/// The message of an absent run names every run that the file holds, so the
/// user reads one line and corrects the flag. A file that holds no run has
/// nothing to name, and a message that promises a list and then holds none
/// reads as a defect of the tool. The message stops at the reason.
#[test]
fn a_file_that_holds_no_run_lists_no_run_for_the_run_flag() {
    let file = TempFile::new("empty-with-a-run", "");
    let path = file.arg();
    let stderr = failure(&[REPLAY, path.as_str(), RUN_FLAG, ABSENT_RUN]);
    assert_eq!(stderr, format!("{PROGRAM}: {path}: {NO_RUN}\n"));
}

#[test]
fn a_final_line_that_is_cut_short_warns_and_still_prints_the_summary() {
    let mut text = file_of(&[BUILT_RUN_LINE, BUILT_ROUND_LINE]);
    text.push_str(CUT_CHUNK);
    let file = TempFile::new("cut", &text);
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(result.stdout, BUILT_SUMMARY);
    assert!(
        result.stderr.contains(path.as_str()),
        "the warning names the path: {}",
        result.stderr
    );
    assert_eq!(
        result.stderr.lines().count(),
        1,
        "a cut file raises one warning: {}",
        result.stderr
    );
}

#[test]
fn a_file_without_a_run_record_prints_no_target() {
    let file = TempFile::new("no-run-record", &file_of(&[BUILT_ROUND_LINE]));
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(result.stdout, BUILT_SUMMARY_WITHOUT_A_TARGET);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );
}

/// A run that no round reached says so, and one TTL takes the singular word.
#[test]
fn a_run_that_did_not_reach_the_target_says_so() {
    let text = file_of(&[BUILT_RUN_LINE, BUILT_MISSED_ROUND_LINE]);
    let file = TempFile::new("missed", &text);
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(result.stdout, BUILT_SUMMARY_NEVER_REACHED);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );
}

#[test]
fn a_line_that_is_not_json_fails_and_names_the_line() {
    let text = file_of(&[BUILT_RUN_LINE, NOT_JSON_LINE, BUILT_ROUND_LINE]);
    let file = TempFile::new("corrupt", &text);
    let path = file.arg();
    let stderr = failure(&[REPLAY, path.as_str()]);
    assert!(
        stderr.contains(CORRUPT_LINE),
        "the message names the line: {stderr}"
    );
    assert!(
        stderr.contains(path.as_str()),
        "the message names the path: {stderr}"
    );
}

/// A cut that swallowed every record still names the cut.
///
/// A `kill -9` during the first record of a file leaves a file that holds no
/// complete record. Without the warning, such a file reads exactly like an
/// empty file, and the user cannot tell the one from the other.
#[test]
fn a_cut_that_swallowed_every_record_names_the_cut_and_the_missing_run() {
    let file = TempFile::new("cut-away", CUT_CHUNK);
    let path = file.arg();
    let stderr = failure(&[REPLAY, path.as_str()]);
    assert!(
        stderr.contains(CUT_SHORT),
        "the message names the cut: {stderr}"
    );
    assert!(
        stderr.contains(NO_RUN),
        "the message says the file holds no run: {stderr}"
    );
    assert!(
        stderr.contains(path.as_str()),
        "the message names the path: {stderr}"
    );
}
