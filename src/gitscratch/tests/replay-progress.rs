//! What a replay reports to an observer.
//!
//! The observer is opt-in, so these tests build the scratch worktree through
//! `scratch_with_progress` and read back what the observer received.

use std::sync::{Arc, Mutex};

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
