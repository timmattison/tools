#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar worktree <name> [<branch>]`, tested through the binary.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::Fixture;

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// Makes a fixture and runs `sidecar setup` in the main checkout.
fn after_setup() -> Fixture {
    let fixture = Fixture::new(&[NAME]);
    set_up_in(&fixture, &fixture.main_checkout());
    fixture
}

/// Runs `sidecar setup` in `worktree` and asserts success.
fn set_up_in(fixture: &Fixture, worktree: &Path) {
    let run = fixture.run(worktree, &["setup"]);
    assert_eq!(run.code, 0, "{run:?}");
}

/// What the link of `name` in `worktree` points at, canonical.
fn link_target(fixture: &Fixture, worktree: &Path, name: &str) -> PathBuf {
    fs::canonicalize(fixture.link(worktree, name)).expect("the link points at a path")
}

/// The branch checked out in the directory `place`.
fn branch_of(fixture: &Fixture, place: &Path) -> String {
    fixture.git(place, &["symbolic-ref", "--short", "HEAD"])
}

/// Asserts that a run of `sidecar worktree` made the sidecar worktree for `branch`,
/// points the link of `worktree` at it, and names the link, the target, and the branch.
fn assert_pointed_at_new_worktree(
    fixture: &Fixture,
    worktree: &Path,
    run: &support::Run,
    branch: &str,
) {
    assert_eq!(run.code, 0, "{run:?}");
    let target = fixture.worktrees_dir(NAME).join(branch);
    assert!(target.is_dir(), "{} is missing", target.display());
    assert_eq!(branch_of(fixture, &target), branch);
    assert_eq!(
        link_target(fixture, worktree, NAME),
        fs::canonicalize(&target).unwrap()
    );
    assert!(run.stdout.contains("data/testdata"), "{run:?}");
    assert!(run.stdout.contains(&*target.to_string_lossy()), "{run:?}");
    assert!(
        run.stdout.contains(&format!("(branch {branch})")),
        "{run:?}"
    );
}

#[test]
fn no_branch_uses_the_current_branch_of_the_host() {
    let fixture = after_setup();
    let host = fixture.add_host_worktree("issue-123");
    set_up_in(&fixture, &host);

    let run = fixture.run(&host, &["worktree", NAME]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "issue-123");
}

#[test]
fn a_named_branch_is_used() {
    let fixture = after_setup();
    let host = fixture.add_host_worktree("issue-123");
    set_up_in(&fixture, &host);

    let run = fixture.run(&host, &["worktree", NAME, "other"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "other");
}

#[test]
fn a_branch_with_a_slash_makes_nested_directories() {
    let fixture = after_setup();
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["worktree", NAME, "team/topic"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "team/topic");
    assert!(fixture.worktrees_dir(NAME).join("team").is_dir());
}
