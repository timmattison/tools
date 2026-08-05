//! What a replay counts when it halts on a conflict.
//!
//! The conflicted halt is the one the replay exists to measure: how many regions
//! a human would have to hand-merge, and which files they are in. Both answers
//! come from git by *path* — git lists the conflicted paths, and the replay reads
//! each file back off disk to count the regions in it — so a file whose name does
//! not survive the trip out of git is counted from a file that is not there.
//!
//! That failure is quiet in the worst way. A file the replay cannot read still
//! costs a human one decision, so the fallback is a plausible number rather than
//! an error, and an undercounted conflict is indistinguishable from an easy one.

use gitscratch::testing::two_region_conflict_in_a_quoted_path_repo;
use gitscratch::{Files, Hunks, Scratch, Stops};

/// Git C-quotes a non-ASCII path when it prints one per line, so the name it
/// reports for a conflicted `café.txt` is `"caf\303\251.txt"` — which names
/// nothing on disk. Read literally it costs the replay both of its answers at
/// once: the hunk count collapses to the one decision an unreadable conflict
/// still costs, however many regions the file really has, and the name the
/// caller is shown is git's escaping rather than the file the developer would
/// have to open.
#[test]
fn counts_every_region_of_a_conflicted_file_git_reports_under_a_quoted_name() {
    let repo = two_region_conflict_in_a_quoted_path_repo();

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", "right"])
        .expect("check out the branch detached in the scratch worktree");

    let cost = scratch
        .replay_rebase("left")
        .expect("replay the contested branch onto the simulated base");

    // The shape first, so the assertions that matter cannot pass by having
    // replayed something other than the one two-region conflict.
    assert_eq!(
        cost.stops(),
        Stops::new(1),
        "both edits arrive in one commit, so the rebase should halt exactly once: {cost:?}"
    );
    assert_eq!(
        cost.files(),
        Files::new(1),
        "one file is contested, so one file should be reported: {cost:?}"
    );

    assert_eq!(
        cost.hunks(),
        Hunks::new(2),
        "the two contested regions are twelve lines apart, so git leaves two conflict markers in \
         the file and a human has two decisions to make - a count of one means the file was never \
         read, because the name it was looked up under was git's escaping of it: {cost:?}"
    );

    assert!(
        cost.file_names().contains("café.txt"),
        "the conflicted file should be named as the developer spelled it, so they can find it in \
         their own repository: {:?}",
        cost.file_names()
    );
}
