//! The state of the screen: the board, the selected row, and the status text.
//! The module turns a key into an [`Action`] for the main loop.

use proctree::Pid;

use crate::board::Board;

/// What the main loop does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Do nothing.
    Nothing,
}

/// The state of the screen.
#[derive(Debug, Default)]
pub struct App {
    board: Board,
    selected: Option<Pid>,
    last_index: usize,
    status: Option<String>,
}

impl App {
    /// An app with an empty board.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The board of sessions.
    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The board of sessions, to change.
    pub fn board_mut(&mut self) -> &mut Board {
        &mut self.board
    }

    /// The index of the selected row in [`Board::rows`]. The function gives
    /// the row of the selected pid. When that session is gone, it gives the
    /// row at the last known index, or the last row when the board is
    /// shorter. It gives `None` when the board is empty.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let rows = self.board.rows();
        let last = rows.len().checked_sub(1)?;
        let by_pid = self
            .selected
            .and_then(|pid| rows.iter().position(|session| session.file.pid == pid));
        Some(by_pid.unwrap_or_else(|| self.last_index.min(last)))
    }

    /// The status text, when there is one.
    #[must_use]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Set the status text.
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(text.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{EpochMillis, PaneState, StateFile};

    /// A state file in the directory `/path/<name>` with no Zellij place.
    fn file(pid: u32, name: &str, state: PaneState, stamp: u64) -> StateFile {
        StateFile {
            pid: Pid::new(pid),
            session_id: format!("session-{pid}"),
            started_at: EpochMillis::new(1),
            state,
            stamp: EpochMillis::new(stamp),
            cwd: std::path::PathBuf::from(format!("/path/{name}")),
            zellij_session: None,
            zellij_pane: None,
        }
    }

    /// An app that holds the given sessions as `(pid, state)`, loaded at
    /// stamp `pid`.
    fn app_with(sessions: &[(u32, PaneState)]) -> App {
        let mut app = App::new();
        for &(pid, state) in sessions {
            app.board_mut()
                .load(file(pid, &format!("dir{pid}"), state, u64::from(pid)), None);
        }
        app
    }

    #[test]
    fn an_empty_board_selects_no_row() {
        assert_eq!(App::new().selected_index(), None);
    }

    #[test]
    fn the_first_row_is_selected_before_any_choice() {
        let app = app_with(&[(10, PaneState::Working), (11, PaneState::Idle)]);
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn the_selected_session_stays_selected_when_the_order_changes() {
        let mut app = app_with(&[
            (10, PaneState::Idle),
            (11, PaneState::Working),
            (12, PaneState::Working),
        ]);
        app.selected = Some(Pid::new(12));
        assert_eq!(app.selected_index(), Some(2));
        // Session 11 becomes `needs-you`, so it sorts above session 10.
        app.board_mut()
            .apply(file(11, "dir11", PaneState::NeedsYou, 100), None);
        assert_eq!(app.selected_index(), Some(2));
        // Session 12 becomes `needs-you` and moves to row 1.
        app.board_mut()
            .apply(file(12, "dir12", PaneState::NeedsYou, 101), None);
        assert_eq!(app.selected_index(), Some(1));
    }

    #[test]
    fn a_removed_selection_gives_the_row_at_the_same_index() {
        let mut app = app_with(&[
            (10, PaneState::Idle),
            (11, PaneState::Idle),
            (12, PaneState::Idle),
        ]);
        app.selected = Some(Pid::new(11));
        app.last_index = 1;
        app.board_mut().remove(Pid::new(11), EpochMillis::new(500));
        assert_eq!(app.selected_index(), Some(1));
        assert_eq!(
            app.board().rows()[1].file.pid,
            Pid::new(12),
            "row 1 now holds the next session"
        );
    }

    #[test]
    fn a_removed_last_selection_is_clamped_to_the_last_row() {
        let mut app = app_with(&[(10, PaneState::Idle), (11, PaneState::Idle)]);
        app.selected = Some(Pid::new(11));
        app.last_index = 1;
        app.board_mut().remove(Pid::new(11), EpochMillis::new(500));
        assert_eq!(app.selected_index(), Some(0));
    }
}
