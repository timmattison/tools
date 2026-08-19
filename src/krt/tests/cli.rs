//! Black-box coverage for the `krt` command line, driving the real binary.
//!
//! The build string is what a user reads when a bug report asks which build
//! made the trace. It must name the tool, the package version, the commit, and
//! whether the tree was clean. The test reads the line the binary printed, so
//! it covers the whole path from the flag to standard output.
//!
//! The commit hash and the status change with every build, so the test asserts
//! the shape of the line and not its exact text.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap here is an assertion about the harness, not an unhandled error: the spawn of the freshly built binary, and the decode of the output that binary wrote. A failure of either one is a broken harness, and a panic names it at once"
)]

use std::process::Command;

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
        panic!("`krt {flag}` must print one hash and one status, but the parentheses hold {fields:?}");
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
