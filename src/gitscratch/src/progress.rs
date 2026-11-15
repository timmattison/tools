//! What a replay tells a caller while it runs.
//!
//! A replay can take a long time, and a caller that draws a progress display
//! needs to know where the replay stands. This module holds the types of that
//! report. [`ReplayObserver`] is the receiver, [`ReplayProgress`] is one
//! report, and [`Phase`] names a stage of a replay.
//!
//! The observer is opt-in. [`Repo::scratch`](crate::Repo::scratch) takes no
//! observer, so `gsw` and `grist` show no progress. A caller that wants
//! reports builds its scratch worktree with
//! [`Repo::scratch_with_progress`](crate::Repo::scratch_with_progress).
//!
//! A report never changes the answer of a replay. The observer only listens.
//! It cannot stop a replay and it cannot change what a replay returns.
//!
//! This crate sends one [`ReplayProgress::Phase`] report at the start of each
//! phase of a merge replay and of the creation of the scratch worktree.

use std::sync::Arc;

/// A stage of a replay, named by the work git does in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The `git worktree add` of the scratch worktree.
    CreateScratch,
    /// The rebase or the merge itself.
    Replay,
    /// The read of the conflicted files after a merge halts.
    CountHunks,
}

/// One report that a replay sends to its observer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayProgress {
    /// A phase starts.
    Phase(Phase),
    /// A rebase replays commit `position` of `total`.
    Commit {
        /// The place of the commit in the replay, counted from one.
        position: usize,
        /// The number of commits in the replay.
        total: usize,
    },
}

/// The receiver of the reports of a replay.
///
/// The trait needs `Send + Sync` so that a caller can draw the display from
/// another thread than the one that runs the replay.
pub trait ReplayObserver: Send + Sync {
    /// Receive one report. The call must return fast, because the replay waits
    /// for it.
    fn report(&self, progress: ReplayProgress);
}

/// A shared handle to an observer is an observer, so a caller can keep its own
/// handle and read the observer after the replay.
impl<T: ReplayObserver + ?Sized> ReplayObserver for Arc<T> {
    fn report(&self, progress: ReplayProgress) {
        (**self).report(progress);
    }
}
