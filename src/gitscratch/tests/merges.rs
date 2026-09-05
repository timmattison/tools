//! What a merge replay actually measures.
//!
//! `tests/conflicts.rs` pins the answer a rebase replay gives. These pin the
//! answer the other operation gives: whether a merge conflicts at all, where
//! its hunks land, and the failure that is neither of those.

use std::path::Path;

use gitscratch::testing::{
    conflicting_repo, independent_branches_repo, multi_byte_names_repo, unrelated_histories_repo,
};
use gitscratch::{Files, Hunks, Stops};

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

/// A merge git will not perform is neither clean nor conflicting, and saying
/// so is the whole job of this test.
///
/// Two histories with no commit in common give a developer nothing to resolve,
/// because git never merged the trees at all. Reporting that as clean says the
/// merge is free when git refuses to do it; reporting it as a conflict invents
/// work nobody can do. Only an error says what happened.
///
/// The error has to carry git's own sentence, because that sentence is the one
/// part of the answer that says which of the several ways a merge can be
/// refused this was. A bare "the merge failed" sends the reader back to the
/// terminal to run the command by hand.
#[test]
fn a_merge_git_refuses_outright_is_an_error_rather_than_a_verdict() {
    let repo = unrelated_histories_repo();
    let scratch = repo.scratch("main");

    let refusal = scratch
        .replay_merge("unrelated")
        .expect_err("git refuses to merge two histories with no commit in common");

    let words = format!("{refusal:#}");
    assert!(
        words.contains("refusing to merge unrelated histories"),
        "the error has to carry git's own account of the refusal, got: {words}"
    );
}

/// A file name outside ASCII has to come back out of a merge replay as the
/// developer typed it, carrying the hunks it really contributed.
///
/// Both halves break together, which is why both are asserted here. Git's
/// default `core.quotePath` hands `git diff --name-only` a C-quoted,
/// octal-escaped path, so the breakdown reports a name nobody typed *and* the
/// count collapses: the escaped name resolves to no file on disk, and a
/// conflicted file that cannot be read is floored at a single hunk. The second
/// failure is the quiet one - it looks like a plausible answer.
///
/// `日本語.txt` is contested in two regions precisely so that undercount is
/// visible. With one region the swallowed answer and the true answer would
/// both be 1, and the defect would pass this test.
///
/// The branch names carry multi-byte characters for the same reason: a branch
/// name travels into the merge as an argument and back out in the verdict, so
/// it takes the same road a file name does.
#[test]
fn a_conflicted_non_ascii_path_survives_a_merge_by_name_and_by_count() {
    let repo = multi_byte_names_repo();
    let scratch = repo.scratch("left-左");

    let conflicts = scratch
        .replay_merge("right-右")
        .expect("replay a merge of a branch that rewrites both files");

    // The shape first, so the breakdown below cannot pass by having replayed
    // something other than the collision this fixture is built to produce.
    assert_eq!(
        conflicts.stops(),
        Stops::new(1),
        "a merge halts once or not at all, so a merge that conflicted halted \
         exactly once: {conflicts:?}"
    );
    assert_eq!(
        conflicts.files(),
        Files::new(2),
        "both files are contested, so both should be reported: {conflicts:?}"
    );

    assert_eq!(
        conflicts.file_hunks().collect::<Vec<_>>(),
        vec![
            (Path::new("readme.md"), Hunks::new(1)),
            (Path::new("日本語.txt"), Hunks::new(2))
        ],
        "a non-ASCII path must survive the round trip through git by name and \
         by count"
    );
}

/// A branch name that starts with a dash is a branch name, and the merge that
/// measures it has to read it as one.
///
/// `git merge --no-commit --no-ff --allow-unrelated-histories` is a complete
/// and valid command. Git reads the name as an option of its own, is left with
/// no branch to merge at all, and falls back to the upstream of the current
/// branch - a merge of something nobody named. A scratch worktree stands on a
/// detached HEAD, so there is no current branch to take an upstream from, and
/// git stops with `fatal: No current branch.`
///
/// So the assertion is on the words of the refusal rather than on the fact of
/// one. Both spellings fail, which is what makes the fact of a failure worth
/// nothing here. Only the separated one names the branch git would not merge;
/// the other blames the worktree this crate built, which is machinery the
/// caller never asked for and cannot correct. A name the message never carries
/// is a name nobody can repair.
///
/// `--allow-unrelated-histories` rather than `--abort`, `--quit` or `--squash`:
/// git refuses those three by name - `fatal: --abort expects no arguments` -
/// so a test built on one of them reads its own argument back out of git's
/// complaint and passes with the separator gone.
///
/// The control at the end merges a branch the fixture really has, on the same
/// scratch worktree, because a replay that refused every branch would pass the
/// assertion above and measure nothing at all.
#[test]
fn refuses_a_branch_that_starts_with_a_dash_by_name_rather_than_blaming_the_worktree() {
    let repo = conflicting_repo();
    let scratch = repo.scratch("left");

    let error = scratch
        .replay_merge("--allow-unrelated-histories")
        .map(|cost| format!("{cost:?}"))
        .expect_err(
            "a branch that names no commit has to stop the replay. Git knows \
             `--allow-unrelated-histories` as an option of `merge`, so reading it as one leaves \
             git with no branch to merge and a complaint about the worktree the replay stands in",
        );

    let message = format!("{error:#}");
    assert!(
        message.contains("--allow-unrelated-histories"),
        "the refusal has to name the branch git would not merge, or the caller is told about a \
         detached HEAD it never asked for and never hears the name it typed: {message}"
    );

    let control = scratch
        .replay_merge("right")
        .expect("replay a merge of a branch the fixture really has");

    assert_eq!(
        control.stops(),
        Stops::new(1),
        "the fixture has to conflict, or the refusal above proves only that this replay answers \
         nothing at all: {control:?}"
    );
}
