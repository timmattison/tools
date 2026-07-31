//! Black-box coverage of the `swt` command line surface.
//!
//! `swt` is the gate other agents' worktrees pass through, so the two things a
//! caller can depend on before any git runs are pinned here: a version string
//! that identifies the exact build, and a usage error — never a silent success —
//! for every invocation that does not name a command and its one argument.

use std::process::{Command, Output};

use regex::Regex;

/// Conventional shell exit status for a command line usage error.
const USAGE_EXIT_STATUS: i32 = 2;

/// Runs the freshly built `swt` binary with `args` and captures its output.
fn swt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_swt"))
        .args(args)
        .output()
        .expect("failed to run swt")
}

/// Asserts that `output` is a usage error: exit status 2 and an explanation on
/// stderr, so a caller that got its arguments wrong hears about it.
fn assert_usage_error(output: &Output, invocation: &str) {
    assert_eq!(
        output.status.code(),
        Some(USAGE_EXIT_STATUS),
        "`{invocation}` should exit {USAGE_EXIT_STATUS}, stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stderr.is_empty(),
        "`{invocation}` should explain itself on stderr"
    );
}

/// `--version` reports the package version plus the git build it came from, so
/// a bug report names an exact binary.
#[test]
fn version_reports_the_build_it_came_from() {
    let output = swt(&["--version"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "swt --version failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pattern = Regex::new(r"^swt \d+\.\d+\.\d+ \(.+, (clean|dirty)\)$")
        .expect("version pattern should compile");
    assert!(
        pattern.is_match(stdout.trim()),
        "version output should look like `swt 0.1.0 (abc1234, clean)`, got: {stdout}"
    );
}

/// With no command at all the user learns both commands rather than nothing.
#[test]
fn bare_invocation_is_a_usage_error_naming_both_commands() {
    let output = swt(&[]);

    assert_usage_error(&output, "swt");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("create") && stderr.contains("merge"),
        "usage should name both commands, got: {stderr}"
    );
}

/// `create` names a worktree; without one there is nothing to create.
#[test]
fn create_without_a_name_is_a_usage_error() {
    assert_usage_error(&swt(&["create"]), "swt create");
}

/// `merge` names a worktree path; without one there is nothing to merge.
#[test]
fn merge_without_a_worktree_path_is_a_usage_error() {
    assert_usage_error(&swt(&["merge"]), "swt merge");
}

/// An unrecognized command must fail loudly rather than be treated as one of
/// the real ones.
#[test]
fn unknown_command_is_a_usage_error() {
    assert_usage_error(&swt(&["bogus"]), "swt bogus");
}
