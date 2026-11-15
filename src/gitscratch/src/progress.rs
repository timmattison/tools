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
//!
//! A rebase replay also sends [`ReplayProgress::Commit`] reports. Git keeps two
//! state files in the `rebase-merge` directory of the scratch worktree. The
//! file `msgnum` holds the number of the commit that git replays now. The file
//! `end` holds the number of commits to replay. `git-prompt.sh` reads the same
//! two files. The replay does not parse the text that git prints for a person,
//! because git can change that text.
//!
//! The replay reads the two files in three places:
//!
//! - A poll reads them every 100 ms while a `git rebase` command runs.
//! - A read at each halt reports the position of the halt before the replay
//!   continues. This read does not depend on the poll.
//! - A last read reports `Commit { position: total, total }` when the rebase
//!   completes. Git deletes the directory then, so the poll cannot see the last
//!   position. If no read ever gave a total, the replay counts the commits that
//!   it put on the new base and reports that count. An observer thus gets an
//!   end report that does not depend on timing.
//!
//! A position never decreases, and a repeated position is reported once.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

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

/// The name of the state file that holds the number of the commit that git
/// replays now.
const POSITION_FILE: &str = "msgnum";

/// The name of the state file that holds the number of commits to replay.
const TOTAL_FILE: &str = "end";

/// Reads the state files of a running rebase and reports each new position.
///
/// The rebase replay builds one watch before it starts the rebase. The watch
/// keeps the last pair that it reported, so a position never decreases and a
/// repeated position is reported once. Reading the files costs no process
/// start, which is why the watch reads them and does not ask git.
pub(crate) struct RebaseWatch<'a> {
    /// The receiver of the reports.
    observer: &'a dyn ReplayObserver,
    /// The `rebase-merge` directory, resolved once before the rebase starts.
    state_dir: PathBuf,
    /// The last `(position, total)` that the watch reported. The lock stays
    /// held during the call to the observer, so the reports arrive in the
    /// order of the offers.
    last: Mutex<Option<(usize, usize)>>,
}

impl<'a> RebaseWatch<'a> {
    /// Build a watch that reads `state_dir` and reports to `observer`.
    ///
    /// `state_dir` is the `rebase-merge` directory of the scratch worktree.
    /// It does not need to exist yet, because git creates it after the rebase
    /// starts.
    pub(crate) fn new(observer: &'a dyn ReplayObserver, state_dir: PathBuf) -> Self {
        Self {
            observer,
            state_dir,
            last: Mutex::new(None),
        }
    }

    /// Read the state files once and report the position that they hold.
    ///
    /// A read gives a position only when both files exist and parse, the total
    /// is more than zero, and the position is not more than the total. In every
    /// other case the call does nothing, and the last position stays. Before
    /// git writes `msgnum`, the files do not exist, and that is the normal
    /// case at the start of a rebase.
    pub(crate) fn poll(&self) {
        if let Some((position, total)) = self.read() {
            self.offer(position, total);
        }
    }

    /// Run `call` and read the state files while it runs.
    pub(crate) fn watch<T>(&self, call: impl FnOnce() -> T) -> T {
        call()
    }

    /// Report the end of the rebase.
    ///
    /// Git deletes the state directory when the rebase completes, so the poll
    /// cannot see the last position. If a read gave a total, the watch reports
    /// `(total, total)`. If no read ever gave a total, the rebase ended before
    /// the first poll or git replayed nothing and wrote no state. The watch
    /// then calls `landed` and, when it gives `Some(n)` with `n > 0`, reports
    /// `(n, n)`. The observer thus gets an end report that does not depend on
    /// timing. Otherwise the watch reports nothing.
    pub(crate) fn finish(&self, landed: impl FnOnce() -> Option<usize>) {
        let known_total = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|(_, total)| total);
        match known_total {
            Some(total) => self.offer(total, total),
            None => {
                if let Some(count) = landed().filter(|&count| count > 0) {
                    self.offer(count, count);
                }
            }
        }
    }

    /// Read the two state files. Gives `None` when they do not hold a position.
    fn read(&self) -> Option<(usize, usize)> {
        let position = self.read_number(POSITION_FILE)?;
        let total = self.read_number(TOTAL_FILE)?;
        (total > 0 && position <= total).then_some((position, total))
    }

    /// Read one state file as a number. Gives `None` when the file does not
    /// exist or does not hold a number.
    fn read_number(&self, name: &str) -> Option<usize> {
        std::fs::read_to_string(self.state_dir.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// Report `(position, total)` when it is new and does not go back.
    ///
    /// The first offer always passes. A later offer passes when it differs from
    /// the last pair and its position is not less than the last position.
    fn offer(&self, position: usize, total: usize) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let is_new = match *last {
            None => true,
            Some(previous) => (position, total) != previous && position >= previous.0,
        };
        if is_new {
            *last = Some((position, total));
            self.observer
                .report(ReplayProgress::Commit { position, total });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// An observer that keeps each report, in the order it arrived.
    #[derive(Default)]
    struct Recorder {
        reports: Mutex<Vec<ReplayProgress>>,
    }

    impl Recorder {
        /// A copy of the reports so far.
        fn reports(&self) -> Vec<ReplayProgress> {
            self.reports
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl ReplayObserver for Recorder {
        fn report(&self, progress: ReplayProgress) {
            self.reports
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(progress);
        }
    }

    /// The `Commit` report for `position` of `total`.
    fn commit(position: usize, total: usize) -> ReplayProgress {
        ReplayProgress::Commit { position, total }
    }

    /// Write the two state files into `dir`.
    fn write_state(dir: &Path, position: &str, total: &str) {
        std::fs::write(dir.join(POSITION_FILE), position).expect("write the position file");
        std::fs::write(dir.join(TOTAL_FILE), total).expect("write the total file");
    }

    #[test]
    fn a_poll_reports_the_position_the_files_hold() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());
        write_state(dir.path(), "3\n", "7\n");

        watch.poll();

        assert_eq!(recorder.reports(), [commit(3, 7)]);
    }

    #[test]
    fn a_poll_before_the_files_exist_reports_nothing() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let missing = dir.path().join("not-yet");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, missing);

        watch.poll();

        assert_eq!(recorder.reports(), []);
    }

    #[test]
    fn a_missing_empty_or_unparsable_file_keeps_the_last_position() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());
        write_state(dir.path(), "2", "7");
        watch.poll();

        std::fs::remove_file(dir.path().join(POSITION_FILE)).expect("remove the position file");
        watch.poll();
        write_state(dir.path(), "", "7");
        watch.poll();
        write_state(dir.path(), "three", "7");
        watch.poll();
        write_state(dir.path(), "3", "");
        watch.poll();
        assert_eq!(recorder.reports(), [commit(2, 7)]);

        write_state(dir.path(), "3", "7");
        watch.poll();
        assert_eq!(recorder.reports(), [commit(2, 7), commit(3, 7)]);
    }

    #[test]
    fn a_read_that_cannot_be_a_position_reports_nothing() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());

        write_state(dir.path(), "0", "0");
        watch.poll();
        write_state(dir.path(), "8", "7");
        watch.poll();

        assert_eq!(recorder.reports(), []);
    }

    #[test]
    fn a_position_never_decreases_and_a_repeat_is_not_reported_twice() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());

        write_state(dir.path(), "4", "7");
        watch.poll();
        watch.poll();
        write_state(dir.path(), "2", "7");
        watch.poll();
        write_state(dir.path(), "5", "7");
        watch.poll();

        assert_eq!(recorder.reports(), [commit(4, 7), commit(5, 7)]);
    }

    #[test]
    fn finish_reports_the_whole_rebase_and_skips_landed_when_a_total_is_known() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());
        write_state(dir.path(), "2", "7");
        watch.poll();

        watch.finish(|| panic!("a known total needs no count"));

        assert_eq!(recorder.reports(), [commit(2, 7), commit(7, 7)]);
    }

    #[test]
    fn finish_reports_the_landed_count_when_no_read_gave_a_total() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());

        watch.finish(|| Some(4));

        assert_eq!(recorder.reports(), [commit(4, 4)]);
    }

    #[test]
    fn finish_reports_nothing_for_a_count_of_zero_or_no_count() {
        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());

        watch.finish(|| Some(0));
        watch.finish(|| None);

        assert_eq!(recorder.reports(), []);
    }

    #[test]
    fn the_poll_reads_while_the_watched_call_blocks() {
        const WAIT: std::time::Duration = std::time::Duration::from_secs(10);
        const STEP: std::time::Duration = std::time::Duration::from_millis(10);

        let dir = tempfile::tempdir().expect("make a state directory");
        let recorder = Recorder::default();
        let watch = RebaseWatch::new(&recorder, dir.path().to_path_buf());

        let arrived = watch.watch(|| {
            write_state(dir.path(), "3", "7");
            let started = std::time::Instant::now();
            while started.elapsed() < WAIT {
                if recorder.reports().contains(&commit(3, 7)) {
                    return true;
                }
                std::thread::sleep(STEP);
            }
            false
        });

        assert!(arrived, "no poll read the files while the call blocked");
    }
}
