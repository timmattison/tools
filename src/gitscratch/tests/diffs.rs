//! What a replay captures when a caller asks for the halt diffs.
//!
//! `tests/conflicts.rs` and `tests/merges.rs` pin the counts a replay gives.
//! These pin the halt diffs beside the counts: one for each halt, in halt
//! order, each one the text `git diff` shows at that halt.
//!
//! The last tests pin what a developer's configuration can change in that
//! text. The developer's global configuration reaches the runner, so without a
//! pin the halt diff of one fixture is different bytes on different machines.
//! Each of those tests sets one hostile setting in its fixture's own
//! configuration, shows with plain git that the setting changes the diff at a
//! real halt, and then asserts that the halt diff does not change.

use std::collections::BTreeSet;
use std::process::{Command, Output};

use gitscratch::testing::{
    conflicting_repo, contested_region_repo, equal_hunks_unequal_stops_repo,
    independent_branches_repo, modify_delete_repo, TestRepo,
};
use gitscratch::{Conflicts, HaltDiff, HaltDiffs, Scratch, Stops};
use tempfile::TempDir;

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

/// The branch a merge replay of [`conflicting_repo`] stands on.
const OURS: &str = "left";

/// The branch a merge replay of [`conflicting_repo`] merges. It rewrites the
/// line that [`OURS`] rewrites, so the merge halts once.
const THEIRS: &str = "right";

/// The branch [`conflicting_repo`] leaves checked out in the fixture itself.
const FIXTURE_BRANCH: &str = "main";

/// The byte that starts each color code a terminal reads.
const ESC: u8 = 0x1b;

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

/// [`conflicting_repo`] with `key` set to `value` in the fixture's own
/// configuration.
///
/// The fixture holds the hostile setting itself, as
/// `branches_behind_main_with_a_submodule_pointer_bump_repo` does for
/// `diff.ignoreSubmodules`. So the hazard is live on each machine, and on no
/// machine by accident. A local value wins over a global one, so a developer
/// who sets the key gets the same fixture as a developer who does not.
///
/// The value is read back before the fixture goes to a test. That is the
/// start-state control: a key that the fixture did not take is a key that no
/// pin can be shown to override.
fn conflicting_repo_with(key: &str, value: &str) -> TestRepo {
    let repo = conflicting_repo();
    repo.git(&["config", key, value]);
    assert_eq!(
        repo.git(&["config", "--get", key]),
        value,
        "the fixture does not hold `{key}={value}`, so there is nothing here for a pin to \
         override and the assertions below are measured against nothing"
    );
    repo
}

/// The branch the fixture has checked out, the commit it stands on, and what
/// `git status` says about its working tree.
///
/// Read before and after the armed control, so that the control can show it
/// puts the fixture back where it found it.
fn fixture_state(repo: &TestRepo) -> (String, String, String) {
    (
        repo.git(&["symbolic-ref", "--short", "HEAD"]),
        repo.git(&["rev-parse", "HEAD"]),
        repo.git(&["status", "--porcelain"]),
    )
}

/// What plain git shows at a real halt of the merge that the capture reads:
/// the output of `git diff <args>`, run in the fixture itself.
///
/// The armed control of each pin test below. A pin test ends in an assertion
/// that the halt diff did not change, and that assertion also passes when the
/// setting under test changes nothing. So each test first reads plain git at a
/// real halt and asserts that the hazard is live there. On the day git stops
/// acting on a setting, the control fails and says so, and the test does not
/// pass for a reason that has nothing to do with its pin.
///
/// The halt is the one the merge replay makes: the fixture checks out [`OURS`]
/// and merges [`THEIRS`] with `--no-commit --no-ff`. `--no-ff` also answers a
/// developer's `merge.ff=only`, which refuses a merge of two branches that
/// diverge. Then the control aborts the merge, checks out [`FIXTURE_BRANCH`]
/// again, and asserts that the fixture is back where it started. The replay
/// that follows reads the fixture, and it must not read what the control left
/// behind.
///
/// Each spawn goes through [`TestRepo::git`] or [`TestRepo::try_git`], and
/// both remove the inherited git environment. The diff call runs under
/// `LC_ALL=C`, so a control that reads git's own words reads them in one
/// language.
///
/// A control that reads the text of the diff puts `--no-color` in `args`. A
/// `color.ui=always` in the global configuration of the developer then cannot
/// put codes between the control and the text it reads. The control of the
/// color test puts no such flag in `args`, because the codes are what it reads.
fn plain_diff_at_a_real_halt(repo: &TestRepo, args: &[&str]) -> Output {
    let before = fixture_state(repo);

    repo.checkout(OURS);
    let merged = repo.try_git(&["merge", "--no-commit", "--no-ff", THEIRS], &[]);
    let conflicted = repo.git(&["diff", "--name-only", "--diff-filter=U"]);
    assert!(
        !merged.status.success() && !conflicted.is_empty(),
        "plain git did not halt on a conflict when it merged {THEIRS} into {OURS}, so there is \
         no halt here to read and this control could only pass vacuously: {}",
        String::from_utf8_lossy(&merged.stderr)
    );

    let diff_call: Vec<&str> = std::iter::once("diff")
        .chain(args.iter().copied())
        .collect();
    let diff = repo.try_git(&diff_call, &[("LC_ALL", "C")]);

    repo.git(&["merge", "--abort"]);
    repo.checkout(FIXTURE_BRANCH);
    assert_eq!(
        fixture_state(repo),
        before,
        "the control did not put the fixture back where it started, so the replay below reads \
         what the control left behind"
    );

    diff
}

/// Replay the merge of [`THEIRS`] into [`OURS`] in a scratch worktree of
/// `repo`, through the entrance that captures, and give back the counts and
/// the one halt diff.
fn merge_with_halt_diff(repo: &TestRepo) -> (Conflicts, HaltDiff) {
    let (conflicts, diffs) = repo
        .scratch(OURS)
        .replay_merge_with_diffs(THEIRS)
        .expect("replay a merge of a branch that rewrites the same line and capture its halt diff");
    assert_eq!(
        diffs.len(),
        1,
        "a merge halts once or not at all, and this merge conflicts, so it has to give one halt \
         diff: {diffs:?}"
    );
    let halt = diffs
        .iter()
        .next()
        .cloned()
        .expect("the one halt diff counted above");
    (conflicts, halt)
}

/// A halt diff holds no color code, whatever the color settings of the
/// developer say.
///
/// `color.ui=always` and `color.diff=always` make git write color codes into
/// the output of `git diff`, although git writes to a pipe. A renderer that
/// prints the halt diff cannot tell such a code from an ESC byte in the file.
/// It escapes both, and the reader then sees `\u{1b}[1m` in place of a color.
/// The renderer paints the diff itself, so the capture must stay plain. Each
/// setting gets a fixture of its own, because each one turns the codes on
/// alone.
#[test]
fn a_halt_diff_holds_no_color_code_whatever_the_color_settings_say() {
    for (key, value) in [("color.ui", "always"), ("color.diff", "always")] {
        let repo = conflicting_repo_with(key, value);

        let plain = plain_diff_at_a_real_halt(&repo, &["--diff-filter=U"]);
        assert!(
            plain.stdout.contains(&ESC),
            "`{key}={value}` puts no color code into plain `git diff` through a pipe, so this \
             test could only pass vacuously: {}",
            String::from_utf8_lossy(&plain.stdout)
        );

        let (_, halt) = merge_with_halt_diff(&repo);
        let diff = halt
            .diff()
            .unwrap_or_else(|message| panic!("git gave no diff at the halt: {message}"));
        assert!(
            !diff.contains(&ESC),
            "under `{key}={value}` the halt diff holds color codes, and a renderer prints them as \
             escaped text: {}",
            String::from_utf8_lossy(diff).escape_debug()
        );
        assert!(
            diff_lines(&halt).iter().any(|line| line == OPENING_MARKER),
            "under `{key}={value}` the halt diff has to show the region the merge conflicted \
             in, from its `{OPENING_MARKER}` line: {}",
            String::from_utf8_lossy(diff)
        );
    }
}

/// [`conflicting_repo`] with a diff driver whose textconv program fails, and a
/// committed `.gitattributes` on [`OURS`] that selects that driver for each
/// `.txt` file.
///
/// `diff.hostile.textconv=false` names the program `false`, which exits 1.
/// Git runs the textconv program of a driver on each side of a diff of a file
/// that selects the driver. The attributes file is on the branch the merge
/// replay stands on, so it is in the working tree of the scratch worktree.
/// [`THEIRS`] does not touch it, so it merges clean, and the halt names
/// `shared.txt` alone.
fn textconv_repo() -> TestRepo {
    /// The name of the diff driver that the attributes file selects.
    const DRIVER: &str = "hostile";

    let repo = conflicting_repo_with(&format!("diff.{DRIVER}.textconv"), "false");
    repo.checkout(OURS);
    repo.commit_file(
        ".gitattributes",
        &format!("*.txt diff={DRIVER}\n"),
        "select a diff driver whose textconv program fails",
    );
    repo.checkout(FIXTURE_BRANCH);
    repo
}

/// A halt diff is not lost to a textconv program that fails.
///
/// A `.gitattributes` entry can select a diff driver, and
/// `diff.<driver>.textconv` names a program that git runs on each side of the
/// diff before it compares them. When the program fails, `git diff` stops
/// with `fatal: unable to read files to diff`. The capture then holds that
/// error in place of a diff, and the reader gets no diff for a halt that git
/// can show. The capture must also never run a program from the configuration
/// of the developer.
///
/// The count must not depend on the diff. So the `Conflicts` of the merge that
/// captures must equal the `Conflicts` that the plain entrance gives on a
/// fresh copy of the fixture.
#[test]
fn a_halt_diff_is_not_lost_to_a_textconv_program_that_fails() {
    /// What git says when a textconv program fails, under `LC_ALL=C`.
    const UNREADABLE: &str = "unable to read files to diff";

    let repo = textconv_repo();

    let plain = plain_diff_at_a_real_halt(&repo, &["--diff-filter=U"]);
    let refusal = String::from_utf8_lossy(&plain.stderr);
    assert!(
        !plain.status.success() && refusal.contains(UNREADABLE),
        "a textconv program that fails does not stop plain `git diff` with `{UNREADABLE}`, so \
         this test could only pass vacuously: exit {:?}, {refusal}",
        plain.status.code()
    );

    let (conflicts, halt) = merge_with_halt_diff(&repo);
    let diff = halt.diff().unwrap_or_else(|message| {
        panic!("a textconv program that fails left the halt diff with no diff: {message}")
    });
    assert!(
        diff_lines(&halt).iter().any(|line| line == OPENING_MARKER),
        "the halt diff has to show the region the merge conflicted in, from its \
         `{OPENING_MARKER}` line: {}",
        String::from_utf8_lossy(diff)
    );

    let plain_count = textconv_repo()
        .scratch(OURS)
        .replay_merge(THEIRS)
        .expect("replay the same merge through the plain entrance");
    assert_eq!(
        conflicts, plain_count,
        "the capture changed a count: the merge that captures and the plain merge have to \
         measure one fixture the same way"
    );
}

/// The lines around the one region of a halt diff of [`conflicting_repo`]:
/// the lines between the hunk header and the opening marker, and the lines
/// after the closing marker.
///
/// The fixture rewrites one line of thirty, so its halt diff has one hunk, and
/// that hunk holds one region with context on each side. The hunk header of a
/// combined diff starts with `@@@`.
fn lines_around_the_region(diff: &[u8]) -> (Vec<String>, Vec<String>) {
    let text = String::from_utf8_lossy(diff);
    let lines: Vec<&str> = text.lines().collect();

    let header = lines
        .iter()
        .position(|line| line.starts_with("@@@"))
        .unwrap_or_else(|| panic!("the diff has no hunk header: {text}"));
    let opening = lines
        .iter()
        .position(|line| *line == OPENING_MARKER)
        .unwrap_or_else(|| panic!("the diff has no `{OPENING_MARKER}` line: {text}"));
    let closing = lines
        .iter()
        .position(|line| line.starts_with(CLOSING_MARKER))
        .unwrap_or_else(|| panic!("the diff has no `{CLOSING_MARKER}` line: {text}"));

    let owned = |span: &[&str]| span.iter().map(|line| (*line).to_owned()).collect();
    (
        owned(&lines[header + 1..opening]),
        owned(&lines[closing + 1..]),
    )
}

/// A halt diff carries three lines of context on each side of its region,
/// whatever `diff.context` says.
///
/// `diff.context` sets how many unchanged lines git shows around a change.
/// Three is git's own default. At `diff.context=0`, git 2.55 gave this
/// fixture a halt diff of 10 lines in place of 16, and the reader lost the
/// lines that show where the region is in the file. The setting belongs to
/// the developer, so without a pin one halt gives a different text on each
/// machine.
#[test]
fn a_halt_diff_carries_three_lines_of_context_whatever_diff_context_says() {
    /// The context lines git gives on each side of a change when no setting
    /// says otherwise.
    const DEFAULT_CONTEXT: usize = 3;
    /// The prefix columns of a context line in a combined diff of two
    /// parents: one space for each parent.
    const CONTEXT_PREFIX: &str = "  ";

    let repo = conflicting_repo_with("diff.context", "0");

    let plain = plain_diff_at_a_real_halt(&repo, &["--no-color", "--diff-filter=U"]);
    let (before, after) = lines_around_the_region(&plain.stdout);
    assert!(
        before.len() < DEFAULT_CONTEXT && after.len() < DEFAULT_CONTEXT,
        "`diff.context=0` takes no context line out of plain `git diff`, so this test could \
         only pass vacuously: {before:?} {after:?}"
    );

    let (_, halt) = merge_with_halt_diff(&repo);
    let diff = halt
        .diff()
        .unwrap_or_else(|message| panic!("git gave no diff at the halt: {message}"));
    let (before, after) = lines_around_the_region(diff);
    assert_eq!(
        (before.len(), after.len()),
        (DEFAULT_CONTEXT, DEFAULT_CONTEXT),
        "under `diff.context=0` the halt diff has to carry {DEFAULT_CONTEXT} lines of context \
         on each side of the region: {}",
        String::from_utf8_lossy(diff)
    );
    assert!(
        before
            .iter()
            .chain(&after)
            .all(|line| line.starts_with(CONTEXT_PREFIX)),
        "each line around the region has to be a context line, with a space for each parent: \
         {before:?} {after:?}"
    );
}

/// The two file header lines of a halt diff of one file: the `---` line and
/// the `+++` line.
///
/// Read from the header of the file alone, the lines above the first hunk
/// header. A content line of a combined diff can also start with `---`: a line
/// that both parents hold and the result does not starts with `--`, so a
/// removed line whose text is `- a/f.txt` reads `--- a/f.txt`. A line below
/// the first hunk header is therefore never a file header line.
fn file_header_lines(diff: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(diff)
        .lines()
        .take_while(|line| !line.starts_with("@@"))
        .filter(|line| line.starts_with("--- ") || line.starts_with("+++ "))
        .map(str::to_owned)
        .collect()
}

/// A halt diff names its file as `a/<name>` and `b/<name>`, whatever the
/// prefix settings of the developer say.
///
/// Four settings change the prefixes of the two file header lines, and git
/// 2.55 was watched to apply each one to the conflict diff at a halt.
/// `diff.noprefix=true` removes the prefixes, `diff.mnemonicPrefix=true` gives
/// `i/` and `w/`, and `diff.srcPrefix` and `diff.dstPrefix` give their own
/// values. A reader, or a tool that reads the halt diff, then gets a different
/// header for one halt on each machine. Each setting gets a fixture of its
/// own, because each one changes the prefixes alone.
#[test]
fn a_halt_diff_names_its_file_with_the_default_prefixes_whatever_the_prefix_settings_say() {
    /// The file that both branches of [`conflicting_repo`] rewrite.
    const CONFLICTED: &str = "shared.txt";

    let expected = vec![format!("--- a/{CONFLICTED}"), format!("+++ b/{CONFLICTED}")];
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.srcPrefix", "x/"),
        ("diff.dstPrefix", "y/"),
    ] {
        let repo = conflicting_repo_with(key, value);

        let plain = plain_diff_at_a_real_halt(&repo, &["--no-color", "--diff-filter=U"]);
        let plain_header = file_header_lines(&plain.stdout);
        assert!(
            plain.status.success() && plain_header.len() == 2 && plain_header != expected,
            "`{key}={value}` leaves plain `git diff` with git's default prefixes, so this test \
             could only pass vacuously: {plain_header:?}"
        );

        let (_, halt) = merge_with_halt_diff(&repo);
        let diff = halt
            .diff()
            .unwrap_or_else(|message| panic!("git gave no diff at the halt: {message}"));
        assert_eq!(
            file_header_lines(diff),
            expected,
            "under `{key}={value}` the halt diff has to name its file with git's default \
             prefixes: {}",
            String::from_utf8_lossy(diff)
        );
    }
}

/// The object ids on the `index` line of a halt diff of one file.
///
/// A combined diff names one blob for each parent and one for the result, as
/// `index <ours>,<theirs>..<result>`. The result is the working tree, which
/// has no blob yet, so its id is all zeros. The line is read from the header
/// of the file alone, above the first hunk header.
fn index_line_ids(diff: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(diff);
    let ids = text
        .lines()
        .take_while(|line| !line.starts_with("@@"))
        .find_map(|line| line.strip_prefix("index "))
        .unwrap_or_else(|| panic!("the diff has no `index` line above its first hunk: {text}"));
    let (parents, result) = ids
        .split_once("..")
        .unwrap_or_else(|| panic!("the `index` line has no `..` in it: {ids}"));
    parents
        .split(',')
        .chain(result.split_whitespace().take(1))
        .map(str::to_owned)
        .collect()
}

/// A halt diff abbreviates each object id to git's own default length,
/// whatever `core.abbrev` says.
///
/// `core.abbrev` sets how many hex digits git prints for an abbreviated id.
/// At `core.abbrev=12`, git 2.55 printed the `index` line of this conflict as
/// `index ca88aa969c5a,9e4d34aa23b2..000000000000` in place of
/// `index ca88aa9,9e4d34a..0000000`. The flag `--abbrev=7` does not reach that
/// line of a combined diff, so the pin is `-c core.abbrev=auto` in the safety
/// configuration of the runner. For a repository with as few objects as this
/// fixture, `auto` gives git's shortest default length, seven digits.
#[test]
fn a_halt_diff_abbreviates_each_id_to_git_s_default_length_whatever_core_abbrev_says() {
    /// The digits of an id under `auto`, in a repository with few objects.
    const DEFAULT_ABBREV: usize = 7;
    /// The digits the fixture asks for.
    const HOSTILE_ABBREV: usize = 12;

    let repo = conflicting_repo_with("core.abbrev", &HOSTILE_ABBREV.to_string());
    let digits =
        |ids: &[String]| -> Vec<usize> { ids.iter().map(|id| id.chars().count()).collect() };

    let plain = plain_diff_at_a_real_halt(&repo, &["--no-color", "--diff-filter=U"]);
    let plain_ids = index_line_ids(&plain.stdout);
    assert_eq!(
        digits(&plain_ids),
        vec![HOSTILE_ABBREV; 3],
        "`core.abbrev={HOSTILE_ABBREV}` does not lengthen the ids on the `index` line of plain \
         `git diff`, so this test could only pass vacuously: {plain_ids:?}"
    );

    let (_, halt) = merge_with_halt_diff(&repo);
    let diff = halt
        .diff()
        .unwrap_or_else(|message| panic!("git gave no diff at the halt: {message}"));
    let ids = index_line_ids(diff);
    assert_eq!(
        digits(&ids),
        vec![DEFAULT_ABBREV; 3],
        "under `core.abbrev={HOSTILE_ABBREV}` the `index` line of the halt diff has to carry \
         three ids of {DEFAULT_ABBREV} digits: {ids:?}"
    );
    assert!(
        ids.iter()
            .all(|id| id.chars().all(|digit| digit.is_ascii_hexdigit())),
        "each id on the `index` line has to be hex digits and nothing else: {ids:?}"
    );
}

/// The program that makes the key of [`ssh_signing_key`], and that git runs
/// to make and to check an SSH signature.
const SSH_KEYGEN: &str = "ssh-keygen";

/// The principal that [`ssh_signing_key`] trusts its key for. `ssh-keygen`
/// names the principal in each good signature it reports.
const SIGNER: &str = "signer@example.invalid";

/// The name of the private key in the directory of [`ssh_signing_key`].
const KEY_FILE: &str = "key";

/// The name of the allowed-signers file in the directory of
/// [`ssh_signing_key`].
const ALLOWED_SIGNERS_FILE: &str = "allowed-signers";

/// A new ed25519 key with no passphrase, and an allowed-signers file that
/// trusts that key for [`SIGNER`], in a temporary directory of their own.
///
/// The directory is not in the fixture, so no key goes into a repository. The
/// value holds the directory, so the two files stay on disk while it lives.
///
/// A machine that cannot run [`SSH_KEYGEN`] fails the test here, with a
/// message that says why. The test does not skip. A test that skips where the
/// program is missing passes there and pins nothing.
fn ssh_signing_key() -> TempDir {
    let dir = TempDir::new().expect("create a temporary directory for the signing key");
    let key = dir.path().join(KEY_FILE);

    let generated = Command::new(SSH_KEYGEN)
        .args(["-q", "-t", "ed25519", "-N", "", "-C", SIGNER, "-f"])
        .arg(&key)
        .output()
        .unwrap_or_else(|err| {
            panic!(
                "could not run `{SSH_KEYGEN}`, so this test cannot sign a commit and cannot show \
                 the hazard it pins: {err}"
            )
        });
    assert!(
        generated.status.success(),
        "`{SSH_KEYGEN}` made no key, so this test cannot sign a commit and cannot show the \
         hazard it pins: {}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let public = std::fs::read_to_string(dir.path().join(format!("{KEY_FILE}.pub")))
        .expect("read the public key that ssh-keygen wrote");
    std::fs::write(
        dir.path().join(ALLOWED_SIGNERS_FILE),
        format!("{SIGNER} {public}"),
    )
    .expect("write the allowed-signers file");

    dir
}

/// The path of `name` in the directory of [`ssh_signing_key`], as a setting of
/// git spells it.
fn key_path(key: &TempDir, name: &str) -> String {
    key.path()
        .join(name)
        .to_str()
        .expect("a temporary directory has a UTF-8 path")
        .to_owned()
}

/// [`conflicting_repo`] with the one commit of [`OURS`] signed by the key in
/// `key`, and `log.showSignature=true` in the fixture's own configuration.
///
/// A rebase of [`OURS`] onto [`THEIRS`] stops once, on that signed commit. The
/// fixture trusts the key in `gpg.ssh.allowedSignersFile`, so git reports a
/// good signature. It names [`SSH_KEYGEN`] in `gpg.ssh.program`. A local value
/// wins over a global one, so a signing program of the developer does not make
/// or check this signature.
///
/// `commit --amend -S` signs the commit again, so the fixture keeps the shape
/// of [`conflicting_repo`]. `SSH_AUTH_SOCK` is empty on that call, so the key
/// file signs and no agent of the developer takes part.
fn signed_stop_repo(key: &TempDir) -> TestRepo {
    let repo = conflicting_repo_with("log.showSignature", "true");
    repo.git(&["config", "gpg.ssh.program", SSH_KEYGEN]);
    repo.git(&[
        "config",
        "gpg.ssh.allowedSignersFile",
        &key_path(key, ALLOWED_SIGNERS_FILE),
    ]);

    repo.checkout(OURS);
    let signing_key = format!("user.signingkey={}", key_path(key, KEY_FILE));
    let signed = repo.try_git(
        &[
            "-c",
            "gpg.format=ssh",
            "-c",
            &signing_key,
            "commit",
            "--amend",
            "-q",
            "-S",
            "--no-edit",
        ],
        &[("SSH_AUTH_SOCK", "")],
    );
    assert!(
        signed.status.success(),
        "git could not sign the commit of {OURS} with `{SSH_KEYGEN}`, so the rebase has no \
         signed commit to stop on: {}",
        String::from_utf8_lossy(&signed.stderr)
    );
    repo.checkout(FIXTURE_BRANCH);

    repo
}

/// A halt diff names a signed stopped commit on one line, whatever
/// `log.showSignature` says.
///
/// The name of a stopped commit is what `git log -1 --format="%h %s"` prints
/// for `REBASE_HEAD`. `log.showSignature=true` makes that call check the
/// signature of a signed commit. For an SSH-signed commit, git 2.55 wrote
/// `Good "git" signature for <principal> with ED25519 key SHA256:...` on
/// stdout, above `<id> <subject>`. The runner kept both lines, so the stop
/// heading of `grind --diff` had two lines, and its first line named a
/// signature and not the commit. The pin is `-c log.showSignature=false` in
/// the safety configuration of the runner.
///
/// The armed control asks plain git in the fixture for the name of the signed
/// commit, and the signature line must be there. The expected name comes from
/// the runner of the scratch worktree with `--no-show-signature`. The two names
/// then take one configuration, so the short id has one length whatever
/// `core.abbrev` says.
#[test]
fn a_halt_diff_names_a_signed_stopped_commit_on_one_line_whatever_log_show_signature_says() {
    /// The format of the name of a stopped commit: its short id, a space, and
    /// its subject.
    const NAME_FORMAT: &str = "--format=%h %s";

    let key = ssh_signing_key();
    let repo = signed_stop_repo(&key);

    let plain = repo.try_git(&["log", "-1", NAME_FORMAT, OURS], &[]);
    let plain_name = String::from_utf8_lossy(&plain.stdout);
    assert!(
        plain.status.success()
            && plain_name.lines().count() > 1
            && plain_name.lines().any(|line| line.contains(SIGNER)),
        "`log.showSignature=true` puts no signature line above the name of a signed commit in \
         plain `git log`, so this test could only pass vacuously: {plain_name:?} {}",
        String::from_utf8_lossy(&plain.stderr)
    );

    let scratch = repo.scratch(FIXTURE_BRANCH);
    let expected = scratch
        .testing_git()
        .run("log", &["--no-show-signature", "-1", NAME_FORMAT, OURS])
        .expect("name the signed commit with no signature line");
    assert!(
        !expected.is_empty() && !expected.contains('\n'),
        "`--no-show-signature` gave no one-line name for the signed commit, so there is no name \
         here to compare the halt diff against: {expected:?}"
    );

    let (_, diffs) = replay_with_diffs(&scratch, OURS, THEIRS);
    assert_eq!(
        diffs.len(),
        1,
        "a rebase of {OURS} onto {THEIRS} stops once, on the signed commit, so it has to give one \
         halt diff: {diffs:?}"
    );
    assert_eq!(
        diffs.iter().next().and_then(HaltDiff::stopped),
        Some(expected.as_str()),
        "under `log.showSignature=true` the name of a signed stopped commit has to be the one \
         line `<id> <subject>`. A signature line above it puts a second line into the stop \
         heading, and that line names a signature and not the commit"
    );
}

/// A halt diff runs no external diff program, whatever `diff.external` names.
///
/// `diff.external` names a program that git runs in place of its own diff.
/// The capture must never run a program from the configuration of the
/// developer, so the diff call carries `--no-ext-diff`. Git 2.55 was watched
/// not to run the program for a combined diff at all, so this test passes
/// with and without the flag. `MUTATIONS.md` records the flag as a guard that
/// no test can make fail. The test holds the day git starts to run the program
/// for a combined diff, and it holds now that the flag changes nothing else in
/// the halt diff.
///
/// The program is a script in the git directory of the fixture, and it makes
/// a sentinel file beside itself, so a run of the program leaves evidence. It
/// finds that place from its own path, so no path is written into the script.
/// The armed control runs an ordinary diff of two commits with plain git,
/// which does run the program, and the sentinel must appear. That shows that
/// the setting is live and that the script works. The control then removes the
/// sentinel, and the capture must leave none behind.
#[cfg(unix)]
#[test]
fn a_halt_diff_runs_no_external_diff_program_whatever_diff_external_names() {
    use std::os::unix::fs::PermissionsExt as _;

    /// A program that makes a sentinel file in its own directory.
    const PROGRAM: &str = "#!/bin/sh\ntouch \"$(dirname \"$0\")/external-diff-ran\"\n";

    let repo = conflicting_repo();
    let git_dir = repo.path().join(".git");
    let program = git_dir.join("external-diff");
    let sentinel = git_dir.join("external-diff-ran");
    std::fs::write(&program, PROGRAM).expect("write the external diff program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("make the external diff program executable");
    let program_path = program
        .to_str()
        .expect("a temporary directory has a UTF-8 path");
    repo.git(&["config", "diff.external", program_path]);

    let ordinary = repo.try_git(&["diff", FIXTURE_BRANCH, OURS], &[]);
    assert!(
        ordinary.status.success() && sentinel.exists(),
        "`diff.external` did not run its program for an ordinary diff of two commits, so the \
         setting is not live and this test could only pass vacuously: {}",
        String::from_utf8_lossy(&ordinary.stderr)
    );
    std::fs::remove_file(&sentinel).expect("remove the sentinel the control made");

    let (_, halt) = merge_with_halt_diff(&repo);
    assert!(
        !sentinel.exists(),
        "the capture ran the program that `diff.external` names"
    );
    let lines = diff_lines(&halt);
    assert!(
        lines.iter().any(|line| line == OPENING_MARKER),
        "the halt diff has to show the region the merge conflicted in, from its \
         `{OPENING_MARKER}` line: {lines:#?}"
    );
}
