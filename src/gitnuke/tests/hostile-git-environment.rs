//! `gitnuke` acts on the repository it stands in, never on the one that an
//! inherited `GIT_DIR` names.
//!
//! `gitnuke` removes a worktree and force-deletes a branch. Under a hook, git
//! exports `GIT_DIR` and its companions into each child. A `gitnuke` that
//! keeps them removes a worktree and a branch of the wrong repository. The
//! decoy here holds a branch and a worktree with the same name as the fixture,
//! so a leak finds a real target and removes it.

use std::process::{Command, Stdio};

use gitscratch::testing::{DecoyRepository, TestRepo};

const BRANCH: &str = "nuke-me";

/// A repository with one commit and a linked worktree on `BRANCH`.
fn repo_with_linked_worktree() -> (TestRepo, std::path::PathBuf) {
    let repo = TestRepo::init();
    repo.commit_file("a.txt", "a\n", "initial");
    let home = repo.git(&["branch", "--show-current"]).trim().to_string();
    repo.branch(BRANCH);
    repo.checkout(&home);
    let worktree = repo.add_worktree(BRANCH);
    (repo, worktree)
}

/// The branch names of the repository at `dir`, read with a clean environment.
fn branches(dir: &std::path::Path) -> String {
    let mut command = childenv::fixture_command("git");
    command
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    let output = command.output().expect("run git branch");
    assert!(output.status.success(), "git branch failed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn nukes_the_worktree_of_the_repository_it_stands_in_under_a_hostile_environment() {
    let (fixture, fixture_worktree) = repo_with_linked_worktree();
    let (decoy_repo, decoy_worktree) = repo_with_linked_worktree();
    let decoy = DecoyRepository::of(decoy_repo);

    let mut command: Command = childenv::fixture_command(env!("CARGO_BIN_EXE_gitnuke"));
    command
        .arg(BRANCH)
        .current_dir(fixture.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::null());
    decoy.aim(&mut command);
    let output = command.output().expect("run gitnuke");

    // Controls: the run did its work on the fixture.
    assert!(
        output.status.success(),
        "gitnuke failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !fixture_worktree.exists(),
        "the worktree of the fixture is still there"
    );
    assert!(
        !branches(fixture.path()).lines().any(|name| name == BRANCH),
        "the fixture still has the branch {BRANCH}"
    );

    // The decoy keeps its branch and its worktree.
    assert!(
        branches(decoy.path()).lines().any(|name| name == BRANCH),
        "gitnuke deleted the branch {BRANCH} of the decoy"
    );
    assert!(
        decoy_worktree.exists(),
        "gitnuke removed the worktree of the decoy"
    );
    decoy.assert_untouched("gitnuke");
}
