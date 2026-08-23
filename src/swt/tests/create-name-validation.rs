//! End-to-end coverage of the name check `swt create` performs before it
//! touches git.
//!
//! The unit tests in `git.rs` pin the predicate; this pins the part a user
//! actually experiences — that a bad name stops the command, on stderr, with a
//! failing status, and that it stops it *early*. The test deliberately runs
//! outside any repository swt could act on, so a pass proves the rejection
//! happened before the first git call rather than after it.
//!
//! That claim is only worth as much as the sandbox under it. It goes through
//! [`support::run_swt_outside_a_repository`] like every other spawn in the
//! suite, so should validation ever be reordered to run *after* a git call, the
//! stray git runs against an empty scratch directory and fails the test —
//! rather than picking up an inherited `GIT_DIR` and operating on the real
//! checkout while this test still passes.

mod support;

use support::{run_swt_outside_a_repository, OPTION_LOOKING_NAMES};

/// A name that must never reach git: it traverses out of the worktree parent
/// directory and would put the branch and the checkout somewhere unrelated.
const TRAVERSING_NAME: &str = "../evil";

/// The rule quoted back to the user, verbatim, when a name is refused.
const WORKTREE_NAME_RULE: &str =
    "allowed: letters, digits, '.', '_' and '-'; must not start with '-', and must not be '.' or '..'";

/// A name that would escape the worktree parent directory is refused outright,
/// naming both the offending input and the rule it broke.
#[test]
fn create_rejects_a_traversing_name_before_touching_git() {
    let output = run_swt_outside_a_repository(&["create", TRAVERSING_NAME]);
    let stderr = support::stderr(&output);

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
        let output = run_swt_outside_a_repository(&["create", name]);
        let stderr = support::stderr(&output);

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
        let output = run_swt_outside_a_repository(&[command, "--help"]);
        let stdout = support::stdout(&output);

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
