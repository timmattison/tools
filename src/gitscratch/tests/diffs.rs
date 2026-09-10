//! What a replay captures when a caller asks for the halt diffs.
//!
//! `tests/conflicts.rs` and `tests/merges.rs` pin the counts a replay gives.
//! These pin the halt diffs beside the counts: one for each halt, in halt
//! order, each one the text `git diff` shows at that halt.

use gitscratch::testing::contested_region_repo;
use gitscratch::{Conflicts, HaltDiff, HaltDiffs, Scratch, Stops};

/// The commits of `iterated` in [`contested_region_repo`], in the order a
/// rebase replays them.
const ITERATED_COMMITS: [&str; 3] = ["iterated~2", "iterated~1", "iterated"];

/// Check `branch` out detached in the scratch worktree, the way a consumer
/// does before a rebase replay.
fn check_out(scratch: &Scratch, branch: &str) {
    scratch
        .testing_git()
        .run("checkout", &["-q", "--detach", branch])
        .expect("check out the branch detached in the scratch worktree");
}

/// Replay `branch` onto `onto` through the plain entrance, which captures
/// nothing.
fn replay(scratch: &Scratch, branch: &str, onto: &str) -> Conflicts {
    check_out(scratch, branch);
    scratch
        .replay_rebase(onto)
        .expect("replay the branch onto the simulated base")
}

/// Replay `branch` onto `onto` through the entrance that captures the halt
/// diffs.
fn replay_with_diffs(scratch: &Scratch, branch: &str, onto: &str) -> (Conflicts, HaltDiffs) {
    check_out(scratch, branch);
    scratch
        .replay_rebase_with_diffs(onto)
        .expect("replay the branch onto the simulated base and capture the halt diffs")
}

/// A rebase replay that stops three times captures three halt diffs, in stop
/// order, and each one names the commit it stopped on.
///
/// [`contested_region_repo`] gives `iterated` three commits over one region
/// that `single` already rewrote, so each commit stops the rebase. A halt diff
/// that names the wrong commit, or that comes in the wrong order, puts the
/// diff of one stop under the heading of another.
///
/// Each expected name comes from the runner of the same scratch worktree, with
/// the call the replay makes for `REBASE_HEAD` aimed at a named commit. The
/// two names then take one configuration, so the short id has one length
/// whatever the developer's `core.abbrev` says.
///
/// The capture must change no count. So the `Conflicts` of the replay that
/// captures must equal the `Conflicts` that the plain entrance gives on a
/// fresh copy of the fixture.
#[test]
fn a_rebase_replay_captures_one_halt_diff_per_stop_each_naming_its_stopped_commit() {
    let repo = contested_region_repo();
    let scratch = repo.scratch("main");
    let git = scratch.testing_git();
    let expected: Vec<String> = ITERATED_COMMITS
        .iter()
        .map(|commit| {
            git.run("log", &["-1", "--format=%h %s", commit])
                .expect("name a commit of iterated")
        })
        .collect();

    let (conflicts, diffs) = replay_with_diffs(&scratch, "iterated", "single");

    assert_eq!(
        diffs.iter().map(HaltDiff::stopped).collect::<Vec<_>>(),
        expected
            .iter()
            .map(|name| Some(name.as_str()))
            .collect::<Vec<_>>(),
        "each stop of the replay has to give one halt diff, in stop order, named for the commit \
         the rebase stopped on"
    );
    assert_eq!(
        Stops::new(diffs.len()),
        conflicts.stops(),
        "a replay that captures has one halt diff for each stop it counted"
    );

    let plain = replay(
        &contested_region_repo().scratch("main"),
        "iterated",
        "single",
    );
    assert_eq!(
        conflicts, plain,
        "the capture changed a count: the replay that captures and the plain replay have to \
         measure one fixture the same way"
    );
}
