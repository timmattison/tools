//! What a replay reports to an observer.
//!
//! The observer is opt-in, so these tests build the scratch worktree through
//! `scratch_with_progress` and read back what the observer received.

use std::sync::{Arc, Mutex};

use gitscratch::testing::conflicting_repo;
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
