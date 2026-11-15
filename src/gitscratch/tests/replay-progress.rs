//! What a replay reports to an observer.
//!
//! The observer is opt-in, so these tests build the scratch worktree through
//! `scratch_with_progress` and read back what the observer received.

#[cfg(unix)]
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use gitscratch::testing::git_shim::GitShim;
#[cfg(unix)]
use gitscratch::testing::run_child_half;
use gitscratch::testing::{
    conflicting_repo, independent_branches_repo, numbered_lines, replace_line, TestRepo,
};
use gitscratch::{Phase, ReplayObserver, ReplayProgress, Repo};

/// An observer that keeps each report, in the order it arrived.
struct Recorder {
    reports: Mutex<Vec<ReplayProgress>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            reports: Mutex::new(Vec::new()),
        })
    }

    /// A copy of the reports so far.
    fn reports(&self) -> Vec<ReplayProgress> {
        self.reports.lock().expect("the recorder lock").clone()
    }
}

impl ReplayObserver for Recorder {
    fn report(&self, progress: ReplayProgress) {
        self.reports
            .lock()
            .expect("the recorder lock")
            .push(progress);
    }
}

/// A merge that halts has three phases, and each one reports when it starts.
/// A merge sends no `Commit` report, because only a rebase replays commits.
#[test]
fn a_merge_reports_its_three_phases_in_order_and_no_commit() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture");
    let recorder = Recorder::new();
    let scratch = repo
        .scratch_with_progress("left", Arc::clone(&recorder))
        .expect("build a scratch worktree");

    scratch.replay_merge("right").expect("replay the merge");

    assert_eq!(
        recorder.reports(),
        [
            ReplayProgress::Phase(Phase::CreateScratch),
            ReplayProgress::Phase(Phase::Replay),
            ReplayProgress::Phase(Phase::CountHunks),
        ]
    );
}

/// A clean merge never reads a conflicted file, so it has no `CountHunks`
/// phase. The report sits after the early return for that reason.
#[test]
fn a_clean_merge_reports_no_hunk_count() {
    let fixture = independent_branches_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture");
    let recorder = Recorder::new();
    let scratch = repo
        .scratch_with_progress("alpha", Arc::clone(&recorder))
        .expect("build a scratch worktree");

    scratch.replay_merge("beta").expect("replay the merge");

    assert_eq!(
        recorder.reports(),
        [
            ReplayProgress::Phase(Phase::CreateScratch),
            ReplayProgress::Phase(Phase::Replay),
        ]
    );
}

/// Every position that the `Commit` reports in `reports` name, in order.
fn commit_reports(reports: &[ReplayProgress]) -> Vec<(usize, usize)> {
    reports
        .iter()
        .filter_map(|report| match report {
            ReplayProgress::Commit { position, total } => Some((*position, *total)),
            ReplayProgress::Phase(_) => None,
        })
        .collect()
}

/// A rebase of five commits halts at the third, which collides with `main`.
/// The halt reports its own position, and the last report names the whole
/// rebase, whatever the timing of the poll.
#[test]
fn a_halt_reports_its_position_and_the_last_report_is_the_whole_rebase() {
    const CONTESTED_LINE: usize = 15;

    let fixture = TestRepo::init();
    let base = numbered_lines(30);
    fixture.commit_file("shared.txt", &base, "base");

    fixture.branch("feature");
    fixture.commit_file("one.txt", "one\n", "one");
    fixture.commit_file("two.txt", "two\n", "two");
    fixture.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "feature-edit"),
        "three",
    );
    fixture.commit_file("four.txt", "four\n", "four");
    fixture.commit_file("five.txt", "five\n", "five");

    fixture.checkout("main");
    fixture.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "main-edit"),
        "main work",
    );

    let repo = Repo::open(fixture.path()).expect("open the fixture");
    let recorder = Recorder::new();
    let scratch = repo
        .scratch_with_progress("feature", Arc::clone(&recorder))
        .expect("build a scratch worktree");

    let cost = scratch.replay_rebase("main").expect("replay the rebase");

    let reports = recorder.reports();
    assert_eq!(
        cost.stops().digits(),
        "1",
        "the fixture must halt once, at commit 3"
    );
    assert_eq!(
        reports.get(..2),
        Some(
            &[
                ReplayProgress::Phase(Phase::CreateScratch),
                ReplayProgress::Phase(Phase::Replay),
            ][..]
        )
    );
    let commits = commit_reports(&reports);
    assert!(
        commits.iter().all(|&(_, total)| total == 5),
        "every total is 5: {commits:?}"
    );
    assert!(
        commits.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "a position never decreases: {commits:?}"
    );
    assert!(
        reports.contains(&ReplayProgress::Commit {
            position: 3,
            total: 5
        }),
        "the halt reports commit 3: {reports:?}"
    );
    assert_eq!(
        reports.last(),
        Some(&ReplayProgress::Commit {
            position: 5,
            total: 5
        })
    );
}

/// A clean rebase of thirty commits, which git may finish between two polls.
/// The reports that arrive depend on timing, but the first two phases, the
/// total, the order, and the last report do not.
#[test]
fn a_rebase_of_thirty_commits_reports_each_position_against_a_total_of_thirty() {
    const COMMITS: usize = 30;

    let fixture = TestRepo::init();
    fixture.commit_file("shared.txt", &numbered_lines(30), "base");

    fixture.branch("feature");
    for number in 1..=COMMITS {
        fixture.commit_file(
            &format!("file-{number:02}.txt"),
            &format!("{number}\n"),
            &format!("commit {number}"),
        );
    }

    fixture.checkout("main");
    fixture.commit_file("main.txt", "main\n", "main work");

    let repo = Repo::open(fixture.path()).expect("open the fixture");
    let recorder = Recorder::new();
    let scratch = repo
        .scratch_with_progress("feature", Arc::clone(&recorder))
        .expect("build a scratch worktree");

    scratch.replay_rebase("main").expect("replay the rebase");

    let reports = recorder.reports();
    assert_eq!(
        reports.get(..2),
        Some(
            &[
                ReplayProgress::Phase(Phase::CreateScratch),
                ReplayProgress::Phase(Phase::Replay),
            ][..]
        )
    );
    let commits = commit_reports(&reports);
    assert!(
        commits.iter().all(|&(_, total)| total == COMMITS),
        "every total is {COMMITS}: {commits:?}"
    );
    assert!(
        commits.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "a position never decreases: {commits:?}"
    );
    assert_eq!(
        reports.last(),
        Some(&ReplayProgress::Commit {
            position: COMMITS,
            total: COMMITS
        })
    );
}

/// The name of the variable that hands the child half the fixture path. It does
/// not start with `GIT_`, because `childenv` sheds that prefix.
#[cfg(unix)]
const FIXTURE_VARIABLE: &str = "GITSCRATCH_SHIM_FIXTURE";

/// The name of the variable that says whether the child half builds its scratch
/// with an observer (`1`) or without one (`0`).
#[cfg(unix)]
const OBSERVER_VARIABLE: &str = "GITSCRATCH_SHIM_OBSERVER";

/// The filter that the parent halves hand to the child half.
#[cfg(unix)]
const STATE_DIRECTORY_CHILD: &str = "state_directory_child";

/// A clean rebase that must write a commit: `feature` holds one commit on its
/// own file, and `main` moved on another file.
#[cfg(unix)]
fn clean_rebase_fixture() -> TestRepo {
    let fixture = TestRepo::init();
    fixture.commit_file("base.txt", "base\n", "base");
    fixture.branch("feature");
    fixture.commit_file("feature.txt", "feature\n", "feature work");
    fixture.checkout("main");
    fixture.commit_file("main.txt", "main\n", "main work");
    fixture
}

/// The child half of the two tests below. It replays the rebase of `feature`
/// onto `main` under the `PATH` that the parent chose, and returns at once when
/// the parent set no fixture.
#[cfg(unix)]
#[test]
fn state_directory_child() {
    let Some(fixture) = std::env::var_os(FIXTURE_VARIABLE) else {
        return;
    };
    let repo = Repo::open(Path::new(&fixture)).expect("open the fixture");
    let scratch = if std::env::var(OBSERVER_VARIABLE).as_deref() == Ok("1") {
        repo.scratch_with_progress("feature", Recorder::new())
    } else {
        repo.scratch("feature")
    }
    .expect("build a scratch worktree");
    scratch.replay_rebase("main").expect("replay the rebase");
    drop(scratch);
    gitscratch::testing::child_ran();
}

/// Run the child half under a `git` shim, with or without an observer, and
/// count the calls before the first `git rebase` that ask for the state
/// directory of the rebase.
#[cfg(unix)]
fn state_directory_calls_before_the_rebase(observer: bool) -> usize {
    let fixture = clean_rebase_fixture();
    let shim = GitShim::new(&[]);
    let path = shim.path_variable();
    run_child_half(STATE_DIRECTORY_CHILD, |child| {
        child
            .env("PATH", path)
            .env(FIXTURE_VARIABLE, fixture.path())
            .env(OBSERVER_VARIABLE, if observer { "1" } else { "0" });
    })
    .expect("the child half ran the replay");

    let calls = shim.calls();
    let first_rebase = calls
        .iter()
        .position(|call| call.subcommand() == Some("rebase"))
        .expect("the shim saw a `git rebase`, so it saw the replay");
    calls[..first_rebase]
        .iter()
        .filter(|call| call.has_arguments(&["--git-path", "rebase-merge"]))
        .count()
}

/// A replay with no observer is what `gsw` and `grist` run, and it must start
/// no git process for a display that does not exist. So no call before the
/// first `git rebase` asks for the state directory of the rebase. The calls
/// after the rebase ask for it too, on each round, so only the calls before it
/// count.
#[cfg(unix)]
#[test]
fn a_replay_with_no_observer_asks_git_for_no_rebase_state_directory() {
    assert_eq!(state_directory_calls_before_the_rebase(false), 0);
}

/// The armed control of the test above. A replay with an observer asks for the
/// state directory once, before the rebase. It proves that the log sees the call
/// that the test above rules out.
#[cfg(unix)]
#[test]
fn a_replay_with_an_observer_asks_git_for_the_rebase_state_directory_once() {
    assert_eq!(state_directory_calls_before_the_rebase(true), 1);
}
