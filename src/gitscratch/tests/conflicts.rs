//! What a replay actually measures.
//!
//! `tests/safety.rs` pins the properties that make running against a real
//! repository acceptable. These pin the answer itself: whether a replay
//! conflicted at all, and where the hunks landed.

use gitscratch::testing::{conflicting_repo, independent_branches_repo};
use gitscratch::{Conflicts, Scratch};

/// Replay `branch` onto `onto` the way a consumer does: check it out detached
/// in the scratch worktree, then rebase.
fn replay(scratch: &Scratch, branch: &str, onto: &str) -> Conflicts {
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", branch])
        .expect("check out the branch detached in the scratch worktree");
    scratch
        .replay_rebase(onto)
        .expect("replay the branch onto the simulated base")
}

/// The whole point of the tools built on this crate is a yes-or-no verdict, so
/// "did anything conflict" has to be answerable without a caller re-deriving it
/// from three counts and getting the edge cases wrong.
#[test]
fn a_replay_that_conflicts_with_nothing_is_clean() {
    let repo = independent_branches_repo();
    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");

    let conflicts = replay(&scratch, "alpha", "beta");

    assert!(
        conflicts.is_clean(),
        "two branches that touch different files should replay clean, got {conflicts:?}"
    );
}

#[test]
fn a_replay_that_hits_a_contested_region_is_not_clean() {
    let repo = conflicting_repo();
    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");

    replay(&scratch, "left", "main");
    let conflicts = replay(&scratch, "right", "left");

    assert!(
        !conflicts.is_clean(),
        "two branches rewriting the same line should not replay clean, got {conflicts:?}"
    );
}
