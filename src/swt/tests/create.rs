//! `swt create` end to end: the green gate, where the check runs, where its
//! configuration comes from, and what is left behind when it says no.
//!
//! Every case here drives the real binary, because every guarantee `create`
//! makes is about the world outside the process — a directory that exists or
//! does not, a branch that survives or does not, a path on stdout that a caller
//! captures. The in-process tests can pin the plan; only a subprocess can pin
//! that the worktree is gone.
//!
//! Unix only: the fixtures are `sh` scripts dropped as executable `.swt-check`
//! overrides, which is precisely how the escape hatch is documented.
#![cfg(unix)]

mod support;

use std::fs;
use std::process::{Command, Output};

use support::{exiting_check, run_swt, unique, write_swt_check, TestRepo, SWT_CHECK, TRACKED_FILE};

/// A check that records the directory it ran in. `pwd -P` asks the kernel rather
/// than trusting an inherited `PWD`, so the answer is the cwd `swt` chose.
const RECORD_CWD_CHECK: &str = "#!/bin/sh\npwd -P > ran-in\n";

/// The file [`RECORD_CWD_CHECK`] writes, in whatever directory it ran in.
const CWD_MARKER: &str = "ran-in";

/// A check that passes only against an uncommitted edit in the parent worktree.
/// In a clean checkout of HEAD the tracked file still reads `original`.
const NEEDS_PARENT_EDIT_CHECK: &str = "#!/bin/sh\ngrep -q MODIFIED tracked.txt\n";

/// The uncommitted content [`NEEDS_PARENT_EDIT_CHECK`] looks for.
const PARENT_EDIT: &str = "MODIFIED\n";

/// A check that deletes the worktree's own `.git` link and then fails, so the
/// teardown that follows cannot succeed either: git refuses to remove a working
/// tree whose `.git` has vanished, and refuses to delete a branch a registered
/// worktree still claims. No permission games, so it behaves the same for an
/// unprivileged user and for root.
const SABOTAGE_CHECK: &str = "#!/bin/sh\nrm -f .git\nexit 1\n";

/// Decodes a finished run's stdout.
fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Decodes a finished run's stderr.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The `git branch --list` pattern matching every branch a `swt create <name>`
/// could have left behind.
fn branch_pattern(name: &str) -> String {
    format!("swt/{name}-*")
}

/// Sorted names of everything sitting beside the repository, so an orphaned
/// worktree cannot hide by being merely un-asserted-about.
fn beside_the_repo(repo: &TestRepo) -> Vec<String> {
    let mut entries: Vec<String> = fs::read_dir(repo.siblings())
        .expect("the fixture's sibling directory should be readable")
        .map(|entry| {
            entry
                .expect("sibling directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();
    entries
}

// The whole point of the command: a worktree branched from a verified HEAD, and
// its path on stdout with nothing else beside it — callers capture stdout, so
// anything chatty there is a bug, not noise.
#[test]
fn a_green_check_yields_a_worktree_a_branch_and_only_the_path_on_stdout() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), &exiting_check(0));
    let name = unique("green");
    let expected = repo.siblings().join(format!("{name}.swt"));

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a green check must produce a worktree: {stderr}"
    );
    assert_eq!(
        stdout_of(&output),
        format!("{}\n", expected.display()),
        "stdout carries the path and nothing else, so a caller can capture it"
    );
    assert!(
        expected.is_dir(),
        "the verified worktree must still be there at {}",
        expected.display()
    );
    let branches = repo.branches(&branch_pattern(&name));
    assert_eq!(
        branches.len(),
        1,
        "exactly one branch should have been created, got {branches:?}"
    );
    assert!(
        branches[0].starts_with(&format!("swt/{name}-")),
        "the branch should be the name under the swt namespace: {branches:?}"
    );
}

// The two halves of the same design decision. The check runs *inside* the fresh
// worktree — anything else verifies a tree nobody is branching from — while the
// `.swt-check` override is read from the *parent*, because it is an uncommitted
// per-developer file that a clean checkout of HEAD by definition does not have.
#[test]
fn the_check_runs_in_the_new_worktree_from_an_override_only_the_parent_has() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), RECORD_CWD_CHECK);
    let name = unique("inside");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "the parent's override should have been found and passed: {stderr}"
    );
    let recorded = fs::read_to_string(worktree.join(CWD_MARKER))
        .expect("the check should have recorded its own cwd inside the new worktree");
    assert_eq!(
        recorded.trim(),
        worktree.to_string_lossy(),
        "the check must run in the worktree being verified"
    );
    assert!(
        !repo.path().join(CWD_MARKER).exists(),
        "the parent supplies the check; it is never the directory it runs in"
    );
    assert!(
        !worktree.join(SWT_CHECK).exists(),
        "the fresh checkout has no override of its own — the plan came from the parent"
    );
}

// The inverse of the override case, and the failure mode that would make `swt`
// worthless: a repository where nothing can be detected is not green by default.
#[test]
fn a_repository_with_no_check_anywhere_fails_instead_of_reporting_a_vacuous_green() {
    let repo = TestRepo::new();
    let name = unique("nocheck");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "an undetectable check must fail the command: {stderr}"
    );
    assert!(
        stderr.contains("HEAD not green: No green-check defined"),
        "the user should be told no check applied, not handed a green: {stderr}"
    );
    assert!(
        stderr.contains(&repo.path().display().to_string()),
        "the override belongs at the parent root, so that is the path to name: {stderr}"
    );
    assert!(
        !worktree.exists(),
        "an unverified worktree must not survive at {}",
        worktree.display()
    );
    assert!(
        repo.branches(&branch_pattern(&name)).is_empty(),
        "an unverified branch must not survive either"
    );
}

// A red check leaves nothing behind — worktree, branch, or a directory beside
// the repo — and says so, because the user asked for a worktree and is getting
// none.
#[test]
fn a_red_check_tears_the_worktree_and_the_branch_down_and_says_so() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), &exiting_check(1));
    let name = unique("red");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a red check must fail the command: {stderr}"
    );
    assert!(
        stderr.contains("HEAD not green:"),
        "the red verdict should be reported: {stderr}"
    );
    assert!(
        !worktree.exists(),
        "a red check left an orphaned worktree at {}: {stderr}",
        worktree.display()
    );
    assert!(
        repo.branches(&branch_pattern(&name)).is_empty(),
        "a red check left an orphaned branch: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "Cleaned up worktree {} and branch swt/{name}-",
            worktree.display()
        )),
        "a cleanup that happened should be reported: {stderr}"
    );
    assert_eq!(
        beside_the_repo(&repo),
        vec!["repo".to_string()],
        "nothing at all may be left beside the repository: {stderr}"
    );
    assert_eq!(
        stdout_of(&output),
        "",
        "a failed create prints no path for a caller to capture"
    );
}

// Teardown is best-effort, so its success is reported rather than assumed.
// Claiming a cleanup that did not happen is worse than not cleaning up at all:
// it strands the user with an orphaned worktree *and* branch they were told did
// not exist.
#[test]
fn a_teardown_that_failed_is_never_reported_as_a_cleanup() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), SABOTAGE_CHECK);
    let name = unique("sabotaged");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a red check must fail the command: {stderr}"
    );
    // The claim would only be a lie if the orphans really are orphans.
    assert!(
        worktree.exists(),
        "fixture precondition: {} should have survived teardown",
        worktree.display()
    );
    let branches = repo.branches(&branch_pattern(&name));
    assert_eq!(
        branches.len(),
        1,
        "fixture precondition: the branch should have survived too, got {branches:?}"
    );
    assert!(
        !stderr.contains("Cleaned up"),
        "claimed a cleanup while {} and {branches:?} both survived: {stderr}",
        worktree.display()
    );
    assert!(
        stderr.contains("fatal:"),
        "git's own account of the failed teardown was swallowed: {stderr}"
    );
    assert!(
        stderr.contains(&format!("Could not clean up {}.", worktree.display())),
        "the user should be told the cleanup did not work: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "  git worktree remove --force '{}' && git branch -D swt/{name}-",
            worktree.display()
        )),
        "no copy-pasteable recovery command naming the quoted path and the branch: {stderr}"
    );
}

// The reason the check runs in the fresh worktree at all. A check that passes
// against the parent's uncommitted edit must fail in a clean checkout of HEAD —
// otherwise `swt` would hand a subagent a worktree branched from a commit that
// was never green.
#[test]
fn uncommitted_parent_state_cannot_fake_a_green() {
    let repo = TestRepo::new();
    fs::write(repo.path().join(TRACKED_FILE), PARENT_EDIT).expect("uncommitted parent edit");
    let check = write_swt_check(repo.path(), NEEDS_PARENT_EDIT_CHECK);
    let name = unique("dirty");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    // Mutation guard: run the very same check against the parent, where it must
    // pass. Without this the test could be passing because the check is simply
    // broken everywhere.
    let in_parent = Command::new(&check)
        .current_dir(repo.path())
        .status()
        .expect("the fixture check should run");
    assert!(
        in_parent.success(),
        "fixture precondition: this check passes against the parent's uncommitted edit"
    );

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a check that only passes on uncommitted parent state must not report green: {stderr}"
    );
    assert!(
        stderr.contains("HEAD not green:"),
        "the red verdict should be reported: {stderr}"
    );
    assert!(
        !worktree.exists(),
        "the unverified worktree must be gone: {stderr}"
    );
    assert!(
        repo.branches(&branch_pattern(&name)).is_empty(),
        "the unverified branch must be gone: {stderr}"
    );
}

// Everything `create` does starts from `git rev-parse --show-toplevel`, so the
// one place there is no repository at all has to stop the command with git's own
// explanation rather than inventing a path beside nothing.
#[test]
fn create_outside_a_repository_fails_with_gits_own_complaint() {
    let scratch = tempfile::Builder::new()
        .prefix("swt-create-no-repo-")
        .tempdir()
        .expect("scratch temp dir");
    let dir = fs::canonicalize(scratch.path()).expect("canonical scratch dir");

    let output = run_swt(&dir, &["create", &unique("orphan")]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a create with no repository to branch from must fail: {stderr}"
    );
    assert_eq!(
        stdout_of(&output),
        "",
        "a failed create prints no path for a caller to capture"
    );
    assert!(
        stderr.contains("not a git repository"),
        "git's own explanation should reach the user: {stderr}"
    );
}
