//! What a merge replay actually measures.
//!
//! `tests/conflicts.rs` pins the answer a rebase replay gives. These pin the
//! answer the other operation gives: whether a merge conflicts at all, where
//! its hunks land, and the failure that is neither of those.

use gitscratch::testing::independent_branches_repo;

/// The verdict every tool built on this crate prints is "clean" or
/// "conflicts", so a merge that has nothing to argue about has to come back
/// clean. This is the first half of that verdict, and the fixture makes it a
/// real three-way merge: `alpha` and `beta` are siblings off `main`, so git
/// merges two divergent histories rather than moving a pointer.
#[test]
fn a_merge_that_conflicts_with_nothing_is_clean() {
    let repo = independent_branches_repo();
    let scratch = repo.scratch("alpha");

    let conflicts = scratch
        .replay_merge("beta")
        .expect("replay a merge of a branch that touches other files");

    assert!(
        conflicts.is_clean(),
        "two branches that each add a file of their own should merge clean, got {conflicts:?}"
    );
}
