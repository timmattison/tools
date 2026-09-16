//! Black-box coverage of the `swt` command line surface.
//!
//! `swt` is the gate other agents' worktrees pass through, so the two things a
//! caller can depend on before any git runs are pinned here: a version string
//! that identifies the exact build, and a usage error — never a silent success —
//! for every invocation that does not name a command with the arguments that
//! the command takes.
//!
//! Nothing here can reach git: every case is a clap usage error or `--version`.
//! It still spawns through the shared harness, because "this file happens to be
//! safe" is a property of today's argument parsing, not a rule, and one file
//! spawning the binary its own way is how the sandbox drifts apart again.

use std::process::Output;

use regex::Regex;

mod support;

use support::run_swt_outside_a_repository;

/// Conventional shell exit status for a command line usage error.
const USAGE_EXIT_STATUS: i32 = 2;

/// Every command that `swt` accepts. The bare usage must name each of them.
const COMMANDS: [&str; 3] = ["create", "merge", "list"];

/// Whether the usage text `usage` lists `command` as an entry: a line whose
/// first word is the command.
///
/// A match on a substring is not sufficient. It also accepts a word that only
/// contains the command, such as `listing` in the description of another
/// command.
fn names_command(usage: &str, command: &str) -> bool {
    usage
        .lines()
        .any(|line| line.split_whitespace().next() == Some(command))
}

/// Asserts that `output` is a usage error: exit status 2 and an explanation on
/// stderr, so a caller that got its arguments wrong hears about it.
fn assert_usage_error(output: &Output, invocation: &str) {
    assert_eq!(
        output.status.code(),
        Some(USAGE_EXIT_STATUS),
        "`{invocation}` should exit {USAGE_EXIT_STATUS}, stderr was: {}",
        support::stderr(output)
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
    let output = run_swt_outside_a_repository(&["--version"]);
    let stdout = support::stdout(&output);

    assert!(
        output.status.success(),
        "swt --version failed: {}",
        support::stderr(&output)
    );
    let pattern = Regex::new(r"^swt \d+\.\d+\.\d+ \(.+, (clean|dirty)\)$")
        .expect("version pattern should compile");
    assert!(
        pattern.is_match(stdout.trim()),
        "version output should look like `swt 0.1.0 (abc1234, clean)`, got: {stdout}"
    );
}

/// With no command at all the user learns every command rather than nothing.
#[test]
fn bare_invocation_is_a_usage_error_naming_all_three_commands() {
    let output = run_swt_outside_a_repository(&[]);

    assert_usage_error(&output, "swt");
    let stderr = support::stderr(&output);
    for command in COMMANDS {
        assert!(
            names_command(&stderr, command),
            "usage should name the command {command:?}, got: {stderr}"
        );
    }
}

/// `create` names a worktree; without one there is nothing to create.
#[test]
fn create_without_a_name_is_a_usage_error() {
    assert_usage_error(&run_swt_outside_a_repository(&["create"]), "swt create");
}

/// `merge` names a worktree path; without one there is nothing to merge.
#[test]
fn merge_without_a_worktree_path_is_a_usage_error() {
    assert_usage_error(&run_swt_outside_a_repository(&["merge"]), "swt merge");
}

/// An unrecognized command must fail loudly rather than be treated as one of
/// the real ones.
#[test]
fn unknown_command_is_a_usage_error() {
    assert_usage_error(&run_swt_outside_a_repository(&["bogus"]), "swt bogus");
}
