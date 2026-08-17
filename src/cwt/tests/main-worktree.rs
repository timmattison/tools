//! End-to-end coverage for `cwt --main`, the flag behind the `wtm` alias.
//!
//! The unit tests in `main.rs` prove how `find_main_worktree` ranks branch
//! names. These tests prove that the binary applies that ranking to a real
//! repository: a repository that never renamed `master` still gets a main
//! worktree, `main` wins wherever both branches exist, and a branch that merely
//! contains the text "main" never captures the shortcut.
//!
//! The repository fixtures and the binary runner are shared with the other
//! end-to-end targets and live in [`common`]. Only the `--main` assertions are
//! here.

// These mirror the crate-root attributes in src/main.rs. A crate-root attribute
// reaches only its own target, so the binary raising them does nothing for this
// test target; repeating them here is what keeps the whole crate under one lint
// set now that they no longer live in a manifest `[lints]` table.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod common;

use std::path::Path;

use common::{add_worktree, cwt_main, init_repo, WORKTREE_NOT_FOUND};

/// Asserts that `cwt --main`, run in `from`, printed the path of `expected`.
///
/// Both paths are canonicalized because macOS reports the temporary directory
/// through the `/var` symlink.
fn assert_main_worktree_is(from: &Path, expected: &Path) {
    let output = cwt_main(from);
    assert!(
        output.status.success(),
        "cwt --main failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let printed = String::from_utf8(output.stdout).expect("cwt printed invalid UTF-8");
    let printed = printed.trim();
    let actual = std::fs::canonicalize(printed)
        .unwrap_or_else(|e| panic!("cwt printed {printed:?}, which is not a path: {e}"));
    assert_eq!(
        actual,
        std::fs::canonicalize(expected).expect("the expected worktree does not exist"),
    );
}

#[test]
fn master_only_repository_resolves_to_master() {
    let (temp, repo) = init_repo("master");
    let feature = add_worktree(&temp, &repo, "feature");

    assert_main_worktree_is(&feature, &repo);
}

#[test]
fn main_wins_when_both_branches_have_worktrees() {
    let (temp, repo) = init_repo("main");
    let master = add_worktree(&temp, &repo, "master");

    assert_main_worktree_is(&master, &repo);
}

#[test]
fn branch_containing_main_does_not_capture_the_main_worktree() {
    let (temp, repo) = init_repo("master");
    let decoy = add_worktree(&temp, &repo, "wt-main-master");

    assert_main_worktree_is(&decoy, &repo);
}

#[test]
fn repository_without_main_or_master_reports_not_found() {
    let (temp, repo) = init_repo("trunk");
    let feature = add_worktree(&temp, &repo, "feature");

    let output = cwt_main(&feature);
    assert_eq!(output.status.code(), Some(WORKTREE_NOT_FOUND));
    assert!(
        output.stdout.is_empty(),
        "cwt must print no path when it finds no main worktree"
    );
}

#[test]
fn not_found_message_names_every_branch_that_was_searched() {
    let (temp, repo) = init_repo("trunk");
    let feature = add_worktree(&temp, &repo, "feature");

    let output = cwt_main(&feature);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("'main' or 'master'"),
        "the not-found message must name every branch cwt searched for, got: {stderr}"
    );
}
