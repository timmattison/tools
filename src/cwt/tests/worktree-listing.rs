//! End-to-end coverage for the worktree list that `cwt` prints under an error.
//!
//! Two different failures list worktrees: "no main worktree" and "not found"
//! list every worktree, and "multiple matches" lists only the ones that
//! matched. The sets differ, the rendering does not, so these tests pin the
//! rendered line at both sites and would catch the two drifting apart.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod common;

use common::{
    add_worktree, cwt, cwt_main, init_repo, MULTIPLE_MATCHES, REPO_DIR_NAME, WORKTREE_NOT_FOUND,
};

/// Renders the line `cwt` is expected to print for one worktree.
///
/// The indent is two spaces. It is part of the format under test, so it is
/// spelled out here and the assertions compare whole lines rather than
/// trimming it away.
fn worktree_line(dir: &str, branch: &str) -> String {
    format!("  {dir} [{branch}]")
}

/// Asserts that `stderr` has a line listing the worktree `dir` on `branch`.
fn assert_lists(stderr: &str, dir: &str, branch: &str) {
    let expected = worktree_line(dir, branch);
    assert!(
        stderr.lines().any(|line| line == expected),
        "expected a line {expected:?} in stderr, got:\n{stderr}"
    );
}

/// Asserts that `stderr` has no line listing the worktree `dir` on `branch`.
fn assert_does_not_list(stderr: &str, dir: &str, branch: &str) {
    let unwanted = worktree_line(dir, branch);
    assert!(
        !stderr.lines().any(|line| line == unwanted),
        "did not expect a line {unwanted:?} in stderr, got:\n{stderr}"
    );
}

#[test]
fn not_found_lists_every_worktree() {
    let (temp, repo) = init_repo("trunk");
    add_worktree(&temp, &repo, "feature");

    let output = cwt_main(&repo);
    assert_eq!(output.status.code(), Some(WORKTREE_NOT_FOUND));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Available worktrees:"),
        "the not-found error must introduce the list, got:\n{stderr}"
    );
    assert_lists(&stderr, REPO_DIR_NAME, "trunk");
    assert_lists(&stderr, "feature", "feature");
}

#[test]
fn multiple_matches_lists_every_candidate() {
    let (temp, repo) = init_repo("trunk");
    add_worktree(&temp, &repo, "feature-alpha");
    add_worktree(&temp, &repo, "feature-beta");

    let output = cwt(&repo, &["feature"]);
    assert_eq!(output.status.code(), Some(MULTIPLE_MATCHES));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Multiple worktrees match 'feature'"),
        "the ambiguous-match error must name the search term, got:\n{stderr}"
    );
    assert_lists(&stderr, "feature-alpha", "feature-alpha");
    assert_lists(&stderr, "feature-beta", "feature-beta");
    // This arm lists the matches, not every worktree. That is the difference
    // that kept it off the shared helper; the rendered line is still shared.
    assert_does_not_list(&stderr, REPO_DIR_NAME, "trunk");
}
