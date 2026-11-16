//! The board: the live sessions, and the log of their changes.

use crate::state::{EpochMillis, StateFile};

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
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PaneState;
    use proctree::Pid;

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
}
