//! The state of the screen: the board, the selected row, and the status text.
//! The module turns a key into an [`Action`] for the main loop.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use proctree::Pid;

use crate::board::{Board, HandledToggle};
use crate::focus::FocusTarget;
use crate::state::{EpochMillis, PaneState};

/// What the main loop does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Do nothing.
    Nothing,
    /// Leave the program.
    Quit,
    /// Focus the terminal of a session.
    Focus(FocusTarget),
}

/// The time in milliseconds that a key press holds a jump. A jump to a
/// session that needs you waits until this time passes after the last key
/// press.
pub const JUMP_HOLD_MS: u64 = 2_000;

/// The number of session rows that one Page Up or Page Down moves the
/// selection. A page is at least 1 row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSize(std::num::NonZeroUsize);

impl PageSize {
    /// A page of `rows` rows. A page of 0 rows becomes a page of 1 row.
    #[must_use]
    pub fn new(rows: usize) -> Self {
        Self(std::num::NonZeroUsize::new(rows).unwrap_or(std::num::NonZeroUsize::MIN))
    }

    /// The number of rows of the page. It is at least 1.
    #[must_use]
    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for PageSize {
    /// A page of 1 row.
    fn default() -> Self {
        Self(std::num::NonZeroUsize::MIN)
    }
}

/// The state of the screen.
#[derive(Debug, Default)]
pub struct App {
    board: Board,
    selected: Option<Pid>,
    last_index: usize,
    status: Option<String>,
    last_key: Option<EpochMillis>,
    held: Vec<Pid>,
    page_size: PageSize,
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

    /// Set the number of rows that Page Up and Page Down move the selection.
    pub fn set_page_size(&mut self, page: PageSize) {
        self.page_size = page;
    }

    /// Handle one key. The function moves the selection, changes the status
    /// text (a move key clears it), and tells the main loop what to do next.
    /// The argument `now` is the time of the key. A key press holds a jump
    /// for [`JUMP_HOLD_MS`]. A key release does nothing and holds no jump.
    pub fn handle_key(&mut self, key: KeyEvent, now: EpochMillis) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::Nothing;
        }
        self.last_key = Some(now);
        self.sync_selection();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Enter => return self.focus_selected(),
            KeyCode::Char('n') => return self.focus_next_needs_you(),
            KeyCode::Char('h' | 'H') => self.toggle_handled_selected(),
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Quit;
            }
            _ => {}
        }
        Action::Nothing
    }

    /// Tell the app that the sessions `pids` changed to `needs-you` at `now`.
    /// The app holds them until [`JUMP_HOLD_MS`] passes after the last key
    /// press. Then it selects the one that sorts first on the board. If the
    /// clock moves back to a time before the last key press, the hold starts
    /// again at the new time.
    pub fn on_needs_you(&mut self, pids: &[Pid], now: EpochMillis) {
        self.held.extend_from_slice(pids);
        self.flush_held(now);
    }

    /// Tell the app that time passed. The app then ends a held jump when the
    /// hold is over at `now`.
    pub fn on_tick(&mut self, now: EpochMillis) {
        self.flush_held(now);
    }

    /// Select the first row of the board among the held sessions that are
    /// still in the state `needs-you` and have no handled mark. Then
    /// forget all held sessions. A key press less than [`JUMP_HOLD_MS`]
    /// before `now` keeps the held sessions and selects nothing. A key press
    /// time later than `now` means the clock moved back. The function then
    /// sets the time of the key to `now`, so the hold ends [`JUMP_HOLD_MS`]
    /// after that.
    fn flush_held(&mut self, now: EpochMillis) {
        if self.last_key.is_some_and(|key| key > now) {
            self.last_key = Some(now);
        }
        let held_by_key = self
            .last_key
            .is_some_and(|key| now.saturating_since(key) < JUMP_HOLD_MS);
        if self.held.is_empty() || held_by_key {
            return;
        }
        let first = self.board.rows().iter().position(|session| {
            self.held.contains(&session.file.pid)
                && session.file.state == PaneState::NeedsYou
                && !session.is_handled()
        });
        if let Some(index) = first {
            self.select(index);
        }
        self.held.clear();
    }

    /// Put a handled mark on the selected session, or remove its mark. A new
    /// mark moves the session to the bottom of its state. The cursor then
    /// selects the first row at or below the old index of the marked row that
    /// has no mark and has a state that takes a mark. If no such row exists,
    /// the cursor stays on the marked session. A removed mark moves the
    /// session up, and the cursor goes with the session. A session whose state
    /// takes no mark keeps its row and the cursor, and the status text tells
    /// why.
    fn toggle_handled_selected(&mut self) {
        let selected = self.selected_index().and_then(|index| {
            let session = self.board.rows().get(index).copied()?;
            Some((index, session.file.pid, session.label()))
        });
        let Some((index, pid, label)) = selected else {
            self.status = Some("No session to mark".to_string());
            return;
        };
        match self.board.toggle_handled(pid) {
            Some(HandledToggle::Marked) => {
                let next = self
                    .board
                    .rows()
                    .iter()
                    .skip(index)
                    .position(|session| {
                        !session.is_handled() && session.file.state.takes_handled_mark()
                    })
                    .map(|offset| index + offset);
                match next {
                    Some(next) => self.select(next),
                    None => self.sync_selection(),
                }
                self.status = Some(format!("Marked {label} as handled"));
            }
            Some(HandledToggle::Unmarked) => {
                self.sync_selection();
                self.status = Some(format!("Removed the handled mark from {label}"));
            }
            Some(HandledToggle::Refused) => {
                self.status = Some(format!(
                    "Cannot mark {label} as handled: only {} sessions take a mark",
                    states_that_take_a_mark()
                ));
            }
            None => {}
        }
    }

    /// Select the next session in the state `needs-you` that has no handled
    /// mark, and ask the main loop to focus it. The search starts after the
    /// selected row and wraps around, so the selected row counts last.
    fn focus_next_needs_you(&mut self) -> Action {
        let rows = self.board.rows();
        let count = rows.len();
        let current = self.selected_index().unwrap_or(0);
        let found = (1..=count)
            .map(|offset| (current + offset) % count)
            .find(|&index| {
                rows[index].file.state == PaneState::NeedsYou && !rows[index].is_handled()
            });
        let Some(index) = found else {
            self.status = Some("No session needs you".to_string());
            return Action::Nothing;
        };
        self.select(index);
        self.focus_selected()
    }

    /// Ask the main loop to focus the selected session.
    fn focus_selected(&mut self) -> Action {
        let target = self.selected_index().and_then(|index| {
            self.board
                .rows()
                .get(index)
                .map(|session| FocusTarget::of(session))
        });
        if let Some(target) = target {
            self.status = Some(format!("Focusing {}", target.label));
            Action::Focus(target)
        } else {
            self.status = Some("No session to focus".to_string());
            Action::Nothing
        }
    }

    /// Store the row that [`App::selected_index`] gives, so a later change of
    /// the board does not move the selection.
    fn sync_selection(&mut self) {
        let Some(index) = self.selected_index() else {
            return;
        };
        self.last_index = index;
        self.selected = self.board.rows().get(index).map(|session| session.file.pid);
    }

    /// Clear the status text, then select the row `step` rows away, and stay
    /// inside the board. The status clears also when the selection cannot
    /// move.
    fn move_selection(&mut self, step: isize) {
        self.status = None;
        let Some(index) = self.selected_index() else {
            return;
        };
        let last = self.board.len().saturating_sub(1);
        let target = index.saturating_add_signed(step).min(last);
        self.select(target);
    }

    /// Select the row at `index`.
    fn select(&mut self, index: usize) {
        self.last_index = index;
        self.selected = self.board.rows().get(index).map(|session| session.file.pid);
    }
}

/// The labels of the states that take a handled mark, in the sort order of
/// the rows, joined by ` and `. The text is `needs-you and idle`.
fn states_that_take_a_mark() -> String {
    let mut states: Vec<PaneState> = PaneState::ALL
        .into_iter()
        .filter(|state| state.takes_handled_mark())
        .collect();
    states.sort_by_key(|state| state.rank());
    states
        .into_iter()
        .map(PaneState::label)
        .collect::<Vec<_>>()
        .join(" and ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    use crate::state::{EpochMillis, PaneState, StateFile};

    /// The time that every key of these tests carries.
    const KEY_TIME: EpochMillis = EpochMillis::new(1_000);

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

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn three_rows() -> App {
        app_with(&[
            (10, PaneState::Idle),
            (11, PaneState::Idle),
            (12, PaneState::Idle),
        ])
    }

    #[test]
    fn j_and_down_move_down_and_stop_at_the_last_row() {
        let mut app = three_rows();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('j')), KEY_TIME),
            Action::Nothing
        );
        assert_eq!(app.selected_index(), Some(1));
        app.handle_key(press(KeyCode::Down), KEY_TIME);
        assert_eq!(app.selected_index(), Some(2));
        app.handle_key(press(KeyCode::Down), KEY_TIME);
        assert_eq!(app.selected_index(), Some(2), "no wrap at the end");
    }

    #[test]
    fn k_and_up_move_up_and_stop_at_the_first_row() {
        let mut app = three_rows();
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('k')), KEY_TIME);
        assert_eq!(app.selected_index(), Some(1));
        app.handle_key(press(KeyCode::Up), KEY_TIME);
        assert_eq!(app.selected_index(), Some(0));
        app.handle_key(press(KeyCode::Up), KEY_TIME);
        assert_eq!(app.selected_index(), Some(0), "no wrap at the start");
    }

    fn ten_idle_rows() -> App {
        let rows: Vec<(u32, PaneState)> = (10..20).map(|pid| (pid, PaneState::Idle)).collect();
        app_with(&rows)
    }

    #[test]
    fn page_down_and_page_up_move_by_the_page_size_and_stop_at_the_ends() {
        let mut app = ten_idle_rows();
        app.set_page_size(PageSize::new(4));
        for expected in [4, 8, 9, 9] {
            app.handle_key(press(KeyCode::PageDown), KEY_TIME);
            assert_eq!(
                app.selected_index(),
                Some(expected),
                "Page Down must select row {expected}"
            );
        }
        for expected in [5, 1, 0, 0] {
            app.handle_key(press(KeyCode::PageUp), KEY_TIME);
            assert_eq!(
                app.selected_index(),
                Some(expected),
                "Page Up must select row {expected}"
            );
        }
    }

    #[test]
    fn home_selects_the_first_row_and_end_selects_the_last_row() {
        let mut app = ten_idle_rows();
        for _ in 0..4 {
            app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        }
        assert_eq!(app.selected_index(), Some(4));
        app.handle_key(press(KeyCode::End), KEY_TIME);
        assert_eq!(app.selected_index(), Some(9), "End must select the last row");
        for _ in 0..5 {
            app.handle_key(press(KeyCode::Char('k')), KEY_TIME);
        }
        assert_eq!(app.selected_index(), Some(4));
        app.handle_key(press(KeyCode::Home), KEY_TIME);
        assert_eq!(app.selected_index(), Some(0), "Home must select the first row");
    }

    #[test]
    fn a_move_on_an_empty_board_does_nothing() {
        let mut app = App::new();
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('k')), KEY_TIME);
        for code in [
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::End,
        ] {
            app.handle_key(press(code), KEY_TIME);
        }
        assert_eq!(app.selected_index(), None);
    }

    #[test]
    fn a_key_release_does_nothing() {
        let mut app = three_rows();
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(app.handle_key(release, KEY_TIME), Action::Nothing);
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn enter_focuses_the_selected_session() {
        let mut app = three_rows();
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        let expected = FocusTarget::of(app.board().get(Pid::new(11)).expect("session 11 exists"));
        assert_eq!(
            app.handle_key(press(KeyCode::Enter), KEY_TIME),
            Action::Focus(expected)
        );
        assert_eq!(app.status(), Some("Focusing dir11"));
    }

    #[test]
    fn enter_names_the_session_by_its_registry_name() {
        let mut app = App::new();
        app.board_mut().load(
            file(10, "dir10", PaneState::Idle, 10),
            Some("my-name".to_string()),
        );
        app.handle_key(press(KeyCode::Enter), KEY_TIME);
        assert_eq!(app.status(), Some("Focusing my-name"));
    }

    #[test]
    fn a_move_key_clears_the_status() {
        for code in [
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::End,
        ] {
            let mut app = three_rows();
            app.handle_key(press(KeyCode::Enter), KEY_TIME);
            assert_eq!(app.status(), Some("Focusing dir10"));
            app.handle_key(press(code), KEY_TIME);
            assert_eq!(app.status(), None, "key {code:?} must clear the status");
        }
    }

    #[test]
    fn a_move_key_on_an_empty_board_clears_the_status() {
        for code in [
            KeyCode::Char('j'),
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::End,
        ] {
            let mut app = App::new();
            app.set_status("No session needs you");
            app.handle_key(press(code), KEY_TIME);
            assert_eq!(
                app.status(),
                None,
                "key {code:?} must clear the status on an empty board"
            );
        }
    }

    #[test]
    fn enter_on_an_empty_board_has_nothing_to_focus() {
        let mut app = App::new();
        assert_eq!(
            app.handle_key(press(KeyCode::Enter), KEY_TIME),
            Action::Nothing
        );
        assert_eq!(app.status(), Some("No session to focus"));
    }

    fn focus_pid(action: &Action) -> Pid {
        match action {
            Action::Focus(target) => target.pid,
            other => panic!("expected a focus action, got {other:?}"),
        }
    }

    #[test]
    fn n_selects_the_next_needs_you_row_and_wraps() {
        let mut app = app_with(&[
            (10, PaneState::NeedsYou),
            (11, PaneState::NeedsYou),
            (12, PaneState::Working),
        ]);
        let first = app.handle_key(press(KeyCode::Char('n')), KEY_TIME);
        assert_eq!(focus_pid(&first), Pid::new(11));
        assert_eq!(app.selected_index(), Some(1));
        assert_eq!(app.status(), Some("Focusing dir11"));
        let second = app.handle_key(press(KeyCode::Char('n')), KEY_TIME);
        assert_eq!(focus_pid(&second), Pid::new(10));
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn n_skips_rows_that_do_not_need_you() {
        let mut app = app_with(&[
            (10, PaneState::NeedsYou),
            (11, PaneState::Working),
            (12, PaneState::Working),
        ]);
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        let action = app.handle_key(press(KeyCode::Char('n')), KEY_TIME);
        assert_eq!(focus_pid(&action), Pid::new(10));
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn n_on_the_only_needs_you_row_gives_that_row_again() {
        let mut app = app_with(&[(10, PaneState::NeedsYou), (11, PaneState::Idle)]);
        let action = app.handle_key(press(KeyCode::Char('n')), KEY_TIME);
        assert_eq!(focus_pid(&action), Pid::new(10));
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn n_with_no_needs_you_session_does_nothing() {
        let mut app = three_rows();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('n')), KEY_TIME),
            Action::Nothing
        );
        assert_eq!(app.status(), Some("No session needs you"));
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn n_skips_a_handled_needs_you_session() {
        let mut app = app_with(&[
            (10, PaneState::NeedsYou),
            (11, PaneState::NeedsYou),
            (12, PaneState::Working),
        ]);
        app.board_mut().toggle_handled(Pid::new(10));
        // The rows are 11, 10 (handled), 12, and the cursor is on row 0.
        let action = app.handle_key(press(KeyCode::Char('n')), KEY_TIME);
        assert_eq!(focus_pid(&action), Pid::new(11));
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn n_with_only_handled_needs_you_sessions_does_nothing() {
        let mut app = app_with(&[(10, PaneState::NeedsYou), (11, PaneState::Idle)]);
        app.board_mut().toggle_handled(Pid::new(10));
        assert_eq!(
            app.handle_key(press(KeyCode::Char('n')), KEY_TIME),
            Action::Nothing
        );
        assert_eq!(app.status(), Some("No session needs you"));
    }

    /// The pid of the selected row.
    fn selected_pid(app: &App) -> Option<Pid> {
        let index = app.selected_index()?;
        app.board()
            .rows()
            .get(index)
            .map(|session| session.file.pid)
    }

    /// The pids of the rows, in the order of the screen.
    fn row_order(app: &App) -> Vec<u32> {
        app.board()
            .rows()
            .iter()
            .map(|session| session.file.pid.as_u32())
            .collect()
    }

    /// Tell if the session with the pid has a handled mark.
    fn is_handled(app: &App, pid: u32) -> bool {
        app.board()
            .get(Pid::new(pid))
            .is_some_and(crate::board::Session::is_handled)
    }

    #[test]
    fn h_marks_the_selected_session_and_selects_the_next_unmarked_row() {
        let mut app = three_rows();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('h')), KEY_TIME),
            Action::Nothing
        );
        assert!(is_handled(&app, 10));
        assert_eq!(row_order(&app), vec![11, 12, 10]);
        assert_eq!(app.selected_index(), Some(0));
        assert_eq!(selected_pid(&app), Some(Pid::new(11)));
        assert_eq!(app.status(), Some("Marked dir10 as handled"));
    }

    #[test]
    fn h_selects_the_first_unmarked_row_at_or_below_the_old_index() {
        let mut app = app_with(&[(10, PaneState::NeedsYou), (20, PaneState::Idle)]);
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        assert!(is_handled(&app, 10));
        assert_eq!(selected_pid(&app), Some(Pid::new(20)));
    }

    #[test]
    fn h_skips_a_row_that_has_a_mark_already() {
        let mut app = app_with(&[
            (10, PaneState::NeedsYou),
            (11, PaneState::NeedsYou),
            (20, PaneState::Idle),
        ]);
        app.board_mut().toggle_handled(Pid::new(11));
        assert_eq!(row_order(&app), vec![10, 11, 20]);
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        assert_eq!(row_order(&app), vec![11, 10, 20]);
        assert_eq!(selected_pid(&app), Some(Pid::new(20)));
    }

    #[test]
    fn h_stays_on_the_marked_row_when_no_unmarked_row_is_below() {
        let mut app = app_with(&[(10, PaneState::NeedsYou), (11, PaneState::NeedsYou)]);
        app.board_mut().toggle_handled(Pid::new(11));
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        assert_eq!(row_order(&app), vec![11, 10]);
        assert!(is_handled(&app, 10));
        assert!(is_handled(&app, 11));
        assert_eq!(selected_pid(&app), Some(Pid::new(10)));
    }

    #[test]
    fn a_second_h_marks_the_next_session() {
        let mut app = three_rows();
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        assert!(is_handled(&app, 10));
        assert!(is_handled(&app, 11));
        assert_eq!(row_order(&app), vec![12, 10, 11]);
        assert_eq!(selected_pid(&app), Some(Pid::new(12)));
    }

    #[test]
    fn a_shifted_h_also_marks_the_selected_session() {
        let mut app = three_rows();
        let shifted = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);
        assert_eq!(app.handle_key(shifted, KEY_TIME), Action::Nothing);
        assert!(is_handled(&app, 10));
    }

    #[test]
    fn h_on_a_handled_session_removes_the_mark_and_the_cursor_follows_it() {
        let mut app = three_rows();
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
        assert_eq!(selected_pid(&app), Some(Pid::new(10)));
        app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
        assert!(!is_handled(&app, 10));
        assert_eq!(row_order(&app), vec![10, 11, 12]);
        assert_eq!(selected_pid(&app), Some(Pid::new(10)));
        assert_eq!(app.status(), Some("Removed the handled mark from dir10"));
    }

    #[test]
    fn h_on_a_session_that_does_not_wait_for_the_person_marks_nothing() {
        for state in [PaneState::Working, PaneState::Agents, PaneState::Shells] {
            let mut app = app_with(&[(10, PaneState::NeedsYou), (11, state), (12, state)]);
            app.handle_key(press(KeyCode::Char('j')), KEY_TIME);
            assert_eq!(selected_pid(&app), Some(Pid::new(11)), "{state:?}");
            assert_eq!(
                app.handle_key(press(KeyCode::Char('h')), KEY_TIME),
                Action::Nothing,
                "{state:?}"
            );
            assert!(!is_handled(&app, 11), "{state:?}");
            assert_eq!(row_order(&app), vec![10, 11, 12], "{state:?}");
            assert_eq!(selected_pid(&app), Some(Pid::new(11)), "{state:?}");
            assert_eq!(
                app.status(),
                Some("Cannot mark dir11 as handled: only needs-you and idle sessions take a mark"),
                "{state:?}"
            );
        }
    }

    #[test]
    fn after_a_mark_the_cursor_skips_a_session_that_takes_no_mark() {
        for state in [PaneState::Working, PaneState::Agents, PaneState::Shells] {
            let mut app = app_with(&[
                (10, PaneState::NeedsYou),
                (11, PaneState::Idle),
                (12, state),
            ]);
            assert_eq!(selected_pid(&app), Some(Pid::new(10)), "{state:?}");
            app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
            assert!(is_handled(&app, 10), "{state:?}");
            assert_eq!(selected_pid(&app), Some(Pid::new(11)), "{state:?}");
            app.handle_key(press(KeyCode::Char('h')), KEY_TIME);
            assert!(is_handled(&app, 11), "{state:?}");
            assert_eq!(selected_pid(&app), Some(Pid::new(11)), "{state:?}");
            assert_eq!(app.status(), Some("Marked dir11 as handled"), "{state:?}");
        }
    }

    #[test]
    fn h_on_an_empty_board_marks_nothing() {
        let mut app = App::new();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('h')), KEY_TIME),
            Action::Nothing
        );
        assert_eq!(app.status(), Some("No session to mark"));
    }

    #[test]
    fn q_and_control_c_quit() {
        let mut app = three_rows();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('q')), KEY_TIME),
            Action::Quit
        );
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.handle_key(control_c, KEY_TIME), Action::Quit);
    }

    #[test]
    fn a_plain_c_does_nothing() {
        let mut app = three_rows();
        assert_eq!(
            app.handle_key(press(KeyCode::Char('c')), KEY_TIME),
            Action::Nothing
        );
        assert_eq!(app.status(), None);
    }
}
