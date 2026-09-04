//! What a merge replay actually measures.
//!
//! `tests/conflicts.rs` pins the answer a rebase replay gives. These pin the
//! answer the other operation gives: whether a merge conflicts at all, where
//! its hunks land, and the failure that is neither of those.

use std::path::Path;

use gitscratch::testing::{conflicting_repo, independent_branches_repo};
use gitscratch::{Hunks, Stops};

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

/// **This is the test that fails the day someone drops `--no-ff`.**
///
/// Git fast-forwards a merge whose branch is strictly ahead, and a
/// fast-forward is not a merge at all: git moves HEAD to the other tip and
/// merges no trees. The replay then reports "clean" for an operation it never
/// performed, which is the one answer this crate exists never to give, and it
/// is indistinguishable from the answer a genuinely free merge earns.
///
/// So the assertion is about the operation rather than about the verdict.
/// HEAD standing where it started, with `MERGE_HEAD` set beside it, is what
/// only a real three-way merge leaves behind. A fast-forward leaves the
/// opposite of both: HEAD on the other branch's tip, and no `MERGE_HEAD`.
#[test]
fn a_fast_forwardable_merge_still_runs_a_real_three_way_merge() {
    let repo = independent_branches_repo();
    // `main` is `alpha`'s parent, so git can take this merge as a
    // fast-forward. Nothing else in this file can catch that.
    let scratch = repo.scratch("main");
    let git = scratch.testing_git();

    let before = git
        .run("rev-parse", &["HEAD"])
        .expect("read the commit the scratch worktree starts on");

    scratch
        .replay_merge("alpha")
        .expect("replay a merge git could take as a fast-forward");

    assert_eq!(
        git.run("rev-parse", &["HEAD"])
            .expect("read the commit the scratch worktree ends on"),
        before,
        "a fast-forward moves HEAD to the other branch's tip, so a HEAD that \
         moved means no three-way merge happened"
    );
    assert!(
        git.try_run("rev-parse", &["-q", "--verify", "MERGE_HEAD"])
            .expect("ask git whether a merge is in progress")
            .success,
        "only a real merge records MERGE_HEAD, so its absence means git \
         fast-forwarded instead of merging"
    );
}

/// The other half of the verdict, and the whole of what it is worth reading.
///
/// A tool that only said "conflicts" would tell a developer nothing about the
/// size of the job ahead, so the shape is asserted rather than the boolean: one
/// stop, and a breakdown that names the contested file with the hunks it really
/// contributed. An undercount is the quiet failure here, exactly as
/// `tests/conflicts.rs` describes for a name git cannot print plainly - the
/// count still looks like a plausible answer, so nothing but the count itself
/// catches it.
#[test]
fn a_merge_that_hits_a_contested_region_counts_its_hunks_and_files() {
    let repo = conflicting_repo();
    // `left` and `right` rewrite the same line of `shared.txt`, so git cannot
    // merge them and has to hand the file over with its markers in it.
    let scratch = repo.scratch("left");

    let conflicts = scratch
        .replay_merge("right")
        .expect("replay a merge of a branch that rewrites the same line");

    assert!(
        !conflicts.is_clean(),
        "two branches rewriting the same line should not merge clean, got {conflicts:?}"
    );
    assert_eq!(
        conflicts.stops(),
        Stops::new(1),
        "a merge halts once or not at all, so a merge that conflicted halted \
         exactly once: {conflicts:?}"
    );
    assert_eq!(
        conflicts.file_hunks().collect::<Vec<_>>(),
        vec![(Path::new("shared.txt"), Hunks::new(1))],
        "the one contested file should carry the one region it was contested in"
    );
}
