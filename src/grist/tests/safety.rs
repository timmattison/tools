//! `grist` runs against the developer's real repository. These tests pin the
//! properties that make that acceptable.

use std::process::Command;

use gitscratch::testing::conflicting_repo;
use grist::{BranchName, Simulator};

fn order(names: &[&str]) -> Vec<BranchName> {
    names.iter().map(|n| BranchName::new(*n)).collect()
}

/// `rebase.updateRefs` rewrites any branch pointing into the range being
/// replayed - which is exactly the branch under simulation. A developer who has
/// turned it on must not lose their branches to a dry run.
#[test]
fn never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled() {
    let repo = conflicting_repo();
    repo.git(&["config", "rebase.updateRefs", "true"]);

    let before: Vec<(String, String)> = ["main", "left", "right"]
        .iter()
        .map(|name| ((*name).to_string(), repo.rev_parse(name)))
        .collect();

    let simulator = Simulator::new(repo.path(), "main");
    simulator
        .score(&order(&["left", "right"]))
        .expect("simulation runs");

    for (name, sha) in before {
        assert_eq!(
            repo.rev_parse(&name),
            sha,
            "simulation moved branch '{name}'"
        );
    }
}

/// The branches worth comparing are usually the ones already checked out in
/// other worktrees - which is exactly the situation where a plain `git checkout`
/// refuses to run. Simulation must detach instead.
#[test]
fn works_when_the_branches_are_checked_out_in_other_worktrees() {
    let repo = conflicting_repo();
    let _left = repo.add_worktree("left");
    let _right = repo.add_worktree("right");

    let simulator = Simulator::new(repo.path(), "main");

    let ranked = simulator
        .evaluate(&order(&["left", "right"]))
        .expect("evaluation runs even though both branches are checked out elsewhere");

    assert_eq!(ranked.len(), 2);
}

/// A worktree's directory can be temporarily unreachable while the worktree
/// itself is perfectly alive - an external drive unmounted, a network mount
/// asleep, a directory moved aside for a minute. Everything that makes it
/// recoverable, including any halted rebase, lives in the real repository under
/// `.git/worktrees/`. Repo-wide cleanup deletes that state on sight and with no
/// grace period, so a simulation must only ever tidy up after itself.
#[test]
fn never_disturbs_other_worktrees_whose_directories_are_temporarily_missing() {
    let repo = conflicting_repo();
    let elsewhere = repo.add_worktree("left");

    let common_dir = repo
        .path()
        .join(repo.git(&["rev-parse", "--git-common-dir"]));
    let admin_dir = common_dir
        .join("worktrees")
        .join(elsewhere.file_name().expect("worktree directory name"));
    assert!(
        admin_dir.is_dir(),
        "fixture must start with worktree state that could be lost"
    );

    // Stand in for an unmounted volume: the directory is gone, but it is
    // coming back.
    let parked = elsewhere.with_file_name("parked-while-unmounted");
    std::fs::rename(&elsewhere, &parked).expect("park the worktree directory");

    Simulator::new(repo.path(), "main")
        .score(&order(&["left", "right"]))
        .expect("simulation runs");

    assert!(
        admin_dir.is_dir(),
        "simulation deleted an unrelated worktree's administrative state"
    );
    let listed = repo.git(&["worktree", "list"]);
    assert!(
        listed.contains("wt-left"),
        "simulation dropped an unrelated worktree from the repo:\n{listed}"
    );

    // The volume comes back.
    std::fs::rename(&parked, &elsewhere).expect("restore the worktree directory");
    let status = Command::new("git")
        .args([
            "-C",
            elsewhere.to_str().expect("utf-8 worktree path"),
            "status",
        ])
        .output()
        .expect("run git status in the restored worktree");
    assert!(
        status.status.success(),
        "the restored worktree is no longer a working worktree:\n{}",
        String::from_utf8_lossy(&status.stderr)
    );
}
