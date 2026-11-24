//! `cwt` must list the worktrees of the repository it stands in, whatever
//! `GIT_` variables it inherits.
//!
//! Git exports `GIT_DIR` and its siblings into every hook. A `cwt` that a hook
//! starts must still read the worktrees of its own repository. This test aims
//! those variables at a decoy repository that also has a worktree named
//! `feature`. A leak then resolves to a real path in the decoy, not to a
//! "not found" error. The path in the fixture is the control.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod common;

use std::process::Stdio;

use common::{add_worktree, init_repo};
use gitscratch::testing::{DecoyRepository, TestRepo};

/// The branch and the directory name of the worktree in both repositories.
const BRANCH: &str = "feature";

#[test]
fn cwt_resolves_the_worktree_of_its_own_repository_under_a_hostile_git_environment() {
    let (temp, repo) = init_repo("main");
    let fixture_worktree = add_worktree(&temp, &repo, BRANCH);

    let decoy_repo = TestRepo::init();
    decoy_repo.commit_file("README.md", "decoy\n", "decoy baseline");
    let decoy_worktree = decoy_repo.path().join(BRANCH);
    decoy_repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        BRANCH,
        decoy_worktree.to_str().expect("utf-8 decoy worktree path"),
    ]);
    let decoy = DecoyRepository::of(decoy_repo);

    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_cwt"));
    command
        .arg(BRANCH)
        .current_dir(&repo)
        .env("NO_COLOR", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::null());
    decoy.aim(&mut command);
    let output = command.output().expect("failed to run cwt");

    assert!(
        output.status.success(),
        "cwt {BRANCH} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8(output.stdout).expect("cwt printed invalid UTF-8");
    let printed = printed.trim();
    let actual = std::fs::canonicalize(printed)
        .unwrap_or_else(|e| panic!("cwt printed {printed:?}, which is not a path: {e}"));
    let expected = std::fs::canonicalize(&fixture_worktree).expect("fixture worktree exists");
    let decoy_canonical = std::fs::canonicalize(&decoy_worktree).expect("decoy worktree exists");
    assert_ne!(
        actual, decoy_canonical,
        "cwt resolved the worktree of the decoy repository"
    );
    assert_eq!(actual, expected, "cwt did not resolve the fixture worktree");

    decoy.assert_untouched("cwt");
}
