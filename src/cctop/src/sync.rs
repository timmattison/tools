//! The link between the state directory and the board. The module reads the
//! files, finds the stale sessions, and tells the board what changed.

use std::path::{Path, PathBuf};

use crate::board::Board;
use crate::liveness::ProcessClock;
use crate::paths::ClaudeDirs;

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
        _board: &mut Board,
        _clock: &mut dyn ProcessClock,
    ) -> std::io::Result<()> {
        Ok(())
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
}
