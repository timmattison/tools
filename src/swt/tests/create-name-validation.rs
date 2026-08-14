//! End-to-end coverage of the name check `swt create` performs before it
//! touches git.
//!
//! The unit tests in `git.rs` pin the predicate; this pins the part a user
//! actually experiences — that a bad name stops the command, on stderr, with a
//! failing status, and that it stops it *early*. The test deliberately runs
//! outside any repository swt could act on, so a pass proves the rejection
//! happened before the first git call rather than after it.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// A name that must never reach git: it traverses out of the worktree parent
/// directory and would put the branch and the checkout somewhere unrelated.
const TRAVERSING_NAME: &str = "../evil";

/// Names that look like options. Each is a name swt refuses, and the refusal is
/// swt's own — a hyphen-leading argument must not be read as a flag on the way
/// in, or the user is told the argument was "unexpected" instead of being told
/// which rule it broke.
const OPTION_LOOKING_NAMES: [&str; 3] = ["-b", "-rf", "--force"];

/// The rule quoted back to the user, verbatim, when a name is refused.
const WORKTREE_NAME_RULE: &str =
    "allowed: letters, digits, '.', '_' and '-'; must not start with '-', and must not be '.' or '..'";

/// Creates an empty directory no concurrent test run can also be using, so this
/// test's working directory is entirely its own.
fn unique_scratch_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "swt-create-name-validation-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("scratch directory should be creatable");
    dir
}

/// Runs the freshly built `swt` binary with `args` from a directory of its own.
fn swt_in_scratch_dir(args: &[&str]) -> Output {
    let dir = unique_scratch_dir();
    let output = Command::new(env!("CARGO_BIN_EXE_swt"))
        .args(args)
        .current_dir(&dir)
        .output()
        .expect("failed to run swt");
    // Best effort: a leaked empty directory is harmless, a panic that hides the
    // assertion below is not.
    let _ = fs::remove_dir_all(&dir);
    output
}

/// A name that would escape the worktree parent directory is refused outright,
/// naming both the offending input and the rule it broke.
#[test]
fn create_rejects_a_traversing_name_before_touching_git() {
    let output = swt_in_scratch_dir(&["create", TRAVERSING_NAME]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "`swt create {TRAVERSING_NAME}` should exit 1, stderr was: {stderr}"
    );
    assert_eq!(
        stderr,
        format!("Invalid worktree name {TRAVERSING_NAME:?} — {WORKTREE_NAME_RULE}.\n"),
        "the rejection should name the input and quote the rule verbatim"
    );
}

/// A name that starts with a hyphen is still a name. It is refused for breaking
/// the naming rule — by swt, with swt's message and swt's status — and never
/// mistaken for an option `swt` does not have.
#[test]
fn create_rejects_option_looking_names_with_its_own_message() {
    for name in OPTION_LOOKING_NAMES {
        let output = swt_in_scratch_dir(&["create", name]);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(1),
            "`swt create {name}` should exit 1, stderr was: {stderr}"
        );
        assert_eq!(
            stderr,
            format!("Invalid worktree name {name:?} — {WORKTREE_NAME_RULE}.\n"),
            "`swt create {name}` should be refused by swt's own name check"
        );
    }
}

/// Letting a hyphen-leading name through must not cost the subcommands their
/// help: `--help` after a subcommand still prints that subcommand's usage.
#[test]
fn subcommand_help_still_prints_usage() {
    for command in ["create", "merge"] {
        let output = swt_in_scratch_dir(&[command, "--help"]);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert_eq!(
            output.status.code(),
            Some(0),
            "`swt {command} --help` should exit 0, stdout was: {stdout}"
        );
        assert!(
            stdout.contains(&format!("Usage: swt {command}")),
            "`swt {command} --help` should print its usage line, stdout was: {stdout}"
        );
    }
}
