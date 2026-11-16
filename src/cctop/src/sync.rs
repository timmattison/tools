//! The link between the state directory and the board. The module reads the
//! files, finds the stale sessions, and tells the board what changed.

use std::path::{Path, PathBuf};

use proctree::Pid;

use crate::board::{Board, Change};
use crate::liveness::{ProcessClock, stale_pids};
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
    /// The function logs no change. It removes the file of each stale
    /// session, and it does not load that session. It skips a file that it
    /// cannot read.
    ///
    /// # Errors
    ///
    /// Returns the error of the read of the state directory.
    pub fn initial(
        &self,
        board: &mut Board,
        clock: &mut dyn ProcessClock,
    ) -> std::io::Result<()> {
        let files: Vec<StateFile> = store::read_dir(&self.state_dir)?
            .into_iter()
            .filter_map(|(_, outcome)| match outcome {
                ReadOutcome::Present(file) => Some(file),
                ReadOutcome::Gone | ReadOutcome::Unreadable(_) => None,
            })
            .collect();
        let sessions: Vec<(Pid, EpochMillis)> =
            files.iter().map(|file| (file.pid, file.started_at)).collect();
        let stale = stale_pids(&sessions, clock);
        for file in files {
            if stale.contains(&file.pid) {
                // A new process can have reused the pid since the first read. Its
                // file stays, and its own write event loads it.
                let _removed = store::remove_if_stale(&self.state_dir, file.pid, clock);
                continue;
            }
            let name = registry::session_name(&self.sessions_dir, file.pid);
            board.load(file, name);
        }
        Ok(())
    }

    /// Apply one directory event to `board`. Return the changes that it made.
    ///
    /// `File(pid)` reads the file of that pid. A good file of a live process
    /// goes to the board. A good file of a stale process goes away, with its
    /// session. A missing file removes the session. A file that cannot be read
    /// changes nothing. `Rescan` does this for each file in the directory, and
    /// it removes each session whose file is gone.
    pub fn on_dir_event(
        &self,
        _board: &mut Board,
        _event: DirEvent,
        _clock: &mut dyn ProcessClock,
        _now: EpochMillis,
    ) -> Vec<Change> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proctree::Pid;

    use super::*;
    use crate::state::PaneState;
    use crate::store::test_support::{FakeClock, state_json};

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
        FakeClock(pids.iter().map(|pid| (Pid::new(*pid), 500)).collect())
    }

    #[test]
    fn the_first_read_loads_the_live_sessions_with_their_names() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        env.write(20, "working", 2_000_100);
        env.name(10);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10, 20])).unwrap();
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
        env.sync.initial(&mut board, &mut live(&[10])).unwrap();
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
        env.sync.initial(&mut board, &mut live(&[10, 20])).unwrap();
        assert_eq!(board.pids(), vec![Pid::new(10)]);
        assert!(env.exists(20));
    }

    #[test]
    fn the_first_read_of_a_missing_directory_is_an_error() {
        let env = Env::new();
        let sync = Sync::with_dirs(env.state.path().join("none"), env.sessions.path().into());
        let mut board = Board::new();
        let mut clock = FakeClock(HashMap::new());
        assert!(sync.initial(&mut board, &mut clock).is_err());
    }

    const NOW: EpochMillis = EpochMillis::new(9_000_000);

    fn file_event(env: &Env, pid: u32, clock: &mut FakeClock, board: &mut Board) -> Vec<Change> {
        env.sync
            .on_dir_event(board, DirEvent::File(Pid::new(pid)), clock, NOW)
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
        assert_eq!(changes[0].new, Some(PaneState::NeedsYou));
        assert_eq!(changes[0].label, "issue-697-d1");
        assert_eq!(board.pids(), vec![Pid::new(10)]);
    }

    #[test]
    fn a_changed_state_gives_one_change() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10])).unwrap();
        env.write(10, "working", 2_000_500);
        let changes = file_event(&env, 10, &mut live(&[10]), &mut board);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, Some(PaneState::Idle));
        assert_eq!(changes[0].new, Some(PaneState::Working));
    }

    #[test]
    fn a_removed_file_gives_a_change_with_no_new_state() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10])).unwrap();
        std::fs::remove_file(env.state.path().join("10.json")).unwrap();
        let changes = file_event(&env, 10, &mut live(&[10]), &mut board);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old, Some(PaneState::Idle));
        assert_eq!(changes[0].new, None);
        assert_eq!(changes[0].at, NOW);
        assert!(board.is_empty());
    }

    #[test]
    fn an_unreadable_file_changes_nothing() {
        let env = Env::new();
        env.write(10, "idle", 2_000_000);
        let mut board = Board::new();
        env.sync.initial(&mut board, &mut live(&[10])).unwrap();
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
        env.sync.initial(&mut board, &mut live(&[10])).unwrap();
        env.write(10, "working", 2_000_500);
        let changes = file_event(&env, 10, &mut FakeClock(HashMap::new()), &mut board);
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
        let changes = file_event(&env, 10, &mut FakeClock(HashMap::new()), &mut board);
        assert!(changes.is_empty());
        assert!(!env.exists(10));
    }
}
