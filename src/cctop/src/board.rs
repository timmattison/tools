//! The board: the live sessions, and the log of their changes.

use std::collections::{HashMap, VecDeque};

use proctree::Pid;

use crate::state::{EpochMillis, PaneState, StateFile};

/// One live Claude Code session on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The last state file of the session.
    pub file: StateFile,
    /// The name of the session, when the person gave one.
    pub name: Option<String>,
    /// The time when the session entered its current state.
    pub since: EpochMillis,
}

impl Session {
    /// The text that names the session on the screen: its name, else the name
    /// of its directory.
    #[must_use]
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| self.file.directory_name())
    }
}

/// One change in the log: a session appeared, changed state, or ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The time of the change.
    pub at: EpochMillis,
    /// The process id of the session.
    pub pid: Pid,
    /// The label of the session at the time of the change.
    pub label: String,
    /// The state before the change. `None` when the session is new.
    pub old: Option<PaneState>,
    /// The state after the change. `None` when the session ended.
    pub new: Option<PaneState>,
}

impl Change {
    /// Tell if the change makes a session wait for the person. The new state
    /// must be `needs-you`, and the old state must be another one.
    #[must_use]
    pub fn is_needs_you(&self) -> bool {
        self.new == Some(PaneState::NeedsYou) && self.old != Some(PaneState::NeedsYou)
    }
}

/// The number of changes that the log keeps. The log drops the oldest change
/// when it is full.
pub const LOG_CAPACITY: usize = 500;

/// The live sessions, and the log of their changes.
#[derive(Debug, Default)]
pub struct Board {
    sessions: HashMap<Pid, Session>,
    log: VecDeque<Change>,
}

impl Board {
    /// An empty board.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a state file that was read after the start. The function gives
    /// the change that the file makes, or `None` when the board shows no
    /// change of state. The change goes into the log.
    pub fn apply(&mut self, file: StateFile, name: Option<String>) -> Option<Change> {
        let pid = file.pid;
        let stamp = file.stamp;
        let new_state = file.state;
        let session = Session {
            file,
            name,
            since: stamp,
        };
        let label = session.label();
        self.sessions.insert(pid, session);
        let change = Change {
            at: stamp,
            pid,
            label,
            old: None,
            new: Some(new_state),
        };
        self.record(change.clone());
        Some(change)
    }

    /// Put a change at the end of the log. The log drops the oldest change
    /// when it holds more than [`LOG_CAPACITY`] changes.
    fn record(&mut self, change: Change) {
        self.log.push_back(change);
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
    }

    /// The session with the pid, when the board holds it.
    #[must_use]
    pub fn get(&self, pid: Pid) -> Option<&Session> {
        self.sessions.get(&pid)
    }

    /// The changes in the log, newest first.
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.log.iter().rev()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state file with the given values and no Zellij place.
    fn file(pid: u32, state: PaneState, started_at: u64, stamp: u64) -> StateFile {
        StateFile {
            pid: Pid::new(pid),
            session_id: format!("session-{pid}"),
            started_at: EpochMillis::new(started_at),
            state,
            stamp: EpochMillis::new(stamp),
            cwd: std::path::PathBuf::from("/path/to/worktree"),
            zellij_session: None,
            zellij_pane: None,
        }
    }

    #[test]
    fn the_label_is_the_name_else_the_directory_name() {
        let mut session = Session {
            file: file(1, PaneState::Idle, 100, 200),
            name: None,
            since: EpochMillis::new(200),
        };
        assert_eq!(session.label(), "worktree");
        session.name = Some("fix the bug".to_string());
        assert_eq!(session.label(), "fix the bug");
    }

    fn change(old: Option<PaneState>, new: Option<PaneState>) -> Change {
        Change {
            at: EpochMillis::new(1),
            pid: Pid::new(1),
            label: "x".to_string(),
            old,
            new,
        }
    }

    #[test]
    fn a_change_is_needs_you_only_when_the_session_enters_needs_you() {
        use PaneState::{Idle, NeedsYou};
        assert!(change(Some(Idle), Some(NeedsYou)).is_needs_you());
        assert!(change(None, Some(NeedsYou)).is_needs_you());
        assert!(!change(Some(NeedsYou), Some(NeedsYou)).is_needs_you());
        assert!(!change(Some(NeedsYou), Some(Idle)).is_needs_you());
        assert!(!change(Some(NeedsYou), None).is_needs_you());
    }

    #[test]
    fn apply_inserts_an_unknown_pid_and_logs_a_new_session() {
        let mut board = Board::new();
        let change = board
            .apply(
                file(7, PaneState::Working, 100, 500),
                Some("named".to_string()),
            )
            .expect("a new session is a change");
        assert_eq!(
            change,
            Change {
                at: EpochMillis::new(500),
                pid: Pid::new(7),
                label: "named".to_string(),
                old: None,
                new: Some(PaneState::Working),
            }
        );
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert_eq!(session.since, EpochMillis::new(500));
        assert_eq!(session.name.as_deref(), Some("named"));
        assert_eq!(board.changes().cloned().collect::<Vec<_>>(), vec![change]);
    }
}
