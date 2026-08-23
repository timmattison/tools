//! Black-box coverage for the `krt` command line, driving the real binary.
//!
//! The build string is what a user reads when a bug report asks which build
//! made the trace. It must name the tool, the package version, the commit, and
//! whether the tree was clean. The test reads the line the binary printed, so
//! it covers the whole path from the flag to standard output.
//!
//! The commit hash and the status change with every build, so the test asserts
//! the shape of the line and not its exact text.
//!
//! The rest of the file covers the resolved configuration. A command line that
//! names a destination prints the block and exits with success. A command line
//! that contradicts itself prints the reason on standard error and exits with a
//! failure. The `replay` command prints one summary line in the place of the
//! block, and `tests/replay.rs` covers that line.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap here is an assertion about the harness, not an unhandled error: the spawn of the freshly built binary, and the decode of the output that binary wrote. A failure of either one is a broken harness, and a panic names it at once"
)]

use std::path::PathBuf;
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

/// The text before the commit hash of the build string.
const BUILD_STRING_PREFIX: &str = "krt 0.1.0 (";

/// The separator between the commit hash and the status.
const FIELD_SEPARATOR: &str = ", ";

/// The number of characters of an abbreviated commit hash.
const HASH_LENGTH: usize = 7;

/// The text `buildinfo` writes for a field it cannot read from git.
const UNKNOWN: &str = "unknown";

/// The words `buildinfo` writes for the state of the working tree.
const STATUS_WORDS: [&str; 3] = ["clean", "dirty", UNKNOWN];

/// The name of the command that folds a recorded file.
const REPLAY: &str = "replay";

/// The exit code of a run that stopped before it recorded anything.
const EXIT_FAILURE: i32 = 1;

/// The exit code of a run whose recorded file did not take a record.
const EXIT_WRITE_FAILED: i32 = 3;

/// The exit code of a run whose tracer stopped.
const EXIT_TRACER_FAILED: i32 = 4;

/// A literal address of ip version 4.
///
/// A literal address resolves inside the machine and asks no name server, so a
/// test that names one runs offline.
const AN_IPV4_ADDRESS: &str = "1.2.3.4";

/// The loopback address of ip version 4. Every machine holds a route to it.
const LOOPBACK: &str = "127.0.0.1";

/// The flag that asks for ip version 6.
const FLAG_VERSION_6: &str = "-6";

/// The name of ip version 6, as the message of a refusal writes it.
const IPV6_IN_THE_MESSAGE: &str = "ipv6";

/// A last TTL that `krt` takes and the tracer refuses. The tracer stops at 254.
const TTL_ABOVE_THE_TRACER: &str = "255";

/// A recorded file that no test makes.
///
/// The parser rejects the command line that names it, and it does so before
/// anything opens a file, so no test needs the file to exist.
const A_RECORDED_FILE: &str = "old.jsonl";

/// The recorded file the repository holds, which carries two runs.
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/two-runs.jsonl");

/// Invoke the freshly built `krt` binary.
fn krt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_krt"))
}

/// Assert that `flag` makes `krt` print one build string and exit with success.
fn assert_flag_prints_the_build_string(flag: &str) {
    let output = krt().arg(flag).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "`krt {flag}` must exit with success, but it exited with {}; stderr: {stderr}",
        output.status
    );

    let mut lines = stdout.lines();
    let line = lines.next().unwrap_or_default();
    assert!(
        lines.next().is_none(),
        "`krt {flag}` must print one line, but it printed {stdout:?}"
    );

    let fields = line
        .strip_prefix(BUILD_STRING_PREFIX)
        .and_then(|rest| rest.strip_suffix(')'));
    let Some(fields) = fields else {
        panic!("`krt {flag}` must print `krt 0.1.0 (<hash>, <status>)`, but it printed {line:?}");
    };

    let parts: Vec<&str> = fields.split(FIELD_SEPARATOR).collect();
    let [hash, status] = parts.as_slice() else {
        panic!(
            "`krt {flag}` must print one hash and one status, but the parentheses hold {fields:?}"
        );
    };

    let hash_is_valid = *hash == UNKNOWN
        || (hash.chars().count() == HASH_LENGTH
            && hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    assert!(
        hash_is_valid,
        "`krt {flag}` must print {HASH_LENGTH} lowercase hexadecimal characters or `{UNKNOWN}` for the hash, but it printed {hash:?}"
    );

    assert!(
        STATUS_WORDS.contains(status),
        "`krt {flag}` must print one of {STATUS_WORDS:?} for the status, but it printed {status:?}"
    );
}

#[test]
fn version_flags_report_the_build_string() {
    assert_flag_prints_the_build_string("--version");
    assert_flag_prints_the_build_string("-V");
}

/// The block that `krt 1.2.3.4 -6` prints before it stops.
///
/// A trace prints the block that names what it will do, and then it acts. This
/// destination names no address of ip version 6, so the run stops at the
/// resolution, and the block is the whole of standard output.
const IPV6_REFUSAL_BLOCK: &str = "\
resolved configuration:
  destination:    1.2.3.4
  output:         derived at run time
  interval:       1s
  first ttl:      1
  max ttl:        30
  protocol:       icmp
  multipath:      classic
  address family: ipv6
  reverse dns:    on
  source:         discovered at run time
  display:        table
  duration limit: none
  round limit:    none
";

/// What one run of the binary wrote, and whether it succeeded.
struct Run {
    /// True when the binary exited with success.
    success: bool,
    /// The code that the binary exited with. A binary that a signal stopped
    /// carries none.
    code: Option<i32>,
    /// The text the binary wrote to standard output.
    stdout: String,
    /// The text the binary wrote to standard error.
    stderr: String,
}

/// Runs `krt` with the arguments and reads what it wrote.
fn run(arguments: &[&str]) -> Run {
    let output = krt().args(arguments).output().unwrap();
    Run {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Asserts that the arguments make `krt` fail, and reads the message.
fn failure(arguments: &[&str]) -> String {
    let result = run(arguments);
    assert!(
        !result.success,
        "`krt {}` must fail, but it succeeded; stdout: {}",
        arguments.join(" "),
        result.stdout
    );
    result.stderr
}

#[test]
fn a_destination_that_names_no_address_of_the_family_prints_the_block_and_stops() {
    let result = run(&[AN_IPV4_ADDRESS, FLAG_VERSION_6]);
    assert_eq!(
        result.code,
        Some(EXIT_FAILURE),
        "the run stops at the resolution; stderr: {}",
        result.stderr
    );
    assert_eq!(result.stdout, IPV6_REFUSAL_BLOCK);
    assert!(
        result.stderr.contains(IPV6_IN_THE_MESSAGE),
        "the message names the version that the run asked for: {}",
        result.stderr
    );
    assert!(
        result.stderr.contains(AN_IPV4_ADDRESS),
        "the message names the destination: {}",
        result.stderr
    );
}

// macOS is the platform that sends probes without privileges, so it is the
// platform where a trace reaches the two faults below. Linux and Windows stop
// every trace at the privilege gate first, with a different code, so the same
// test there would cover the gate and not the fault it names. The unit tests of
// `src/krt/src/trace.rs` cover that gate on every platform.

/// A path under the temporary directory of the machine.
///
/// The name keys on the process and on the moment, so two copies of one test
/// that run at the same time never share a file. `CLAUDE.md` demands it.
fn temp_path(name: &str) -> PathBuf {
    let moment = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must sit after the epoch")
        .as_nanos();
    env::temp_dir().join(format!("{name}-{}-{moment}", process::id()))
}

#[cfg(target_os = "macos")]
#[test]
fn a_recorded_file_that_will_not_open_stops_the_run() {
    let missing = temp_path("krt-no-such-directory");
    let path = missing.join(A_RECORDED_FILE);
    let result = run(&[
        LOOPBACK,
        "--output",
        path.to_str().expect("the temporary path must be text"),
    ]);
    assert_eq!(
        result.code,
        Some(EXIT_WRITE_FAILED),
        "a recorded file that will not open stops the run; stderr: {}",
        result.stderr
    );
    assert!(
        result.stderr.contains(A_RECORDED_FILE),
        "the message names the file: {}",
        result.stderr
    );
    assert!(
        !missing.exists(),
        "the run makes no directory of its own: {}",
        missing.display()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_ttl_above_what_the_tracer_takes_stops_the_run() {
    let path = temp_path("krt-ttl-above-the-limit");
    let result = run(&[
        LOOPBACK,
        "--max-ttl",
        TTL_ABOVE_THE_TRACER,
        "--output",
        path.to_str().expect("the temporary path must be text"),
    ]);
    let _ = fs::remove_file(&path);
    assert_eq!(
        result.code,
        Some(EXIT_TRACER_FAILED),
        "a tracer that refuses the configuration stops the run; stderr: {}",
        result.stderr
    );
    assert!(
        result.stderr.contains(TTL_ABOVE_THE_TRACER),
        "the message names the ttl: {}",
        result.stderr
    );
}

#[test]
fn an_interval_that_is_not_a_duration_fails_and_names_the_accepted_forms() {
    let stderr = failure(&["--interval", "bogus", "example.com"]);
    assert!(
        stderr.contains("as in `500ms`, `1s`, or `2m`"),
        "the message names the accepted forms: {stderr}"
    );
}

#[test]
fn a_first_ttl_above_the_max_ttl_fails_and_names_both_flags() {
    let stderr = failure(&["--first-ttl", "5", "--max-ttl", "3", "example.com"]);
    for flag in ["--first-ttl", "--max-ttl"] {
        assert!(
            stderr.contains(flag),
            "the message names `{flag}`: {stderr}"
        );
    }
}

#[test]
fn a_round_limit_of_zero_fails_and_names_the_flag() {
    let stderr = failure(&["--rounds", "0", "example.com"]);
    assert!(
        stderr.contains("--rounds"),
        "the message names `--rounds`: {stderr}"
    );
}

#[test]
fn both_address_family_flags_fail() {
    let stderr = failure(&["-4", "-6", "example.com"]);
    assert!(!stderr.is_empty(), "the failure carries a message");
}

#[test]
fn a_command_line_without_a_destination_fails() {
    let stderr = failure(&[]);
    assert!(!stderr.is_empty(), "the failure carries a message");
}

#[test]
fn a_destination_beside_a_replay_fails_and_names_both() {
    let stderr = failure(&["example.com", REPLAY, A_RECORDED_FILE]);
    for part in ["DESTINATION", REPLAY] {
        assert!(
            stderr.contains(part),
            "the message names `{part}`: {stderr}"
        );
    }
}

/// A replay takes no destination, and the parser asks for none.
///
/// The summary line that the replay prints belongs to `tests/replay.rs`, so
/// this test reads the exit status only.
#[test]
fn a_replay_needs_no_destination() {
    let result = run(&[REPLAY, FIXTURE]);
    assert!(
        result.success,
        "`krt {REPLAY} {FIXTURE}` must exit with success; stderr: {}",
        result.stderr
    );
}
