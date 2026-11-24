//! The link between the state directory and the board. The module reads the
//! files, finds the stale sessions, and tells the board what changed.

use std::path::{Path, PathBuf};

use proctree::Pid;

use crate::board::{Board, Change};
use crate::liveness::{survey, ProcessClock};
use crate::paths::ClaudeDirs;
use crate::registry;
use crate::state::{EpochMillis, StateFile};
use crate::store::{self, DirEvent, ReadOutcome};

/// The reader that keeps a board in step with the state directory.
#[derive(Debug, Clone)]
pub struct Sync {
    state_dir: PathBuf,
    sessions_dir: PathBuf,
}

impl Sync {
    /// A reader for the directories of `dirs`.
    #[must_use]
    pub fn new(dirs: &ClaudeDirs) -> Self {
        Self::with_dirs(dirs.state_dir(), dirs.sessions_dir())
    }

    /// A reader for a state directory and a session registry directory.
    #[must_use]
    pub fn with_dirs(state_dir: PathBuf, sessions_dir: PathBuf) -> Self {
        Self {
            state_dir,
            sessions_dir,
        }
    }

    /// The state directory.
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Load the live sessions into `board`. This is the first read.
    ///
    /// The function reads the process table once for all sessions, and once
    /// more for each stale session before it removes the file. It logs no
    /// change. It removes the file of each stale session, and it does not load
    /// that session. It loads a stopped session with the frozen flag since
    /// `now`. It skips a file that it cannot read.
    ///
    /// # Errors
    ///
    /// Returns the error of the read of the state directory.
    pub fn initial(
        &self,
        board: &mut Board,
        clock: &mut dyn ProcessClock,
        now: EpochMillis,
    ) -> std::io::Result<()> {
        let files: Vec<StateFile> = store::read_dir(&self.state_dir)?
            .into_iter()
            .filter_map(|(_, outcome)| match outcome {
                ReadOutcome::Present(file) => Some(file),
                ReadOutcome::Gone | ReadOutcome::Unreadable(_) => None,
            })
            .collect();
        let sessions: Vec<(Pid, EpochMillis)> = files
            .iter()
            .map(|file| (file.pid, file.started_at))
            .collect();
        let found = survey(&sessions, clock);
        for file in files {
            if found.stale.contains(&file.pid) {
                // A new process can have reused the pid since the first read. Its
                // file stays, and its own write event loads it.
                let _removed = store::remove_if_stale(&self.state_dir, file.pid, clock);
                continue;
            }
            let name = registry::session_name(&self.sessions_dir, file.pid);
            if found.stopped.contains(&file.pid) {
                board.load_stopped(file, name, now);
            } else {
                board.load(file, name);
            }
        }
        Ok(())
    }

    /// Apply one directory event to `board`. Return the changes that it made.
    ///
    /// `File(pid)` reads the file of that pid. A good file of a live process
    /// goes to the board, with the run status that the survey of its process
    /// read. A good file of a stale process goes away, with its
    /// session. A missing file removes the session. A file that cannot be read
    /// changes nothing. `Rescan` does this for each file in the directory, and
    /// it removes each session whose file is gone.
    pub fn on_dir_event(
        &self,
        board: &mut Board,
        event: DirEvent,
        clock: &mut dyn ProcessClock,
        now: EpochMillis,
    ) -> Vec<Change> {
        match event {
            DirEvent::File(pid) => {
                let outcome = store::read_state_file(&store::state_file_path(&self.state_dir, pid));
                self.apply_outcome(board, pid, outcome, clock, now)
                    .into_iter()
                    .collect()
            }
            DirEvent::Rescan => self.rescan(board, clock, now),
        }
    }

    /// Check each session on the board against the process table.
    ///
    /// The function reads the process table once for all sessions, and once
    /// more for each stale session before it removes the file. A process that
    /// ends can leave its file behind. The function removes each such stale
    /// session from the board and from the directory. A stopped process is
    /// live, thus its file stays. For each other session, the function gives
    /// the run status to the board, so a stop adds the frozen flag and a
    /// continue removes it. It returns the changes: one for each removed
    /// session, and one for each freeze or thaw.
    pub fn on_tick(
        &self,
        board: &mut Board,
        clock: &mut dyn ProcessClock,
        now: EpochMillis,
    ) -> Vec<Change> {
        let sessions: Vec<(Pid, EpochMillis)> = board
            .pids()
            .into_iter()
            .filter_map(|pid| Some((pid, board.get(pid)?.file.started_at)))
            .collect();
        let found = survey(&sessions, clock);
        let mut changes = Vec::new();
        for (pid, _) in &sessions {
            if found.stale.contains(pid) {
                let _removed = store::remove_if_stale(&self.state_dir, *pid, clock);
                changes.extend(board.remove(*pid, now));
            } else {
                changes.extend(board.set_run_status(*pid, found.run_status(*pid), now));
            }
        }
        changes
    }

    /// Read each file in the state directory, and drop each session whose file
    /// is gone. When the directory cannot be read, nothing changes.
    fn rescan(
        &self,
        board: &mut Board,
        clock: &mut dyn ProcessClock,
        now: EpochMillis,
    ) -> Vec<Change> {
        let Ok(entries) = store::read_dir(&self.state_dir) else {
            return Vec::new();
        };
        let in_directory: Vec<Pid> = entries.iter().map(|(pid, _)| *pid).collect();
        let mut changes = Vec::new();
        for (pid, outcome) in entries {
            changes.extend(self.apply_outcome(board, pid, outcome, clock, now));
        }
        for pid in board.pids() {
            if !in_directory.contains(&pid) {
                changes.extend(board.remove(pid, now));
            }
        }
        changes
    }

    /// Apply the read of one file to the board. The function surveys the
    /// process of the file once. The board gets the run status that the
    /// survey read, so a file that arrives between two ticks is compared with
    /// the live run status, not with the status of the last tick.
    fn apply_outcome(
        &self,
        board: &mut Board,
        pid: Pid,
        outcome: ReadOutcome,
        clock: &mut dyn ProcessClock,
        now: EpochMillis,
    ) -> Option<Change> {
        match outcome {
            ReadOutcome::Present(file) => {
                let found = survey(&[(pid, file.started_at)], clock);
                if !found.stale.is_empty() {
                    // When the file stays, a new process owns the pid, and its
                    // own write event loads the new file.
                    return store::remove_if_stale(&self.state_dir, pid, clock)
                        .then(|| board.remove(pid, now))
                        .flatten();
                }
                let name = registry::session_name(&self.sessions_dir, pid);
                board.apply(file, name, found.run_status(pid), now)
            }
            ReadOutcome::Gone => board.remove(pid, now),
            ReadOutcome::Unreadable(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proctree::Pid;

    use super::*;
    use crate::board::ShownState;
    use crate::state::PaneState;
    use crate::store::test_support::{state_json, FakeClock};

    const REGISTRY: &str = include_str!("../fixtures/registry-session.json");
    const STARTED: u64 = 1_000_000;

    /// Two temporary directories and a reader for them.
    struct Env {
        state: tempfile::TempDir,
        sessions: tempfile::TempDir,
        sync: Sync,
    }

    impl Env {
        fn new() -> Self {
            let state = tempfile::tempdir().unwrap();
            let sessions = tempfile::tempdir().unwrap();
            let sync = Sync::with_dirs(state.path().to_path_buf(), sessions.path().to_path_buf());
            Self {
                state,
                sessions,
                sync,
            }
        }

        fn write(&self, pid: u32, state: &str, stamp: u64) {
            std::fs::write(
                self.state.path().join(format!("{pid}.json")),
                state_json(pid, state, STARTED, stamp),
            )
            .unwrap();
        }

        fn name(&self, pid: u32) {
            std::fs::write(self.sessions.path().join(format!("{pid}.json")), REGISTRY).unwrap();
        }

        fn exists(&self, pid: u32) -> bool {
            self.state.path().join(format!("{pid}.json")).exists()
        }
    }

    /// A clock where each pid in `live` runs and started before the session.
    fn live(pids: &[u32]) -> FakeClock {
        FakeClock::new(pids.iter().map(|pid| (Pid::new(*pid), 500)).collect())
    }

    #[test]
    fn the_first_read_loads_the_live_sessions_with_their_names() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "working", 2_000_100);
        env.name(10);
        let mut board = Board::new();
        env.sync
            .initial(&mut board, &mut live(&[10, 20]), NOW)
            .unwrap();
        assert_eq!(board.pids(), vec![Pid::new(10), Pid::new(20)]);
        let first = board.get(Pid::new(10)).unwrap();
        assert_eq!(first.name.as_deref(), Some("issue-697-d1"));
        assert_eq!(first.file.state, PaneState::Idle);
        assert_eq!(board.get(Pid::new(20)).unwrap().name, None);
        assert_eq!(board.changes().count(), 0);
    }

    #[test]
    fn the_first_read_removes_the_file_of_a_stale_session_and_skips_it() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        assert_eq!(board.pids(), vec![Pid::new(10)]);
        assert!(env.exists(10));
        assert!(!env.exists(20));
    }

    #[test]
    fn the_first_read_skips_an_unreadable_file() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        std::fs::write(env.state.path().join("20.json"), "not json").unwrap();
        let mut board = Board::new();
        env.sync
            .initial(&mut board, &mut live(&[10, 20]), NOW)
            .unwrap();
        assert_eq!(board.pids(), vec![Pid::new(10)]);
        assert!(env.exists(20));
    }

    #[test]
    fn the_first_read_of_a_missing_directory_is_an_error() {
        let env = Env::new();
        let sync = Sync::with_dirs(env.state.path().join("none"), env.sessions.path().into());
        let mut board = Board::new();
        let mut clock = FakeClock::new(HashMap::new());
        assert!(sync.initial(&mut board, &mut clock, NOW).is_err());
    }

    const NOW: EpochMillis = EpochMillis::new(9_000_000);

    fn file_event(env: &Env, pid: u32, clock: &mut FakeClock, board: &mut Board) -> Vec<Change> {
        env.sync
            .on_dir_event(board, DirEvent::File(Pid::new(pid)), clock, NOW)
    }

    #[test]
    fn a_file_event_of_a_stopped_process_freezes_the_session_at_the_read() {
        let env = Env::new();
        env.write(7, "idle", 10);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[7]), NOW).unwrap();
        env.write(7, "needs-you", 20);
        let now = EpochMillis::new(5_000);
        let mut clock = live(&[7]).with_stopped(7);
        let changes =
            env.sync
                .on_dir_event(&mut board, DirEvent::File(Pid::new(7)), &mut clock, now);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, Some(ShownState::live(PaneState::Idle)));
        assert_eq!(
            changes[0].new,
            Some(ShownState::frozen(PaneState::NeedsYou))
        );
        assert_eq!(changes[0].at, now);
        assert_eq!(board.get(Pid::new(7)).unwrap().since, now);
    }

    #[test]
    fn a_new_file_gives_one_change_with_no_old_state() {
        let env = Env::new();
        env.write(10, "needs-you", 2_000_000);
        env.name(10);
        let mut board = Board::new();
        let changes = file_event(&env, 10, &mut live(&[10]), &mut board);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, None);
        assert_eq!(changes[0].new, Some(ShownState::live(PaneState::NeedsYou)));
        assert_eq!(changes[0].label, "issue-697-d1");
        assert_eq!(board.pids(), vec![Pid::new(10)]);
    }

    #[test]
    fn a_changed_state_gives_one_change() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        env.write(10, "working", 2_000_500);
        let changes = file_event(&env, 10, &mut live(&[10]), &mut board);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, Some(ShownState::live(PaneState::Idle)));
        assert_eq!(changes[0].new, Some(ShownState::live(PaneState::Working)));
    }

    #[test]
    fn a_removed_file_gives_a_change_with_no_new_state() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        std::fs::remove_file(env.state.path().join("10.json")).unwrap();
        let changes = file_event(&env, 10, &mut live(&[10]), &mut board);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, Some(ShownState::live(PaneState::Idle)));
        assert_eq!(changes[0].new, None);
        assert_eq!(changes[0].at, NOW);
        assert!(board.is_empty());
    }

    #[test]
    fn an_unreadable_file_changes_nothing() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        std::fs::write(env.state.path().join("10.json"), "{\"version\":").unwrap();
        let changes = file_event(&env, 10, &mut live(&[10]), &mut board);
        assert!(changes.is_empty());
        assert_eq!(board.get(Pid::new(10)).unwrap().file.state, PaneState::Idle);
    }

    #[test]
    fn a_file_of_a_stale_process_is_removed_with_its_session() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        env.write(10, "working", 2_000_500);
        let changes = file_event(&env, 10, &mut FakeClock::new(HashMap::new()), &mut board);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].new, None);
        assert!(board.is_empty());
        assert!(!env.exists(10));
    }

    #[test]
    fn a_new_file_of_a_stale_process_gives_no_change() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        let changes = file_event(&env, 10, &mut FakeClock::new(HashMap::new()), &mut board);
        assert!(changes.is_empty());
        assert!(!env.exists(10));
    }

    #[test]
    fn a_rescan_applies_each_file_and_removes_each_session_whose_file_is_gone() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync
            .initial(&mut board, &mut live(&[10, 20, 30]), NOW)
            .unwrap();
        // 10 changes, 20 goes away, 30 is new, 40 holds a bad file.
        env.write(10, "needs-you", 2_000_500);
        std::fs::remove_file(env.state.path().join("20.json")).unwrap();
        env.write(30, "working", 2_000_600);
        std::fs::write(env.state.path().join("40.json"), "bad").unwrap();
        let changes = env.sync.on_dir_event(
            &mut board,
            DirEvent::Rescan,
            &mut live(&[10, 20, 30, 40]),
            NOW,
        );
        let mut summary: Vec<(u32, Option<ShownState>, Option<ShownState>)> = changes
            .iter()
            .map(|change| (change.pid.as_u32(), change.old, change.new))
            .collect();
        summary.sort_by_key(|entry| entry.0);
        assert_eq!(
            summary,
            vec![
                (
                    10,
                    Some(ShownState::live(PaneState::Idle)),
                    Some(ShownState::live(PaneState::NeedsYou))
                ),
                (20, Some(ShownState::live(PaneState::Idle)), None),
                (30, None, Some(ShownState::live(PaneState::Working))),
            ]
        );
        assert_eq!(board.pids(), vec![Pid::new(10), Pid::new(30)]);
    }

    #[test]
    fn a_rescan_keeps_a_session_whose_file_is_unreadable() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        std::fs::write(env.state.path().join("10.json"), "bad").unwrap();
        let changes = env
            .sync
            .on_dir_event(&mut board, DirEvent::Rescan, &mut live(&[10]), NOW);
        assert!(changes.is_empty());
        assert_eq!(board.pids(), vec![Pid::new(10)]);
    }

    #[test]
    fn a_tick_removes_the_stale_sessions_and_keeps_the_live_ones() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "working", 2_000_000);
        let mut board = Board::new();
        env.sync
            .initial(&mut board, &mut live(&[10, 20]), NOW)
            .unwrap();
        let changes = env.sync.on_tick(&mut board, &mut live(&[10]), NOW);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].pid, Pid::new(20));
        assert_eq!(changes[0].old, Some(ShownState::live(PaneState::Working)));
        assert_eq!(changes[0].new, None);
        assert_eq!(changes[0].at, NOW);
        assert_eq!(board.pids(), vec![Pid::new(10)]);
        assert!(env.exists(10));
        assert!(!env.exists(20));
    }

    #[test]
    fn a_tick_with_only_live_sessions_changes_nothing() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        assert!(env
            .sync
            .on_tick(&mut board, &mut live(&[10]), NOW)
            .is_empty());
        assert_eq!(board.pids(), vec![Pid::new(10)]);
    }

    /// A board with the session 10 loaded as `idle` and running.
    fn board_with_idle_ten(env: &Env) -> Board {
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10]), NOW).unwrap();
        board
    }

    #[test]
    fn a_tick_that_sees_a_stop_gives_a_change_and_keeps_the_file() {
        let env = Env::new();
        let mut board = board_with_idle_ten(&env);
        let mut clock = live(&[10]).with_stopped(10);
        let changes = env.sync.on_tick(&mut board, &mut clock, NOW);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].pid, Pid::new(10));
        assert_eq!(changes[0].old, Some(ShownState::live(PaneState::Idle)));
        assert_eq!(changes[0].new, Some(ShownState::frozen(PaneState::Idle)));
        assert_eq!(changes[0].at, NOW);
        let session = board.get(Pid::new(10)).unwrap();
        assert_eq!(session.state(), PaneState::Idle);
        assert!(session.is_frozen());
        assert_eq!(session.file.state, PaneState::Idle);
        assert!(env.exists(10));
    }

    #[test]
    fn a_tick_that_sees_the_process_continue_gives_a_change() {
        let env = Env::new();
        let mut board = board_with_idle_ten(&env);
        let mut stopped = live(&[10]).with_stopped(10);
        env.sync.on_tick(&mut board, &mut stopped, NOW);
        let changes = env.sync.on_tick(&mut board, &mut live(&[10]), NOW);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, Some(ShownState::frozen(PaneState::Idle)));
        assert_eq!(changes[0].new, Some(ShownState::live(PaneState::Idle)));
        let session = board.get(Pid::new(10)).unwrap();
        assert_eq!(session.state(), PaneState::Idle);
        assert!(!session.is_frozen());
    }

    #[test]
    fn a_tick_that_sees_no_new_stop_gives_no_change() {
        let env = Env::new();
        let mut board = board_with_idle_ten(&env);
        let mut clock = live(&[10]).with_stopped(10);
        assert_eq!(env.sync.on_tick(&mut board, &mut clock, NOW).len(), 1);
        assert!(env.sync.on_tick(&mut board, &mut clock, NOW).is_empty());
    }

    // This test passes before the tick reads the stop. It guards the rule that
    // one tick reads the process table once.
    #[test]
    fn a_tick_reads_the_process_table_once() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "working", 2_000_000);
        env.write(30, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync
            .initial(&mut board, &mut live(&[10, 20, 30]), NOW)
            .unwrap();
        let mut clock = live(&[10, 20, 30]).with_stopped(20);
        env.sync.on_tick(&mut board, &mut clock, NOW);
        assert_eq!(clock.reads, 1);
    }

    #[test]
    fn the_first_read_loads_a_stopped_session_as_frozen_since_the_first_read() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "working", 2_000_100);
        let mut board = Board::new();
        let mut clock = live(&[10, 20]).with_stopped(10);
        env.sync.initial(&mut board, &mut clock, NOW).unwrap();
        let stopped = board.get(Pid::new(10)).unwrap();
        assert_eq!(stopped.state(), PaneState::Idle);
        assert!(stopped.is_frozen());
        assert_eq!(stopped.since, NOW);
        let running = board.get(Pid::new(20)).unwrap();
        assert_eq!(running.state(), PaneState::Working);
        assert_eq!(running.since, EpochMillis::new(2_000_100));
        assert_eq!(board.changes().count(), 0);
        assert!(env.exists(10));
        assert!(env.exists(20));
    }

    // This test passes before the first read uses the survey of the stop. It
    // guards the rule that the first read reads the process table once.
    #[test]
    fn the_first_read_reads_the_process_table_once() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "working", 2_000_100);
        let mut board = Board::new();
        let mut clock = live(&[10, 20]);
        env.sync.initial(&mut board, &mut clock, NOW).unwrap();
        assert_eq!(clock.reads, 1);
    }
}
