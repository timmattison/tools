//! What a replay captures when a caller asks for the halt diffs.
//!
//! `tests/conflicts.rs` and `tests/merges.rs` pin the counts a replay gives.
//! These pin the halt diffs beside the counts: one for each halt, in halt
//! order, each one the text `git diff` shows at that halt.

use std::collections::BTreeSet;

use gitscratch::testing::{
    conflicting_repo, contested_region_repo, equal_hunks_unequal_stops_repo,
    independent_branches_repo, modify_delete_repo,
};
use gitscratch::{Conflicts, HaltDiff, HaltDiffs, Scratch, Stops};

/// The commits of `iterated` in [`contested_region_repo`], in the order a
/// rebase replays them.
const ITERATED_COMMITS: [&str; 3] = ["iterated~2", "iterated~1", "iterated"];

/// The start of the line that opens the combined diff of one file. The name
/// of the file follows it.
const COMBINED_DIFF: &str = "diff --cc ";

/// The start of the line git prints for an unmerged file that has no combined
/// diff. The name of the file follows it.
const UNMERGED_PATH: &str = "* Unmerged path ";

/// The line that opens a conflict region in a combined diff. Git labels our
/// side `HEAD` at a rebase stop and at a merge halt alike.
const OPENING_MARKER: &str = "++<<<<<<< HEAD";

/// The start of the line that closes a conflict region in a combined diff.
/// The label of the other side follows it.
const CLOSING_MARKER: &str = "++>>>>>>> ";

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

/// The lines of one halt diff, decoded for an assertion.
///
/// Lossy, because an assertion reads text and every fixture here writes ASCII.
/// The halt diff itself keeps the bytes git wrote.
fn diff_lines(halt: &HaltDiff) -> Vec<String> {
    let diff = halt
        .diff()
        .unwrap_or_else(|message| panic!("git gave no diff at this halt: {message}"));
    String::from_utf8_lossy(diff)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The files one halt diff names.
///
/// A file is named by a `diff --cc <name>` line, which opens the combined diff
/// of a file that git merged in part, or by a `* Unmerged path <name>` line,
/// which git prints for an unmerged file that has no combined diff. Both are
/// read at the start of a line. A content line of a combined diff starts with
/// its prefix columns, each a space, a `+` or a `-`, so no line of file content
/// can read as either of them.
fn named_files(halt: &HaltDiff) -> BTreeSet<String> {
    diff_lines(halt)
        .iter()
        .filter_map(|line| {
            line.strip_prefix(COMBINED_DIFF)
                .or_else(|| line.strip_prefix(UNMERGED_PATH))
                .map(str::to_owned)
        })
        .collect()
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

/// Each halt diff holds the conflict markers of the region its own stop
/// conflicted in.
///
/// The replay stages the markers with `git add -A` before it continues, and
/// after that line `git diff` shows nothing for the stop. So a capture below
/// that line gives an empty halt diff at every stop, and this is the test that
/// fails when the capture moves there.
///
/// The closing marker says whose region it is. Git labels it with the short
/// id and the subject of the stopped commit, so the halt diff of the second
/// stop holds `++>>>>>>> <id> (iterate 2)`. The markers that an earlier stop
/// staged come back at a later stop as content, with one `+`, so a marker line
/// with two is always the region of the stop that the halt diff belongs to.
#[test]
fn each_halt_diff_holds_the_markers_of_its_own_region() {
    let repo = contested_region_repo();
    let scratch = repo.scratch("main");
    let git = scratch.testing_git();
    let closing: Vec<String> = ITERATED_COMMITS
        .iter()
        .map(|commit| {
            let label = git
                .run("log", &["-1", "--format=%h (%s)", commit])
                .expect("label a commit of iterated the way git labels a closing marker");
            format!("{CLOSING_MARKER}{label}")
        })
        .collect();

    let (_, diffs) = replay_with_diffs(&scratch, "iterated", "single");

    assert_eq!(
        diffs.len(),
        closing.len(),
        "the replay stops once for each commit of iterated, so it has to give one halt diff for \
         each, or the assertions below look at fewer stops than there are: {diffs:?}"
    );
    for (stop, (halt, closing)) in (1..).zip(diffs.iter().zip(&closing)) {
        let lines = diff_lines(halt);
        assert!(
            lines.iter().any(|line| line == OPENING_MARKER),
            "the halt diff of stop {stop} has no `{OPENING_MARKER}` line, so it does not show the \
             region that stop conflicted in: {lines:#?}"
        );
        assert!(
            lines.contains(closing),
            "the halt diff of stop {stop} has no `{closing}` line, so the region it shows is not \
             the one its own stopped commit conflicted in: {lines:#?}"
        );
    }
}

/// Each halt diff names the files the breakdown counted at its stop, and no
/// other file.
///
/// A halt diff that names a file the counter did not read, or leaves out a
/// file the counter read, tells the reader a different story from the
/// breakdown above it. `two` edits `x.txt` and `y.txt` in two commits, and
/// `one` edits both, so the first stop conflicts in `x.txt` alone and the
/// second stop in `y.txt` alone. A capture that reads more than the conflicted
/// files, or that keeps the files of an earlier stop, names both files at one
/// stop.
#[test]
fn each_halt_diff_names_the_files_the_breakdown_counted_at_its_stop() {
    let repo = equal_hunks_unequal_stops_repo();
    let scratch = repo.scratch("main");

    let (conflicts, diffs) = replay_with_diffs(&scratch, "two", "one");

    let named: Vec<BTreeSet<String>> = diffs.iter().map(named_files).collect();
    assert_eq!(
        named,
        vec![
            BTreeSet::from(["x.txt".to_owned()]),
            BTreeSet::from(["y.txt".to_owned()]),
        ],
        "the first stop conflicts in x.txt alone and the second in y.txt alone, so each halt \
         diff has to name that one file"
    );

    let counted: BTreeSet<String> = conflicts
        .file_hunks()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        named.into_iter().flatten().collect::<BTreeSet<_>>(),
        counted,
        "the halt diffs together have to name the files the breakdown counted, and no other file"
    );
}

/// A file that git has no combined diff for is named as an unmerged path.
///
/// [`modify_delete_repo`] gives `branch` a change to `x.txt`, and `main`
/// deletes that file. Git has no combined diff for a file that one side
/// deleted, so at that stop `git diff` prints `* Unmerged path x.txt` and no
/// hunk. The halt diff has to hold that line. If not, the reader gets a stop
/// that the breakdown counts and that the diff says nothing about.
#[test]
fn a_modify_delete_halt_diff_names_the_file_as_an_unmerged_path() {
    let repo = modify_delete_repo();
    let scratch = repo.scratch("main");

    let (conflicts, diffs) = replay_with_diffs(&scratch, "branch", "main");

    assert_eq!(
        conflicts
            .file_hunks()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["x.txt".to_owned()],
        "the fixture has to stop on a conflict in x.txt, or there is nothing here to name"
    );
    assert_eq!(
        diffs.len(),
        1,
        "one commit stops the rebase once, so the replay has to give one halt diff: {diffs:?}"
    );

    let unmerged = format!("{UNMERGED_PATH}x.txt");
    let lines = diffs.iter().flat_map(diff_lines).collect::<Vec<_>>();
    assert!(
        lines.contains(&unmerged),
        "a file that one side deleted has no combined diff, so the halt diff has to name it with \
         `{unmerged}`: {lines:#?}"
    );
}

/// A merge replay that conflicts captures one halt diff, and that halt diff
/// names no stopped commit.
///
/// A merge makes one three-way merge and stops at it, so it halts once or not
/// at all. [`conflicting_repo`] gives `left` and `right` one edit each to the
/// same line of `shared.txt`, so the merge halts once. The halt diff of that
/// halt is the combined diff of `shared.txt`, with the markers in it. A merge
/// has no stopped commit, so the name is `None`.
///
/// The capture must change no count here either. So the `Conflicts` of the
/// merge that captures must equal the `Conflicts` that the plain entrance
/// gives on a fresh copy of the fixture.
#[test]
fn a_merge_replay_captures_one_halt_diff_that_names_no_stopped_commit() {
    let repo = conflicting_repo();
    let scratch = repo.scratch("left");

    let (conflicts, diffs) = scratch
        .replay_merge_with_diffs("right")
        .expect("replay a merge of a branch that rewrites the same line and capture its halt diff");

    assert_eq!(
        diffs.len(),
        1,
        "a merge halts once or not at all, so a merge that conflicted has to give one halt diff: \
         {diffs:?}"
    );
    assert_eq!(
        Stops::new(diffs.len()),
        conflicts.stops(),
        "a merge that captures has one halt diff for each stop it counted"
    );

    let halt = diffs
        .iter()
        .next()
        .expect("the one halt diff counted above");
    assert_eq!(
        halt.stopped(),
        None,
        "a merge has no stopped commit, so its halt diff names none"
    );
    let lines = diff_lines(halt);
    let header = format!("{COMBINED_DIFF}shared.txt");
    assert!(
        lines.contains(&header),
        "the merge conflicted in shared.txt, so the halt diff has to open with `{header}`: \
         {lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line == OPENING_MARKER),
        "the halt diff has to show the region the merge conflicted in, from its \
         `{OPENING_MARKER}` line: {lines:#?}"
    );

    let plain = conflicting_repo()
        .scratch("left")
        .replay_merge("right")
        .expect("replay the same merge through the plain entrance");
    assert_eq!(
        conflicts, plain,
        "the capture changed a count: the merge that captures and the plain merge have to measure \
         one fixture the same way"
    );
}

/// A replay that does not halt captures no halt diff, through either entrance.
///
/// A halt diff is the text of a halt, and a clean replay has no halt. A
/// capture that runs where every replay passes, and not at a halt, gives a
/// clean replay a halt diff of nothing. A reader then gets a section for a
/// stop that did not happen. `alpha` and `beta` in
/// [`independent_branches_repo`] each add a file of their own, so neither the
/// rebase nor the merge halts.
///
/// The merge half is the one that can go wrong. A merge that git completes
/// returns early, and a capture above that return runs for a clean merge too.
#[test]
fn a_replay_that_does_not_halt_captures_no_halt_diff() {
    let repo = independent_branches_repo();

    let (rebased, rebase_diffs) = replay_with_diffs(&repo.scratch("main"), "alpha", "beta");
    assert!(
        rebased.is_clean(),
        "the fixture has to replay clean, or there is a halt here to capture: {rebased:?}"
    );
    assert!(
        rebase_diffs.is_empty(),
        "a rebase that did not stop has no halt, so it has no halt diff: {rebase_diffs:?}"
    );

    let (merged, merge_diffs) = repo
        .scratch("alpha")
        .replay_merge_with_diffs("beta")
        .expect("replay a merge of a branch that touches other files");
    assert!(
        merged.is_clean(),
        "the fixture has to merge clean, or there is a halt here to capture: {merged:?}"
    );
    assert!(
        merge_diffs.is_empty(),
        "a merge that git completed has no halt, so it has no halt diff: {merge_diffs:?}"
    );
}
