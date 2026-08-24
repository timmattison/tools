//! Black-box coverage for `krt replay`, driving the real binary.
//!
//! A replay reads a recorded file and folds one run of it into one frame: the
//! header line that names the run, one blank line, the column header, and one
//! row for each TTL of the path. The tests read the lines that the binary
//! printed, so they cover the whole path from the command line to standard
//! output.
//!
//! The binary writes to a pipe here and never to a terminal, so every frame
//! below draws at the nominal width of 97 columns. That rule is what makes
//! these expectations stable: a test that read the width of the terminal of
//! whoever started `cargo test` would pass on one machine and fail on the next.
//!
//! Every number of an expected constant is computed by hand, and the
//! arithmetic stands beside the constant. A constant that copies what the
//! binary printed proves nothing.
//!
//! The committed fixture holds two runs. It covers the default selection of the
//! last run, and the selection of the other run by `--run`. Every other file is
//! built as text in the test that needs it, and it goes away when that test
//! ends. A built file carries a process identifier and a nanosecond in its
//! name, so its name and its size change between two runs of one test: such a
//! test reads both of them off the file it wrote, and it computes the size from
//! the text it wrote and never from the number the binary printed. No test
//! touches the network.

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

/// The exit code of a success.
const EXIT_SUCCESS: i32 = 0;

/// The exit code of a failure.
const EXIT_FAILURE: i32 = 1;

/// The recorded file the repository holds, which carries two runs.
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/two-runs.jsonl");

/// The name of that file, without its directory.
const FIXTURE_NAME: &str = "two-runs.jsonl";

/// The size of that file, in bytes.
///
/// The file is committed, so the number stands. The test below reads it off the
/// disk and compares, so a change to the fixture fails there and names the new
/// size, and not inside a frame that misses by one character.
const FIXTURE_BYTES: u64 = 1899;

/// The size that the header line prints for the fixture.
///
/// 1899 / 1024 is 1.8545, which one decimal place writes as `1.9`.
const FIXTURE_SIZE: &str = "1.9 KB";

/// The identifier of the first run of the fixture.
const FIRST_RUN: &str = "2026-08-18T12:00:00.123Z";

/// The identifier of the second run of the fixture, which is the last run.
const SECOND_RUN: &str = "2026-08-19T09:30:00.000Z";

/// The number of runs that the fixture holds.
const FIXTURE_RUNS: usize = 2;

/// An identifier that the fixture does not hold.
const ABSENT_RUN: &str = "1999-01-01T00:00:00.000Z";

/// The column header of every frame.
///
/// The test spells it, and the render draws it from the list of columns it
/// holds. The two are on purpose: a heading that moved would otherwise agree
/// with itself, and these headings are what a reader of the table sees.
const COLUMN_HEADER: &str =
    " TTL  Host                             Loss%   Sent   Last    Min    Avg    Max  StDev  Recent";

/// The header line of the frame of the last run of the fixture.
///
/// The `run` record of that run names `example.org`, which resolved to
/// `93.184.216.35`, and the source `2001:db8::1`. Its interval is 500
/// milliseconds, and it recorded one round.
const FIXTURE_LAST_HEADER: &str =
    " krt  example.org → 93.184.216.35   src 2001:db8::1   round 1   500ms   two-runs.jsonl (1.9 KB)";

/// The rows of the frame of the last run of the fixture.
///
/// The run makes one round of the TTL range 1 to 2, so each of the two TTLs
/// took one probe, and each of them answered it. Neither TTL loses anything.
///
/// TTL 1 answers from `10.0.0.1` at 0.87, which one decimal place writes as
/// 0.9. One answer is its own last, smallest, mean, and largest time, and the
/// population standard deviation of one sample is 0.0. One sample draws one bar
/// of the sparkline, and a window of one sample varies by nothing, so the bar
/// is the lowest one.
///
/// TTL 2 answers from `93.184.216.35` at 12.5, which is the address that the
/// destination resolved to, so that row carries the star. No `name` record
/// names either address of this run, so each host reads as its address alone.
const FIXTURE_LAST_ROWS: [&str; 2] = [
    "   1  10.0.0.1                          0.0%      1    0.9    0.9    0.9    0.9    0.0  ▁",
    "   2  93.184.216.35 ★                   0.0%      1   12.5   12.5   12.5   12.5    0.0  ▁",
];

/// The header line of the frame of the first run of the fixture.
///
/// The `run` record of that run names `example.com`, which resolved to
/// `93.184.216.34`, and the source `1.2.3.4`. Its interval is 1000
/// milliseconds, which reads `1s`, and it recorded two rounds.
const FIXTURE_FIRST_HEADER: &str =
    " krt  example.com → 93.184.216.34   src 1.2.3.4   round 2   1s   two-runs.jsonl (1.9 KB)";

/// The rows of the frame of the first run of the fixture.
///
/// The run makes two rounds. Both rounds carry the TTL range 1 to 3, so every
/// one of those three TTLs took two probes. The first round answers at TTL 1
/// from 192.168.1.1 at 1.23, and at TTL 3 from 93.184.216.34 at 24.1. The
/// second round answers at TTL 1 from 192.168.1.1 at 1.41.
///
/// TTL 1 answered both probes, so it loses nothing. The two answers are 1.23
/// and 1.41, which print as 1.2 and 1.4. The sum is 2.64, so the mean is
/// 2.64 / 2 = 1.32, which prints as 1.3. The distances from the mean are -0.09
/// and 0.09, and the squares of them sum to 0.0162. The population variance is
/// 0.0162 / 2 = 0.0081, so the standard deviation is 0.09, which prints as 0.1.
/// The window of the sparkline runs from 1.23 to 1.41, so the first sample
/// takes the lowest bar and the second takes the highest. A `name` record of
/// this run names 192.168.1.1 `router.lan`, so the host of the row carries the
/// name and the address together.
///
/// TTL 2 answered no probe of the two, so its loss is 2 / 2 * 100 = 100.0
/// percent, it names no host, it holds no number, and it draws no bar.
///
/// TTL 3 answered one probe of the two, so its loss is 1 / 2 * 100 = 50.0
/// percent. One answer is its own last, smallest, mean, and largest time, and
/// the population standard deviation of one sample is 0.0. The address is the
/// one that the destination resolved to, so the row carries the star.
const FIXTURE_FIRST_ROWS: [&str; 3] = [
    "   1  router.lan (192.168.1.1)          0.0%      2    1.4    1.2    1.3    1.4    0.1  ▁▇",
    "   2  ???                             100.0%      2      -      -      -      -      -",
    "   3  93.184.216.34 ★                  50.0%      2   24.1   24.1   24.1   24.1    0.0  ▁",
];

/// The `run` line of every file that a test builds.
const BUILT_RUN_LINE: &str = r#"{"type":"run","run":"2026-08-20T00:00:00.000Z","krt":"0.1.0 (abc1234, clean)","source":{"addr":"1.2.3.4","kind":"public"},"target":{"arg":"example.net","addr":"198.51.100.7","family":"ipv4"},"config":{"interval_ms":1000,"protocol":"icmp","first_ttl":1,"max_ttl":30,"multipath":"classic","privilege":"unprivileged","dns":true},"host":"tims-mac"}"#;

/// The `round` line of every file that a test builds.
///
/// Two TTLs answered, and the round reached the target.
const BUILT_ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":0.5,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":9.5,"icmp":"echo_reply"}]}"#;

/// A `name` line that names the first router of the built round.
///
/// No run of `krt` writes such a record yet. A file that a later build recorded
/// holds them, and the render already reads them, so this line covers the
/// reader today.
const BUILT_NAME_LINE: &str = r#"{"type":"name","run":"2026-08-20T00:00:00.000Z","ts":"2026-08-20T00:00:02.000Z","addr":"10.0.0.1","host":"router.lan"}"#;

/// The rows of the frame of a built file that holds the one round.
///
/// The round carries the TTL range 1 to 2, so each of the two TTLs took one
/// probe. Both TTLs answered, so neither one loses anything. One answer is its
/// own last, smallest, mean, and largest time, the population standard
/// deviation of one sample is 0.0, and a window of one sample draws the lowest
/// bar. The address of TTL 2 is the one that the destination resolved to, so
/// that row carries the star.
const BUILT_ROWS: [&str; 2] = [
    "   1  10.0.0.1                          0.0%      1    0.5    0.5    0.5    0.5    0.0  ▁",
    "   2  198.51.100.7 ★                    0.0%      1    9.5    9.5    9.5    9.5    0.0  ▁",
];

/// The rows of the frame of that same round, with the `name` record beside it.
///
/// Every number is the number of [`BUILT_ROWS`]. The one change is the host of
/// TTL 1, which now names the router and its address together.
const BUILT_NAMED_ROWS: [&str; 2] = [
    "   1  router.lan (10.0.0.1)             0.0%      1    0.5    0.5    0.5    0.5    0.0  ▁",
    "   2  198.51.100.7 ★                    0.0%      1    9.5    9.5    9.5    9.5    0.0  ▁",
];

/// The rows of the frame of that same round, in a file that holds no `run`
/// record.
///
/// Every number is the number of [`BUILT_ROWS`]. A file that names no
/// destination resolves no address, so no row of it carries the star.
const BUILT_ROWS_WITHOUT_A_TARGET: [&str; 2] = [
    "   1  10.0.0.1                          0.0%      1    0.5    0.5    0.5    0.5    0.0  ▁",
    "   2  198.51.100.7                      0.0%      1    9.5    9.5    9.5    9.5    0.0  ▁",
];

/// The `round` line of a run that did not reach the target.
const BUILT_MISSED_ROUND_LINE: &str = r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":false,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":0.5,"icmp":"time_exceeded"}]}"#;

/// The rows of the frame of a run that did not reach the target.
///
/// The round carries the TTL range 1 to 2, so each of the two TTLs took one
/// probe. TTL 1 answered its probe at 0.5, so it loses nothing. TTL 2 answered
/// no probe of the one, so its loss is 1 / 1 * 100 = 100.0 percent, it names no
/// host, it holds no number, and it draws no bar. No row answered from the
/// address that the destination resolved to, so no row carries the star.
const BUILT_MISSED_ROWS: [&str; 2] = [
    "   1  10.0.0.1                          0.0%      1    0.5    0.5    0.5    0.5    0.0  ▁",
    "   2  ???                             100.0%      1      -      -      -      -      -",
];

/// The `round` lines of a run whose TTL 1 two routers answer at.
///
/// The first router answers round one at 1.0. The second router answers round
/// two at 2.0 and round three at 3.0. TTL 2 answers every round at 10.0.
const BUILT_SPLIT_ROUND_LINES: [&str; 3] = [
    r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":1,"ts":"2026-08-20T00:00:01.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.1","rtt_ms":1.0,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":10.0,"icmp":"echo_reply"}]}"#,
    r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":2,"ts":"2026-08-20T00:00:02.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.2","rtt_ms":2.0,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":10.0,"icmp":"echo_reply"}]}"#,
    r#"{"type":"round","run":"2026-08-20T00:00:00.000Z","seq":3,"ts":"2026-08-20T00:00:03.000Z","dur_ms":1000,"ttl_range":[1,2],"reached":true,"hops":[{"ttl":1,"addr":"10.0.0.2","rtt_ms":3.0,"icmp":"time_exceeded"},{"ttl":2,"addr":"198.51.100.7","rtt_ms":10.0,"icmp":"echo_reply"}]}"#,
];

/// The rows of the frame of the file of the split TTL.
///
/// Every one of the three rounds carries the TTL range 1 to 2, so each of the
/// two TTLs took three probes, and every probe answered. Neither TTL loses
/// anything.
///
/// TTL 1 saw two routers, so its host names the first one and counts the other
/// one, and one address row stands under it for each of them. The three answers
/// of the TTL are 1.0, 2.0, and 3.0. The sum is 6.0, so the mean is 6 / 3 = 2.0.
/// The distances from the mean are -1.0, 0.0, and 1.0, and the squares of them
/// sum to 2.0. The population variance is 2 / 3 = 0.667, so the standard
/// deviation is 0.816, which prints as 0.8. The window of the sparkline runs
/// from 1.0 to 3.0, so 1.0 takes the lowest bar, 3.0 takes the highest, and 2.0
/// stands at half of the span, which is the fourth bar of the seven.
///
/// The first router answered one of the three answers of TTL 1, so its share is
/// 1 / 3 * 100 = 33.3 percent. The second router answered the other two, so its
/// share is 2 / 3 * 100 = 66.7 percent, and the two shares sum to the whole. The
/// two answers of the second router are 2.0 and 3.0. The mean of them is
/// 5 / 2 = 2.5, the distances are -0.5 and 0.5, and the squares sum to 0.5. The
/// variance is 0.5 / 2 = 0.25, so the standard deviation is 0.5.
///
/// TTL 2 saw one router, so it takes no address row. Its three answers are all
/// 10.0, so the mean is 10.0 and the standard deviation is 0.0. Every sample of
/// its window is equal, so every bar of it is the lowest one. That address is
/// the one the destination resolved to, so the row carries the star.
const BUILT_SPLIT_ROWS: [&str; 4] = [
    "   1  10.0.0.1 (+1)                     0.0%      3    3.0    1.0    2.0    3.0    0.8  ▁▄▇",
    "      ├ 10.0.0.1                       33.3%▹     1    1.0    1.0    1.0    1.0    0.0  ▁",
    "      └ 10.0.0.2                       66.7%▹     2    3.0    2.0    2.5    3.0    0.5  ▁▇",
    "   2  198.51.100.7 ★                    0.0%      3   10.0   10.0   10.0   10.0    0.0  ▁▁▁",
];

/// The mark of the row that answered from the destination.
const DESTINATION_MARK: char = '★';

/// The mark that tells a share of one router from the loss of a TTL.
const SHARE_MARK: char = '▹';

/// The glyph that starts the host of an address row that another one follows.
const BRANCH: char = '├';

/// The glyph that starts the host of the last address row of a TTL.
const LAST_BRANCH: char = '└';

/// The text that a TTL row carries when one more router answered at that TTL.
const ONE_MORE_ROUTER: &str = "(+1)";

/// The sign that ends a percentage.
const PERCENT_SIGN: char = '%';

/// The whole of a percentage, which the shares of one TTL sum to while that
/// TTL tracks every address that answered at it.
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

/// The number of bytes of one step of the size scale.
///
/// A size below one step reads as whole bytes, and a larger one reads to one
/// decimal place in the unit above. Every file that these tests build stands
/// below the second step, so the two units below cover every one of them.
const BYTES_OF_ONE_STEP: usize = 1024;

/// The unit of a size below one step.
const BYTES_UNIT: &str = "B";

/// The unit of a size one step above the bytes.
const KILOBYTES_UNIT: &str = "KB";

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
    /// The number of bytes that the test wrote to it.
    bytes: usize,
}

impl TempFile {
    /// Writes the text to a new file that no other run reaches.
    fn new(label: &str, contents: &str) -> Self {
        let path = temp_path(label);
        fs::write(&path, contents).expect("the test file must be written");
        Self {
            path,
            bytes: contents.len(),
        }
    }

    /// The path of the file, as a command line carries it.
    fn arg(&self) -> String {
        self.path.display().to_string()
    }

    /// The name of the file, without its directory.
    fn name(&self) -> String {
        self.path
            .file_name()
            .expect("the test file must hold a name")
            .to_string_lossy()
            .into_owned()
    }

    /// The size that the header line prints for the file.
    ///
    /// The count is the length of the text that the test wrote, and never a
    /// number that the binary printed. A size below one step reads as whole
    /// bytes, and a size above it reads to one decimal place in the unit above.
    /// Every file that these tests build stands below the second step, and the
    /// assertion states that bound: a file that grew past it then fails here,
    /// and not inside a frame that misses by one character.
    #[expect(
        clippy::cast_precision_loss,
        reason = "the assertion above holds the count below 1048576, and an `f64` holds every whole number below 2^53"
    )]
    fn size(&self) -> String {
        if self.bytes < BYTES_OF_ONE_STEP {
            return format!("{} {BYTES_UNIT}", self.bytes);
        }
        assert!(
            self.bytes < BYTES_OF_ONE_STEP * BYTES_OF_ONE_STEP,
            "a built file of {} bytes stands above the second step of the size scale, so the header prints a larger unit",
            self.bytes
        );
        let steps = self.bytes as f64 / BYTES_OF_ONE_STEP as f64;
        format!("{steps:.1} {KILOBYTES_UNIT}")
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

/// The whole text of one frame: the header line, one blank line, the column
/// header, and one row for each line of the path.
fn frame(header: &str, rows: &[&str]) -> String {
    let mut text = format!("{header}\n\n{COLUMN_HEADER}\n");
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    text
}

/// The header line of a built file that holds the `run` record.
///
/// The record names `example.net`, which resolved to `198.51.100.7`, and the
/// source `1.2.3.4`. Its interval is 1000 milliseconds, which reads `1s`. The
/// name and the size come off the file that the test wrote, because a built
/// file carries a process identifier and a nanosecond in its name.
fn built_header(file: &TempFile, rounds: usize) -> String {
    format!(
        " krt  example.net → 198.51.100.7   src 1.2.3.4   round {rounds}   1s   {} ({})",
        file.name(),
        file.size()
    )
}

/// The header line of a built file that holds no `run` record.
///
/// Such a file names no destination, no address, no source, and no interval,
/// and the line writes one word in the place of each of them. It still names
/// its rounds and its file.
fn built_header_without_a_target(file: &TempFile, rounds: usize) -> String {
    format!(
        " krt  unknown   src unknown   round {rounds}   unknown   {} ({})",
        file.name(),
        file.size()
    )
}

/// What a replay writes to standard error for a file that holds more than one
/// run.
///
/// The header line of the frame names the target and not the run, so a file of
/// two runs would otherwise leave a reader unable to tell which of the two the
/// frame folded.
fn folded_run_note(path: &str, runs: usize, id: &str) -> String {
    format!("{PROGRAM}: {path}: the file holds {runs} runs. This frame folds the run `{id}`.\n")
}

/// Reads the share that one address row printed.
fn share_of(line: &str) -> f64 {
    let field = line
        .split_whitespace()
        .find_map(|field| field.strip_suffix(SHARE_MARK))
        .unwrap_or_else(|| panic!("an address row holds a share: {line}"));
    field
        .strip_suffix(PERCENT_SIGN)
        .unwrap_or_else(|| panic!("a share ends with the percent sign: {line}"))
        .parse()
        .unwrap_or_else(|_| panic!("a share reads as a number: {line}"))
}

/// The fixture is committed, so the size that the header line names is fixed.
///
/// A change to the fixture fails here and names the new size, where it would
/// otherwise fail inside a frame that misses by one character.
#[test]
fn the_fixture_holds_the_size_that_the_header_line_names() {
    let bytes = fs::metadata(FIXTURE)
        .expect("the fixture must stand beside the tests")
        .len();
    assert_eq!(
        bytes, FIXTURE_BYTES,
        "the header line of the fixture names {FIXTURE_SIZE}, which {FIXTURE_BYTES} bytes writes"
    );
    assert!(
        FIXTURE_LAST_HEADER.ends_with(&format!("{FIXTURE_NAME} ({FIXTURE_SIZE})")),
        "the header line ends with the name of the file and its size"
    );
}

#[test]
fn a_replay_prints_the_frame_of_the_last_run() {
    let result = success(&[REPLAY, FIXTURE]);
    assert_eq!(
        result.stdout,
        frame(FIXTURE_LAST_HEADER, &FIXTURE_LAST_ROWS)
    );
}

#[test]
fn a_replay_exits_with_success() {
    let result = run(&[REPLAY, FIXTURE]);
    assert_eq!(
        result.code,
        Some(EXIT_SUCCESS),
        "a replay that folded a run exits with {EXIT_SUCCESS}; stderr: {}",
        result.stderr
    );
}

#[test]
fn a_named_run_prints_the_frame_of_that_run() {
    let result = success(&[REPLAY, FIXTURE, RUN_FLAG, FIRST_RUN]);
    assert_eq!(
        result.stdout,
        frame(FIXTURE_FIRST_HEADER, &FIXTURE_FIRST_ROWS)
    );
}

/// A file of more than one run names the run that the frame folded.
///
/// The note goes to standard error, where the warning of a cut file already
/// goes, so standard output stays the frame alone and a reader who redirects it
/// gets a table and nothing else.
#[test]
fn a_file_of_more_than_one_run_names_the_folded_run_on_standard_error() {
    let result = success(&[REPLAY, FIXTURE]);
    assert_eq!(
        result.stderr,
        folded_run_note(FIXTURE, FIXTURE_RUNS, SECOND_RUN),
        "the note names the run that the frame folded"
    );
    let named = success(&[REPLAY, FIXTURE, RUN_FLAG, FIRST_RUN]);
    assert_eq!(
        named.stderr,
        folded_run_note(FIXTURE, FIXTURE_RUNS, FIRST_RUN),
        "the note names the run that `--run` picked"
    );
}

/// A file of one run leaves standard error empty.
///
/// One run is the whole file, so a note that named it would say nothing that
/// the command line does not already say.
#[test]
fn a_file_of_one_run_names_no_run_on_standard_error() {
    let file = TempFile::new("one-run", &file_of(&[BUILT_RUN_LINE, BUILT_ROUND_LINE]));
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(result.stdout, frame(&built_header(&file, 1), &BUILT_ROWS));
    assert_eq!(
        result.stderr, "",
        "a whole file of one run writes nothing to standard error"
    );
}

/// A `name` record names the router beside its address.
///
/// No run of `krt` writes such a record yet, and a file that a later build
/// recorded holds them. The render reads them today, so this file covers that
/// reader.
#[test]
fn a_name_record_names_the_router_beside_its_address() {
    let file = TempFile::new(
        "named",
        &file_of(&[BUILT_RUN_LINE, BUILT_NAME_LINE, BUILT_ROUND_LINE]),
    );
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(
        result.stdout,
        frame(&built_header(&file, 1), &BUILT_NAMED_ROWS)
    );
}

/// A TTL that two routers answer at counts the second one, prints one address
/// row for each of them, and the two shares sum to the whole.
///
/// A share per address is the measure that a TTL of two routers needs. A loss
/// per address would report 50 percent for a pair that splits the traffic
/// evenly and loses nothing.
#[test]
fn a_ttl_that_two_routers_answer_at_prints_one_row_for_each_of_them() {
    let mut lines = vec![BUILT_RUN_LINE];
    lines.extend(BUILT_SPLIT_ROUND_LINES);
    let file = TempFile::new("split", &file_of(&lines));
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(
        result.stdout,
        frame(
            &built_header(&file, BUILT_SPLIT_ROUND_LINES.len()),
            &BUILT_SPLIT_ROWS
        )
    );

    let printed: Vec<&str> = result.stdout.lines().collect();
    let ttl_row = printed
        .iter()
        .find(|line| line.contains(ONE_MORE_ROUTER))
        .unwrap_or_else(|| panic!("the TTL row counts the second router: {printed:?}"));
    assert!(
        ttl_row.contains(ONE_MORE_ROUTER),
        "the TTL of two routers counts the one behind the first: {ttl_row}"
    );
    let address_rows: Vec<&&str> = printed
        .iter()
        .filter(|line| line.contains(BRANCH) || line.contains(LAST_BRANCH))
        .collect();
    assert_eq!(
        address_rows.len(),
        2,
        "one address row for each router of the TTL: {printed:?}"
    );
    assert!(
        address_rows[0].contains(BRANCH) && address_rows[1].contains(LAST_BRANCH),
        "the last address row of the TTL closes the set: {address_rows:?}"
    );
    for row in &address_rows {
        assert!(
            row.contains(SHARE_MARK),
            "an address row marks its percentage as a share: {row}"
        );
    }
    let total = share_of(address_rows[0]) + share_of(address_rows[1]);
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
fn a_final_line_that_is_cut_short_warns_and_still_prints_the_frame() {
    let mut text = file_of(&[BUILT_RUN_LINE, BUILT_ROUND_LINE]);
    text.push_str(CUT_CHUNK);
    let file = TempFile::new("cut", &text);
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(result.stdout, frame(&built_header(&file, 1), &BUILT_ROWS));
    assert!(
        result.stderr.contains(path.as_str()),
        "the warning names the path: {}",
        result.stderr
    );
    assert_eq!(
        result.stderr.lines().count(),
        1,
        "a cut file of one run raises one warning and no note: {}",
        result.stderr
    );
}

#[test]
fn a_file_without_a_run_record_prints_no_target_and_marks_no_row() {
    let file = TempFile::new("no-run-record", &file_of(&[BUILT_ROUND_LINE]));
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(
        result.stdout,
        frame(
            &built_header_without_a_target(&file, 1),
            &BUILT_ROWS_WITHOUT_A_TARGET
        )
    );
    assert_eq!(
        result.stderr, "",
        "a whole file of one run writes nothing to standard error"
    );
}

/// A run that reached the target marks the row that answered from it.
///
/// The star is what the table says in the place of the word that the summary
/// line of the earlier build carried.
#[test]
fn a_run_that_reached_the_target_marks_the_row_of_it() {
    let file = TempFile::new("reached", &file_of(&[BUILT_RUN_LINE, BUILT_ROUND_LINE]));
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    let marked: Vec<&str> = result
        .stdout
        .lines()
        .filter(|line| line.contains(DESTINATION_MARK))
        .collect();
    assert_eq!(
        marked.len(),
        1,
        "one row of the frame answered from the destination: {}",
        result.stdout
    );
}

/// A run that never reached the target marks no row at all.
#[test]
fn a_run_that_did_not_reach_the_target_marks_no_row() {
    let text = file_of(&[BUILT_RUN_LINE, BUILT_MISSED_ROUND_LINE]);
    let file = TempFile::new("missed", &text);
    let path = file.arg();
    let result = success(&[REPLAY, path.as_str()]);
    assert_eq!(
        result.stdout,
        frame(&built_header(&file, 1), &BUILT_MISSED_ROWS)
    );
    assert!(
        !result.stdout.contains(DESTINATION_MARK),
        "no row of a run that never reached the target carries the mark: {}",
        result.stdout
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
