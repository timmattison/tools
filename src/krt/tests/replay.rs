//! Black-box coverage for `krt replay`, driving the real binary.
//!
//! A replay reads a recorded file, prints one summary line for one run, and
//! prints the aggregate numbers of that run under it. The tests read the lines
//! that the binary printed, so they cover the whole path from the command line
//! to standard output.
//!
//! Every number of an expected constant is computed by hand, and the
//! arithmetic stands beside the constant. A constant that copies what the
//! binary printed proves nothing.
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

/// What a replay prints for the first run of the fixture.
///
/// The run makes two rounds. Both rounds carry the TTL range 1 to 3, so every
/// one of those three TTLs took two probes. The first round answers at TTL 1
/// from 192.168.1.1 at 1.23, and at TTL 3 from 93.184.216.34 at 24.1. The
/// second round answers at TTL 1 from 192.168.1.1 at 1.41. Two TTLs answered
/// in all.
///
/// TTL 1 answered both probes, so it loses nothing. The two answers are 1.23
/// and 1.41. The sum is 2.64, so the mean is 2.64 / 2 = 1.32, which prints as
/// 1.3. The distances from the mean are -0.09 and 0.09, and the squares of
/// them sum to 0.0162. The population variance is 0.0162 / 2 = 0.0081, so the
/// standard deviation is 0.09, which prints as 0.1. The jitter is the absolute
/// difference of the last two answers, 1.41 - 1.23 = 0.18, which prints as
/// 0.2.
///
/// TTL 2 answered no probe of the two, so its loss is 2 / 2 * 100 = 100.0
/// percent, it names no host, and it holds no number.
///
/// TTL 3 answered one probe of the two, so its loss is 1 / 2 * 100 = 50.0
/// percent. One answer is its own smallest, mean, and largest time, and the
/// population standard deviation of one sample is 0.0. One sample names no
/// last two answers, so it holds no jitter.
const FIRST_RUN_OUTPUT: &str = "\
2026-08-18T12:00:00.123Z  example.com (93.184.216.34)  2 rounds  2 hops  reached
  1  192.168.1.1  loss 0.0%  sent 2  recv 2  last 1.4  min 1.2  avg 1.3  max 1.4  stddev 0.1  jitter 0.2
  2  ???  loss 100.0%  sent 2  recv 0  last -  min -  avg -  max -  stddev -  jitter -
  3  93.184.216.34  loss 50.0%  sent 2  recv 1  last 24.1  min 24.1  avg 24.1  max 24.1  stddev 0.0  jitter -
";

/// What a replay prints for the second run of the fixture.
///
/// The run makes one round of the TTL range 1 to 2, and it answers at both
/// TTLs. TTL 1 answers from 10.0.0.1 at 0.87, which prints as 0.9, and TTL 2
/// answers from 93.184.216.35 at 12.5. Each TTL took one probe and answered
/// it, so each one loses nothing. One answer is its own smallest, mean, and
/// largest time, the population standard deviation of one sample is 0.0, and
/// one sample holds no jitter.
const SECOND_RUN_OUTPUT: &str = "\
2026-08-19T09:30:00.000Z  example.org (93.184.216.35)  1 round  2 hops  reached
  1  10.0.0.1  loss 0.0%  sent 1  recv 1  last 0.9  min 0.9  avg 0.9  max 0.9  stddev 0.0  jitter -
  2  93.184.216.35  loss 0.0%  sent 1  recv 1  last 12.5  min 12.5  avg 12.5  max 12.5  stddev 0.0  jitter -
";

/// The `run` line of every file that a test builds.
const BUILT_RUN_LINE: &str = r#"{"type":"run","run":"2026-08-20T00:00:00.000Z","krt":"0.1.0 (abc1234, clean)","source":{"addr":"1.2.3.4","kind":"public"},"target":{"arg":"example.net","addr":"198.51.100.7","family":"ipv4"},"config":{"interval_ms":1000,"protocol":"icmp","first_ttl":1,"max_ttl":30,"multipath":"classic","privilege":"unprivileged","dns":true},"host":"tims-mac"}"#;

/// The `round` line of every file that a test builds.
///
/// Two TTLs answered, and the round reached the target.
const BUILT_ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":0.5,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":9.5,"icmp":"echo_reply"}]}"#;

/// The aggregate of a built file that holds the one round.
///
/// The round carries the TTL range 1 to 2, so each of the two TTLs took one
/// probe. Both TTLs answered, so neither one loses anything. One answer is its
/// own smallest, mean, and largest time, the population standard deviation of
/// one sample is 0.0, and one sample names no last two answers, so it holds no
/// jitter.
// A backslash at the end of a line of a string literal takes the newline and
// every space that follows it, so a text whose first line is indented starts on
// the line of the opening quotation mark.
const BUILT_AGGREGATE: &str = "  1  10.0.0.1  loss 0.0%  sent 1  recv 1  last 0.5  min 0.5  avg 0.5  max 0.5  stddev 0.0  jitter -
  2  198.51.100.7  loss 0.0%  sent 1  recv 1  last 9.5  min 9.5  avg 9.5  max 9.5  stddev 0.0  jitter -
";

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

/// What a replay prints for a built file whose run did not reach the target.
///
/// The round carries the TTL range 1 to 2, so each of the two TTLs took one
/// probe. TTL 1 answered its probe, so it loses nothing. TTL 2 answered no
/// probe of the one, so its loss is 1 / 1 * 100 = 100.0 percent, it names no
/// host, and it holds no number.
const BUILT_OUTPUT_NEVER_REACHED: &str = "\
2026-08-20T00:00:00.000Z  example.net (198.51.100.7)  1 round  1 hop  never reached
  1  10.0.0.1  loss 0.0%  sent 1  recv 1  last 0.5  min 0.5  avg 0.5  max 0.5  stddev 0.0  jitter -
  2  ???  loss 100.0%  sent 1  recv 0  last -  min -  avg -  max -  stddev -  jitter -
";

/// The `round` lines of a run whose TTL 1 two routers answer at.
///
/// The first router answers round one at 1.0. The second router answers round
/// two at 2.0 and round three at 3.0. TTL 2 answers every round at 10.0.
const BUILT_SPLIT_ROUND_LINES: [&str; 3] = [
    r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":1.0,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":10.0,"icmp":"echo_reply"}]}"#,
    r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":2,"ts":"2026-08-20T00:00:02.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.2","rtt_ms":2.0,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":10.0,"icmp":"echo_reply"}]}"#,
    r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":3,"ts":"2026-08-20T00:00:03.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.2","rtt_ms":3.0,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":10.0,"icmp":"echo_reply"}]}"#,
];

/// What a replay prints for the file of the split TTL.
///
/// Every one of the three rounds carries the TTL range 1 to 2, so each of the
/// two TTLs took three probes, and every probe answered. Neither TTL loses
/// anything.
///
/// TTL 1 saw two addresses, so its host field names the first one and the
/// count of the other one. The three answers of the TTL are 1.0, 2.0, and 3.0.
/// The sum is 6.0, so the mean is 6 / 3 = 2.0. The distances from the mean are
/// -1.0, 0.0, and 1.0, and the squares of them sum to 2.0. The population
/// variance is 2 / 3 = 0.667, so the standard deviation is 0.816, which prints
/// as 0.8. The jitter is 3.0 - 2.0 = 1.0.
///
/// The first router answered one of the three answers of TTL 1, so its share
/// is 1 / 3 * 100 = 33.3 percent. The second router answered the other two, so
/// its share is 2 / 3 * 100 = 66.7 percent. The two answers of the second
/// router are 2.0 and 3.0. The mean of them is 5 / 2 = 2.5, the distances are
/// -0.5 and 0.5, and the squares sum to 0.5. The variance is 0.5 / 2 = 0.25,
/// so the standard deviation is 0.5.
///
/// TTL 2 saw one address, so it takes no address line. Its three answers are
/// all 10.0, so the mean is 10.0 and the standard deviation is 0.0. The jitter
/// is 10.0 - 10.0 = 0.0.
const BUILT_SPLIT_OUTPUT: &str = "\
2026-08-20T00:00:00.000Z  example.net (198.51.100.7)  3 rounds  2 hops  reached
  1  10.0.0.1 (+1)  loss 0.0%  sent 3  recv 3  last 3.0  min 1.0  avg 2.0  max 3.0  stddev 0.8  jitter 1.0
      10.0.0.1  share 33.3%  recv 1  last 1.0  min 1.0  avg 1.0  max 1.0  stddev 0.0  jitter -
      10.0.0.2  share 66.7%  recv 2  last 3.0  min 2.0  avg 2.5  max 3.0  stddev 0.5  jitter 1.0
  2  198.51.100.7  loss 0.0%  sent 3  recv 3  last 10.0  min 10.0  avg 10.0  max 10.0  stddev 0.0  jitter 0.0
";

/// The text between two fields of a line that a replay prints.
const FIELD_SEPARATOR: &str = "  ";

/// The name of the field that holds the share of one address.
const SHARE: &str = "share";

/// The sign that ends a percentage.
const PERCENT_SIGN: &str = "%";

/// The whole of a percentage, which the shares of one TTL sum to.
const WHOLE_PERCENT: f64 = 100.0;

/// The largest difference that a comparison of two percentages admits.
///
/// The two shares of this test are 33.3 and 66.7, which sum to exactly 100
/// in decimal. A read of each printed decimal into a number with a
/// fraction loses a little. The sum then misses the whole by about 1e-14,
/// and this tolerance covers that loss. A pair of shares that rounds the
/// other way misses the whole by a tenth, and this test names no such
/// pair.
const PERCENT_TOLERANCE: f64 = 1e-9;

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

/// Reads the share that one address line printed.
fn share_of(line: &str) -> f64 {
    let prefix = format!("{SHARE} ");
    let field = line
        .split(FIELD_SEPARATOR)
        .find_map(|field| field.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("an address line holds a share field: {line}"));
    field
        .strip_suffix(PERCENT_SIGN)
        .unwrap_or_else(|| panic!("a share ends with the percent sign: {line}"))
        .parse()
        .unwrap_or_else(|_| panic!("a share reads as a number: {line}"))
}

#[test]
fn a_replay_prints_the_summary_and_the_aggregate_of_the_last_run() {
    let result = success(&[REPLAY, FIXTURE]);
    assert_eq!(result.stdout, SECOND_RUN_OUTPUT);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );
}

#[test]
fn a_named_run_prints_the_summary_and_the_aggregate_of_that_run() {
    let result = success(&[REPLAY, FIXTURE, RUN_FLAG, FIRST_RUN]);
    assert_eq!(result.stdout, FIRST_RUN_OUTPUT);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );
}

/// A TTL that two routers answer at names both of them, and the two shares of
/// that TTL sum to the whole.
///
/// A share per address is the measure that a TTL of two routers needs. A loss
/// per address would report 50 percent for a pair that splits the traffic
/// evenly and loses nothing.
#[test]
fn a_ttl_that_two_routers_answer_at_prints_one_line_for_each_of_them() {
    let mut lines = vec![BUILT_RUN_LINE];
    lines.extend(BUILT_SPLIT_ROUND_LINES);
    let file = TempFile::new("split", &file_of(&lines));
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(result.stdout, BUILT_SPLIT_OUTPUT);
    assert_eq!(
        result.stderr, "",
        "a whole file writes nothing to standard error"
    );

    let printed: Vec<&str> = result.stdout.lines().collect();
    let total = share_of(printed[2]) + share_of(printed[3]);
    assert!(
        (total - WHOLE_PERCENT).abs() < PERCENT_TOLERANCE,
        "the printed shares of one TTL sum to {WHOLE_PERCENT}, and they sum to {total}"
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
    assert_eq!(result.stdout, format!("{BUILT_SUMMARY}{BUILT_AGGREGATE}"));
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
    assert_eq!(
        result.stdout,
        format!("{BUILT_SUMMARY_WITHOUT_A_TARGET}{BUILT_AGGREGATE}")
    );
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
    assert_eq!(result.stdout, BUILT_OUTPUT_NEVER_REACHED);
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
