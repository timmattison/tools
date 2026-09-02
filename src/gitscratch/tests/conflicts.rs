//! What a replay actually measures.
//!
//! `tests/safety.rs` pins the properties that make running against a real
//! repository acceptable. These pin the answer itself: whether a replay
//! conflicted at all, and where the hunks landed.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use gitscratch::testing::awkward_names_repo;
use gitscratch::testing::{
    conflicting_repo, contested_region_repo, equal_hunks_unequal_stops_repo,
    independent_branches_repo, multi_byte_names_repo,
};
use gitscratch::{Conflicts, Hunks, Scratch, Stops};

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

/// One entry of a hand-built per-file breakdown.
///
/// `Conflicts::from_files` takes a [`NonZeroUsize`] per file, because a file
/// that conflicted cost at least one decision, so every fixture wraps its count
/// here rather than at each call site.
fn file(name: &str, hunks: usize) -> (PathBuf, NonZeroUsize) {
    (
        PathBuf::from(name),
        NonZeroUsize::new(hunks).expect("a conflicted file contributes at least one hunk"),
    )
}

/// The whole point of the tools built on this crate is a yes-or-no verdict, so
/// "did anything conflict" has to be answerable without a caller re-deriving it
/// from three counts and getting the edge cases wrong.
#[test]
fn a_replay_that_conflicts_with_nothing_is_clean() {
    let repo = independent_branches_repo();
    let scratch = repo.scratch("main");

    let conflicts = replay(&scratch, "alpha", "beta");

    assert!(
        conflicts.is_clean(),
        "two branches that touch different files should replay clean, got {conflicts:?}"
    );
}

#[test]
fn a_replay_that_hits_a_contested_region_is_not_clean() {
    let repo = conflicting_repo();
    let scratch = repo.scratch("main");

    replay(&scratch, "left", "main");
    let conflicts = replay(&scratch, "right", "left");

    assert!(
        !conflicts.is_clean(),
        "two branches rewriting the same line should not replay clean, got {conflicts:?}"
    );
}

/// "4 hunks across 2 files" tells a developer how much work is coming but not
/// where it lands, so the replay has to remember which file each hunk belonged
/// to rather than only the running total.
#[test]
fn the_breakdown_says_which_file_each_hunk_belonged_to() {
    let repo = equal_hunks_unequal_stops_repo();
    let scratch = repo.scratch("main");

    // `two` splits its two edits across two commits, so the replay halts once
    // per file and each stop contributes to a different name.
    let conflicts = replay(&scratch, "two", "one");

    assert_eq!(
        conflicts.file_hunks().collect::<Vec<_>>(),
        vec![
            (Path::new("x.txt"), Hunks::new(1)),
            (Path::new("y.txt"), Hunks::new(1))
        ],
        "each contested file should carry its own hunk count"
    );
}

/// A file that conflicts at several stops has to accumulate against that one
/// name, not be counted once and then forgotten - otherwise the breakdown would
/// disagree with the total it is supposed to explain.
#[test]
fn a_file_that_conflicts_repeatedly_accumulates_against_its_own_name() {
    let repo = contested_region_repo();
    let scratch = repo.scratch("main");

    // `iterated` rewrites the same region in three separate commits, so every
    // one of them collides with the single edit already on the base.
    let conflicts = replay(&scratch, "iterated", "single");

    let breakdown = conflicts.file_hunks().collect::<Vec<_>>();
    assert_eq!(
        breakdown.len(),
        1,
        "only one file was ever contested, got {breakdown:?}"
    );
    assert_eq!(breakdown[0].0, Path::new("shared.txt"));
    assert!(
        breakdown[0].1 > Hunks::new(1),
        "three colliding commits should leave more than one hunk on the file, got {breakdown:?}"
    );
    assert_eq!(
        breakdown[0].1,
        conflicts.hunks(),
        "the breakdown has to add up to the total it explains"
    );
}

/// A file name outside ASCII has to come back out of a replay as the developer
/// typed it, carrying the hunks it really contributed.
///
/// Both halves break together, which is why both are asserted here. Git's
/// default `core.quotePath` hands `git diff --name-only` a C-quoted,
/// octal-escaped path, so the breakdown reports a name nobody typed *and* the
/// count collapses: the escaped name resolves to no file on disk, and a
/// conflicted file that cannot be read is floored at a single hunk. The second
/// failure is the quiet one - it looks like a plausible answer.
///
/// `日本語.txt` is contested in two regions precisely so that undercount is
/// visible. With one region the swallowed answer and the true answer would both
/// be 1, and the defect would pass this test.
#[test]
fn a_conflicted_non_ascii_path_keeps_its_real_name_and_its_real_hunk_count() {
    let repo = multi_byte_names_repo();
    let scratch = repo.scratch("main");

    let conflicts = replay(&scratch, "right-右", "left-左");

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

/// The classes of awkward name `core.quotePath=false` does *not* rescue have to
/// survive the round trip too, by name and by count.
///
/// Two separate mechanisms wreck them, and the fixture holds both. Git C-quotes
/// a path containing a backslash or a double quote whatever `core.quotePath`
/// says, so those two arrive wrapped in quotes git added, naming no file on
/// disk. The three whitespace-edged names arrive from git perfectly intact and
/// are destroyed by the reader instead - by a trim, which is Unicode-aware and
/// so eats an IDEOGRAPHIC SPACE as readily as an ASCII one. Both failures end in
/// the same place: the name cannot be opened, and a conflicted file that cannot
/// be opened is floored at one hunk.
///
/// So the hunk counts are the assertion that matters. Every file here is
/// genuinely contested in two regions; a mangled name reports 1, and the total
/// still looks like a plausible answer. Asserting the whole breakdown at once
/// catches the name and the undercount together, which is right, because they
/// are the same defect seen from two sides.
///
/// Unix-only: `awkward_names_repo` cannot be built on a filesystem that rejects
/// `"` or `\` in a name.
#[cfg(unix)]
#[test]
fn a_conflicted_path_git_cannot_print_plainly_keeps_its_name_and_its_hunk_count() {
    let repo = awkward_names_repo();
    let scratch = repo.scratch("main");

    let conflicts = replay(&scratch, "right", "left");

    assert_eq!(
        conflicts.file_hunks().collect::<Vec<_>>(),
        vec![
            (Path::new(" lead.txt"), Hunks::new(2)),
            (Path::new("back\\slash.txt"), Hunks::new(2)),
            (Path::new("plain.txt"), Hunks::new(2)),
            (Path::new("quo\"te.txt"), Hunks::new(2)),
            (Path::new("trail.txt "), Hunks::new(2)),
            (Path::new("\u{3000}wide.txt "), Hunks::new(2)),
        ],
        "a path git quotes, or one a trim would erode, must survive the round \
         trip through git by name and by count"
    );
}

/// Each ordering `grist` scores is a sequence of replays folded together, so
/// the fold has to merge the breakdowns rather than let a later step's count
/// for a file replace an earlier one's.
#[test]
fn absorbing_a_step_folds_its_breakdown_into_the_running_total() {
    let mut total = Conflicts::from_files(
        [file("src/lib.rs", 3), file("src/main.rs", 1)],
        Stops::new(2),
    );
    total.absorb(Conflicts::from_files(
        [file("src/lib.rs", 2), file("README.md", 4)],
        Stops::new(1),
    ));

    assert_eq!(
        total.file_hunks().collect::<Vec<_>>(),
        vec![
            (Path::new("README.md"), Hunks::new(4)),
            (Path::new("src/lib.rs"), Hunks::new(5)),
            (Path::new("src/main.rs"), Hunks::new(1))
        ],
        "a file hit by both steps should carry the sum of the two"
    );
    assert_eq!(total.hunks(), Hunks::new(10));
}
