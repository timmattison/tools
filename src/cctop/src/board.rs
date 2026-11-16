//! The board: the live sessions, and the log of their changes.

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
}
