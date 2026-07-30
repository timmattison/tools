//! `gitscratch` runs against the developer's real repository. These tests pin
//! the properties that make that acceptable.

use std::process::Command;

use gitscratch::testing::conflicting_repo;
use gitscratch::{Conflicts, Files, Hunks, Scratch};

/// Replay `branch` onto `onto` the way a consumer does: check it out detached
/// in the scratch worktree, then rebase.
///
/// Detaching is not incidental — it is the guard test 2 exercises, which is why
/// it is spelled out here in the test rather than hidden behind a library call.
fn replay(scratch: &Scratch, branch: &str, onto: &str) -> Conflicts {
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", branch])
        .expect("check out the branch detached in the scratch worktree");
    scratch
        .replay_rebase(onto)
        .expect("replay the branch onto the simulated base")
}

/// `rebase.updateRefs` rewrites any branch pointing into the range being
/// replayed - which is exactly the branch under replay. A developer who has
/// turned it on must not lose their branches to a dry run.
#[test]
fn never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled() {
    let repo = conflicting_repo();
    repo.git(&["config", "rebase.updateRefs", "true"]);

    let before: Vec<(String, String)> = ["main", "left", "right"]
        .iter()
        .map(|name| ((*name).to_string(), repo.rev_parse(name)))
        .collect();

    // Scoped so the scratch is torn down before the refs are re-read: teardown
    // is part of what must not move a branch.
    {
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
        replay(&scratch, "left", "main");
        // `right` onto `left` is the replay that genuinely conflicts, and the
        // replayed range is what `rebase.updateRefs` would rewrite.
        replay(&scratch, "right", "left");
    }

    for (name, sha) in before {
        assert_eq!(repo.rev_parse(&name), sha, "replay moved branch '{name}'");
    }
}

/// The branches worth comparing are usually the ones already checked out in
/// other worktrees - which is exactly the situation where a plain `git checkout`
/// refuses to run. A replay must detach instead.
#[test]
fn works_when_the_branches_are_checked_out_in_other_worktrees() {
    let repo = conflicting_repo();
    let _left = repo.add_worktree("left");
    let _right = repo.add_worktree("right");

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    replay(&scratch, "left", "main");
    let conflicts = replay(&scratch, "right", "left");

    // Asserting on the conflict the replay had to resolve, so this cannot pass
    // by having quietly replayed nothing at all.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_names().any(|name| name == "shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names().collect::<Vec<_>>()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );
}

/// A worktree's directory can be temporarily unreachable while the worktree
/// itself is perfectly alive - an external drive unmounted, a network mount
/// asleep, a directory moved aside for a minute. Everything that makes it
/// recoverable, including any halted rebase, lives in the real repository under
/// `.git/worktrees/`. Repo-wide cleanup deletes that state on sight and with no
/// grace period, so a replay must only ever tidy up after itself.
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

    // Scoped on purpose: this test exists to pin what `Scratch`'s teardown does
    // and does not do, so the drop must have run before anything below is
    // asserted. Do not flatten this block away.
    {
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
        replay(&scratch, "left", "main");
        replay(&scratch, "right", "left");
    }

    assert!(
        admin_dir.is_dir(),
        "replay deleted an unrelated worktree's administrative state"
    );
    let listed = repo.git(&["worktree", "list"]);
    assert!(
        listed.contains("wt-left"),
        "replay dropped an unrelated worktree from the repo:\n{listed}"
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
