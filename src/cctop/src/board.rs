//! The board: the live sessions, and the log of their changes.

use std::collections::{HashMap, VecDeque};

use proctree::Pid;

use crate::liveness::RunStatus;
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
    /// Tell if the process of the session runs or is stopped.
    pub run: RunStatus,
    /// The handled mark of the session, when the person gave it one. A change
    /// of state removes the mark.
    pub handled: Option<HandledMark>,
}

/// The place of one handled mark in the order of the marks. A later mark has
/// a higher place, thus its row sorts lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandledMark(u64);

/// The result of [`Board::toggle_handled`] for a session on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandledToggle {
    /// The session now has a handled mark.
    Marked,
    /// The function removed the handled mark of the session.
    Unmarked,
    /// The state of the session takes no handled mark, see
    /// [`PaneState::takes_handled_mark`]. The board does not change.
    Refused,
}

/// The text that follows the state of a handled session on the screen.
pub const HANDLED_SUFFIX: &str = " - handled";

impl Session {
    /// The text that names the session on the screen: its name, else the name
    /// of its directory.
    #[must_use]
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| self.file.directory_name())
    }

    /// The state that the screen shows. A stopped process shows `frozen`,
    /// whatever the file says. Else it is the state of the file.
    #[must_use]
    pub fn state(&self) -> PaneState {
        match self.run {
            RunStatus::Stopped => PaneState::Frozen,
            RunStatus::Running => self.file.state,
        }
    }

    /// Tell if the session has a handled mark.
    #[must_use]
    pub fn is_handled(&self) -> bool {
        self.handled.is_some()
    }

    /// The state of the session for the screen. A handled session adds
    /// [`HANDLED_SUFFIX`], for example `idle - handled`.
    #[must_use]
    pub fn state_label(&self) -> String {
        let state = self.state().label();
        if self.is_handled() {
            format!("{state}{HANDLED_SUFFIX}")
        } else {
            state.to_string()
        }
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

/// The number of sessions in one state, split by the handled mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateCount {
    /// The state that the count is about.
    pub state: PaneState,
    /// The sessions in the state with no handled mark.
    pub unmarked: usize,
    /// The sessions in the state with a handled mark.
    pub handled: usize,
}

/// The live sessions, and the log of their changes.
#[derive(Debug, Default)]
pub struct Board {
    sessions: HashMap<Pid, Session>,
    log: VecDeque<Change>,
    next_mark: u64,
}

impl Board {
    /// An empty board.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Put a state file on the board at the first read. The function inserts
    /// the session, or replaces the one with the same pid. The state time is
    /// the stamp of the file. The log gets no entry.
    pub fn load(&mut self, file: StateFile, name: Option<String>) {
        let since = file.stamp;
        self.sessions.insert(
            file.pid,
            Session {
                file,
                name,
                since,
                run: RunStatus::Running,
                handled: None,
            },
        );
    }

    /// Put a state file of a stopped process on the board at the first read.
    pub fn load_stopped(&mut self, file: StateFile, name: Option<String>, now: EpochMillis) {
        let _ = now;
        self.load(file, name);
    }

    /// Apply a state file that was read after the start. The function gives
    /// the change that the file makes, or `None` when the board shows no
    /// change of state. The change goes into the log. A session in the same
    /// state keeps its handled mark, and a change of state removes it.
    pub fn apply(&mut self, file: StateFile, name: Option<String>) -> Option<Change> {
        let pid = file.pid;
        let stamp = file.stamp;
        let new_state = file.state;
        let known = self.sessions.get(&pid);
        let same_process = known.filter(|known| known.file.started_at == file.started_at);
        if same_process.is_some_and(|known| stamp <= known.file.stamp) {
            return None;
        }
        let old = same_process.map(|known| known.file.state);
        let same_state = same_process.is_some_and(|known| known.file.state == new_state);
        let (since, handled) = match same_process {
            Some(known) if same_state => (known.since, known.handled),
            _ => (stamp, None),
        };
        let session = Session {
            file,
            name,
            since,
            run: RunStatus::Running,
            handled,
        };
        let label = session.label();
        self.sessions.insert(pid, session);
        if same_state {
            return None;
        }
        let change = Change {
            at: stamp,
            pid,
            label,
            old,
            new: Some(new_state),
        };
        self.record(change.clone());
        Some(change)
    }

    /// Set the run status of the session with the pid. The function gives
    /// `None` when the board does not hold the pid, or when the session has
    /// the run status already. A freeze and a thaw are each a change of state.
    /// The time in the state starts again at `now`, the tick that saw the
    /// change. The change goes into the log, and the handled mark goes away.
    pub fn set_run_status(&mut self, pid: Pid, run: RunStatus, now: EpochMillis) -> Option<Change> {
        let session = self.sessions.get_mut(&pid)?;
        if session.run == run {
            return None;
        }
        let old = session.state();
        session.run = run;
        let new = session.state();
        if old == new {
            return None;
        }
        session.since = now;
        session.handled = None;
        let change = Change {
            at: now,
            pid,
            label: session.label(),
            old: Some(old),
            new: Some(new),
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

    /// Take a session off the board, because its process ended. The function
    /// gives the change that the end makes, or `None` when the board does not
    /// hold the pid. The change goes into the log.
    pub fn remove(&mut self, pid: Pid, now: EpochMillis) -> Option<Change> {
        let session = self.sessions.remove(&pid)?;
        let change = Change {
            at: now,
            pid,
            label: session.label(),
            old: Some(session.file.state),
            new: None,
        };
        self.record(change.clone());
        Some(change)
    }

    /// Put a handled mark on the session with the pid, or remove its mark.
    /// The function gives `Some(HandledToggle::Marked)` when the session now
    /// has a mark, `Some(HandledToggle::Unmarked)` when it removed the mark,
    /// and `None` when the board does not hold the pid. It gives
    /// `Some(HandledToggle::Refused)` and changes nothing when the session has
    /// no mark and its shown state takes none, see [`PaneState::takes_handled_mark`].
    /// A new mark sorts the row below each other row in its state.
    pub fn toggle_handled(&mut self, pid: Pid) -> Option<HandledToggle> {
        let session = self.sessions.get_mut(&pid)?;
        if session.handled.take().is_some() {
            return Some(HandledToggle::Unmarked);
        }
        if !session.state().takes_handled_mark() {
            return Some(HandledToggle::Refused);
        }
        session.handled = Some(HandledMark(self.next_mark));
        self.next_mark += 1;
        Some(HandledToggle::Marked)
    }

    /// The count of each shown state, in the sort order of the rows. The array
    /// holds every state, also a state with no session. Each count splits the
    /// sessions of its state by the handled mark.
    #[must_use]
    pub fn state_counts(&self) -> [StateCount; 6] {
        let mut ranked = PaneState::ALL;
        ranked.sort_by_key(|state| state.rank());
        ranked.map(|state| {
            let in_state = self
                .sessions
                .values()
                .filter(|session| session.state() == state);
            let (unmarked, handled) =
                in_state.fold((0_usize, 0_usize), |(unmarked, handled), session| {
                    if session.is_handled() {
                        (unmarked, handled + 1)
                    } else {
                        (unmarked + 1, handled)
                    }
                });
            StateCount {
                state,
                unmarked,
                handled,
            }
        })
    }

    /// The session with the pid, when the board holds it.
    #[must_use]
    pub fn get(&self, pid: Pid) -> Option<&Session> {
        self.sessions.get(&pid)
    }

    /// The pids of all sessions, sorted.
    #[must_use]
    pub fn pids(&self) -> Vec<Pid> {
        let mut pids: Vec<Pid> = self.sessions.keys().copied().collect();
        pids.sort_by_key(|pid| pid.as_u32());
        pids
    }

    /// The number of sessions on the board.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Tell if the board holds no session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// The sessions in the order of the screen: by the rank of the shown state, then
    /// the sessions with no handled mark, then the handled sessions in the
    /// order of their marks, then the oldest `since` first, then the pid.
    #[must_use]
    pub fn rows(&self) -> Vec<&Session> {
        let mut rows: Vec<&Session> = self.sessions.values().collect();
        rows.sort_by_key(|session| {
            (
                session.state().rank(),
                session.handled,
                session.since,
                session.file.pid.as_u32(),
            )
        });
        rows
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
            run: RunStatus::Running,
            handled: None,
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

    #[test]
    fn load_inserts_a_session_and_logs_nothing() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert_eq!(session.since, EpochMillis::new(500));
        assert_eq!(session.file.state, PaneState::Idle);
        assert_eq!(board.changes().count(), 0);
    }

    #[test]
    fn apply_logs_a_change_of_state_with_the_new_stamp() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        let change = board
            .apply(file(7, PaneState::Working, 100, 900), None)
            .expect("a new state is a change");
        assert_eq!(
            change,
            Change {
                at: EpochMillis::new(900),
                pid: Pid::new(7),
                label: "worktree".to_string(),
                old: Some(PaneState::Idle),
                new: Some(PaneState::Working),
            }
        );
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert_eq!(session.since, EpochMillis::new(900));
        assert_eq!(session.file.state, PaneState::Working);
        assert_eq!(board.changes().cloned().collect::<Vec<_>>(), vec![change]);
    }

    #[test]
    fn apply_ignores_an_older_or_equal_stamp() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        let before = board.get(Pid::new(7)).cloned();
        assert_eq!(
            board.apply(file(7, PaneState::Working, 100, 500), Some("a".into())),
            None
        );
        assert_eq!(
            board.apply(file(7, PaneState::Working, 100, 400), Some("b".into())),
            None
        );
        assert_eq!(board.get(Pid::new(7)).cloned(), before);
        assert_eq!(board.changes().count(), 0);
    }

    #[test]
    fn apply_replaces_a_session_when_the_pid_is_reused() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        let change = board
            .apply(file(7, PaneState::Idle, 200, 300), None)
            .expect("a new session is a change");
        assert_eq!(change.old, None);
        assert_eq!(change.new, Some(PaneState::Idle));
        assert_eq!(change.at, EpochMillis::new(300));
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert_eq!(session.file.started_at, EpochMillis::new(200));
        assert_eq!(session.since, EpochMillis::new(300));
    }

    #[test]
    fn apply_updates_the_file_and_keeps_since_when_the_state_is_the_same() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), Some("old".into()));
        let mut newer = file(7, PaneState::Idle, 100, 900);
        newer.cwd = std::path::PathBuf::from("/elsewhere/project");
        newer.session_id = "new-id".to_string();
        assert_eq!(board.apply(newer, Some("renamed".into())), None);
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert_eq!(session.since, EpochMillis::new(500));
        assert_eq!(session.file.stamp, EpochMillis::new(900));
        assert_eq!(session.file.session_id, "new-id");
        assert_eq!(session.label(), "renamed");
        assert_eq!(board.changes().count(), 0);
    }

    #[test]
    fn remove_logs_the_end_of_a_known_session_and_ignores_an_unknown_pid() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        assert_eq!(board.remove(Pid::new(8), EpochMillis::new(1000)), None);
        let change = board
            .remove(Pid::new(7), EpochMillis::new(1000))
            .expect("the end of a known session is a change");
        assert_eq!(
            change,
            Change {
                at: EpochMillis::new(1000),
                pid: Pid::new(7),
                label: "worktree".to_string(),
                old: Some(PaneState::Idle),
                new: None,
            }
        );
        assert!(board.get(Pid::new(7)).is_none());
        assert_eq!(board.changes().cloned().collect::<Vec<_>>(), vec![change]);
    }

    /// The counts of the board as `(state, unmarked, handled)` rows.
    fn counts(board: &Board) -> Vec<(PaneState, usize, usize)> {
        board
            .state_counts()
            .iter()
            .map(|count| (count.state, count.unmarked, count.handled))
            .collect()
    }

    #[test]
    fn state_counts_count_a_state_of_sessions_with_no_mark() {
        use PaneState::{Agents, Frozen, Idle, NeedsYou, Shells, Working};
        let mut board = Board::new();
        board.load(file(1, Agents, 1, 100), None);
        board.load(file(2, Agents, 1, 100), None);
        board.load(file(3, Agents, 1, 100), None);
        assert_eq!(
            counts(&board),
            [
                (NeedsYou, 0, 0),
                (Idle, 0, 0),
                (Agents, 3, 0),
                (Shells, 0, 0),
                (Working, 0, 0),
                (Frozen, 0, 0)
            ]
        );
    }

    #[test]
    fn state_counts_count_a_state_of_handled_sessions() {
        let mut board = Board::new();
        board.load(file(1, PaneState::Idle, 1, 100), None);
        board.load(file(2, PaneState::Idle, 1, 100), None);
        board.toggle_handled(Pid::new(1));
        board.toggle_handled(Pid::new(2));
        let idle = board.state_counts()[1];
        assert_eq!(
            (idle.state, idle.unmarked, idle.handled),
            (PaneState::Idle, 0, 2)
        );
    }

    #[test]
    fn state_counts_split_a_state_with_both_kinds() {
        let mut board = Board::new();
        board.load(file(1, PaneState::NeedsYou, 1, 100), None);
        board.load(file(2, PaneState::NeedsYou, 1, 100), None);
        board.load(file(3, PaneState::NeedsYou, 1, 100), None);
        board.load(file(4, PaneState::Working, 1, 100), None);
        board.toggle_handled(Pid::new(2));
        let counts = board.state_counts();
        assert_eq!((counts[0].unmarked, counts[0].handled), (2, 1));
        assert_eq!((counts[4].unmarked, counts[4].handled), (1, 0));
    }

    #[test]
    fn state_counts_hold_every_state_in_rank_order() {
        let board = Board::new();
        let counts = board.state_counts();
        let ranks: Vec<u8> = counts.iter().map(|count| count.state.rank()).collect();
        assert_eq!(ranks, [0, 1, 2, 3, 4, 5]);
        assert!(counts
            .iter()
            .all(|count| count.unmarked == 0 && count.handled == 0));
    }

    #[test]
    fn rows_sort_by_state_then_oldest_since_then_pid() {
        use PaneState::{Agents, Idle, NeedsYou, Shells, Working};
        let mut board = Board::new();
        board.load(file(1, Working, 1, 100), None);
        board.load(file(2, Shells, 1, 100), None);
        board.load(file(3, Agents, 1, 100), None);
        board.load(file(4, Idle, 1, 300), None);
        board.load(file(5, Idle, 1, 200), None);
        board.load(file(6, NeedsYou, 1, 100), None);
        board.load(file(8, Idle, 1, 200), None);
        let order: Vec<u32> = board
            .rows()
            .iter()
            .map(|session| session.file.pid.as_u32())
            .collect();
        assert_eq!(order, vec![6, 5, 8, 4, 3, 2, 1]);
    }

    /// The pids of the rows, in the order of the screen.
    fn row_order(board: &Board) -> Vec<u32> {
        board
            .rows()
            .iter()
            .map(|session| session.file.pid.as_u32())
            .collect()
    }

    /// A board with one `needs-you` row, three idle rows, and one `agents`
    /// row. The idle rows sort 5, 8, 4.
    fn board_with_three_idle_rows() -> Board {
        use PaneState::{Agents, Idle, NeedsYou};
        let mut board = Board::new();
        board.load(file(3, Agents, 1, 100), None);
        board.load(file(4, Idle, 1, 300), None);
        board.load(file(5, Idle, 1, 200), None);
        board.load(file(6, NeedsYou, 1, 100), None);
        board.load(file(8, Idle, 1, 200), None);
        board
    }

    #[test]
    fn a_handled_row_sorts_below_each_other_row_in_its_state() {
        let mut board = board_with_three_idle_rows();
        assert_eq!(row_order(&board), vec![6, 5, 8, 4, 3]);
        assert_eq!(
            board.toggle_handled(Pid::new(5)),
            Some(HandledToggle::Marked)
        );
        assert_eq!(row_order(&board), vec![6, 8, 4, 5, 3]);
    }

    #[test]
    fn the_latest_handled_row_sorts_last_in_its_state() {
        let mut board = board_with_three_idle_rows();
        board.toggle_handled(Pid::new(5));
        board.toggle_handled(Pid::new(8));
        assert_eq!(row_order(&board), vec![6, 4, 5, 8, 3]);
    }

    #[test]
    fn toggle_handled_refuses_a_session_that_does_not_wait_for_the_person() {
        for state in [PaneState::Working, PaneState::Agents, PaneState::Shells] {
            let mut board = Board::new();
            board.load(file(7, state, 100, 500), None);
            assert_eq!(
                board.toggle_handled(Pid::new(7)),
                Some(HandledToggle::Refused),
                "{state:?}"
            );
            let session = board.get(Pid::new(7)).expect("the session is on the board");
            assert!(!session.is_handled(), "{state:?}");
        }
    }

    #[test]
    fn a_second_toggle_removes_the_mark_and_restores_the_order() {
        let mut board = board_with_three_idle_rows();
        assert_eq!(
            board.toggle_handled(Pid::new(5)),
            Some(HandledToggle::Marked)
        );
        assert_eq!(
            board.toggle_handled(Pid::new(5)),
            Some(HandledToggle::Unmarked)
        );
        let session = board.get(Pid::new(5)).expect("the session is on the board");
        assert!(!session.is_handled());
        assert_eq!(row_order(&board), vec![6, 5, 8, 4, 3]);
    }

    #[test]
    fn toggle_handled_of_an_unknown_pid_changes_nothing() {
        let mut board = board_with_three_idle_rows();
        assert_eq!(board.toggle_handled(Pid::new(99)), None);
        assert_eq!(row_order(&board), vec![6, 5, 8, 4, 3]);
    }

    #[test]
    fn a_handled_session_shows_the_suffix_after_its_state() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        let label = |board: &Board| board.get(Pid::new(7)).map(Session::state_label);
        assert_eq!(label(&board).as_deref(), Some("idle"));
        board.toggle_handled(Pid::new(7));
        assert_eq!(label(&board).as_deref(), Some("idle - handled"));
    }

    #[test]
    fn a_change_of_state_removes_the_mark() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        assert_eq!(
            board.toggle_handled(Pid::new(7)),
            Some(HandledToggle::Marked)
        );
        board.apply(file(7, PaneState::Working, 100, 600), None);
        let is_handled = |board: &Board| board.get(Pid::new(7)).map(Session::is_handled);
        assert_eq!(is_handled(&board), Some(false));
        board.apply(file(7, PaneState::Idle, 100, 700), None);
        assert_eq!(is_handled(&board), Some(false), "the mark does not return");
    }

    #[test]
    fn a_new_write_in_the_same_state_keeps_the_mark() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        board.toggle_handled(Pid::new(7));
        assert_eq!(board.apply(file(7, PaneState::Idle, 100, 900), None), None);
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert!(session.is_handled());
    }

    #[test]
    fn a_reused_pid_has_no_mark() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        assert_eq!(
            board.toggle_handled(Pid::new(7)),
            Some(HandledToggle::Marked)
        );
        board.apply(file(7, PaneState::Idle, 200, 900), None);
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert!(!session.is_handled());
    }

    #[test]
    fn pids_len_and_is_empty_describe_the_board() {
        let mut board = Board::new();
        assert!(board.is_empty());
        assert_eq!(board.len(), 0);
        board.load(file(9, PaneState::Idle, 1, 1), None);
        board.load(file(3, PaneState::Idle, 1, 1), None);
        assert!(!board.is_empty());
        assert_eq!(board.len(), 2);
        assert_eq!(board.pids(), vec![Pid::new(3), Pid::new(9)]);
    }

    #[test]
    fn changes_give_the_newest_first() {
        let mut board = Board::new();
        board.apply(file(1, PaneState::Working, 1, 10), None);
        board.apply(file(1, PaneState::Idle, 1, 20), None);
        board.remove(Pid::new(1), EpochMillis::new(30));
        let times: Vec<u64> = board.changes().map(|change| change.at.as_u64()).collect();
        assert_eq!(times, vec![30, 20, 10]);
    }

    #[test]
    fn the_log_keeps_at_most_log_capacity_changes_and_drops_the_oldest() {
        let mut board = Board::new();
        for index in 0..=LOG_CAPACITY {
            let stamp = 10 + index as u64;
            let state = if index % 2 == 0 {
                PaneState::Working
            } else {
                PaneState::Idle
            };
            board.apply(file(1, state, 1, stamp), None);
        }
        assert_eq!(board.changes().count(), LOG_CAPACITY);
        assert_eq!(
            board.changes().next().map(|change| change.at.as_u64()),
            Some(10 + LOG_CAPACITY as u64)
        );
        assert_eq!(
            board.changes().last().map(|change| change.at.as_u64()),
            Some(11)
        );
    }

    /// A board with a session of pid 7 that the process table shows stopped
    /// at `1_000`.
    fn frozen_board(state: PaneState) -> Board {
        let mut board = Board::new();
        board.load(file(7, state, 100, 500), None);
        board.set_run_status(Pid::new(7), RunStatus::Stopped, EpochMillis::new(1_000));
        board
    }

    #[test]
    fn a_stopped_session_shows_frozen_for_each_file_state() {
        use PaneState::{Agents, Frozen, Idle, NeedsYou, Shells, Working};
        for state in [Working, NeedsYou, Idle, Agents, Shells] {
            let mut board = Board::new();
            board.load(file(7, state, 100, 500), None);
            let change = board
                .set_run_status(Pid::new(7), RunStatus::Stopped, EpochMillis::new(1_000))
                .expect("a freeze is a change");
            let expected = Change {
                at: EpochMillis::new(1_000),
                pid: Pid::new(7),
                label: "worktree".to_string(),
                old: Some(state),
                new: Some(Frozen),
            };
            assert_eq!(change, expected, "{state:?}");
            let session = board.get(Pid::new(7)).expect("the session is on the board");
            assert_eq!(session.state(), Frozen, "{state:?}");
            assert_eq!(session.state_label(), "frozen", "{state:?}");
            assert_eq!(session.file.state, state, "{state:?}");
            assert_eq!(session.since, EpochMillis::new(1_000), "{state:?}");
            assert_eq!(board.changes().cloned().collect::<Vec<_>>(), vec![expected]);
        }
    }

    #[test]
    fn a_continued_session_shows_the_state_of_its_file_again() {
        let mut board = frozen_board(PaneState::Idle);
        let change = board
            .set_run_status(Pid::new(7), RunStatus::Running, EpochMillis::new(2_000))
            .expect("a thaw is a change");
        assert_eq!(change.old, Some(PaneState::Frozen));
        assert_eq!(change.new, Some(PaneState::Idle));
        assert_eq!(change.at, EpochMillis::new(2_000));
        let session = board.get(Pid::new(7)).expect("the session is on the board");
        assert_eq!(session.since, EpochMillis::new(2_000));
        assert_eq!(session.state(), PaneState::Idle);
    }

    #[test]
    fn set_run_status_of_an_unknown_pid_or_of_the_same_status_changes_nothing() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        let now = EpochMillis::new(1_000);
        assert_eq!(
            board.set_run_status(Pid::new(8), RunStatus::Stopped, now),
            None
        );
        assert_eq!(
            board.set_run_status(Pid::new(7), RunStatus::Running, now),
            None
        );
        assert!(board
            .set_run_status(Pid::new(7), RunStatus::Stopped, now)
            .is_some());
        let later = EpochMillis::new(2_000);
        assert_eq!(
            board.set_run_status(Pid::new(7), RunStatus::Stopped, later),
            None
        );
        assert_eq!(board.changes().count(), 1);
        assert_eq!(board.get(Pid::new(7)).expect("on the board").since, now);
    }

    #[test]
    fn a_freeze_removes_the_handled_mark() {
        let mut board = Board::new();
        board.load(file(7, PaneState::Idle, 100, 500), None);
        assert_eq!(
            board.toggle_handled(Pid::new(7)),
            Some(HandledToggle::Marked)
        );
        let now = EpochMillis::new(1_000);
        board.set_run_status(Pid::new(7), RunStatus::Stopped, now);
        assert!(!board.get(Pid::new(7)).expect("on the board").is_handled());
        board.set_run_status(Pid::new(7), RunStatus::Running, EpochMillis::new(2_000));
        assert!(!board.get(Pid::new(7)).expect("on the board").is_handled());
    }

    #[test]
    fn toggle_handled_refuses_a_frozen_session() {
        let mut board = frozen_board(PaneState::Idle);
        assert_eq!(
            board.toggle_handled(Pid::new(7)),
            Some(HandledToggle::Refused)
        );
        assert!(!board.get(Pid::new(7)).expect("on the board").is_handled());
    }

    #[test]
    fn a_frozen_session_sorts_last() {
        let mut board = Board::new();
        board.load(file(1, PaneState::Working, 100, 500), None);
        board.load(file(2, PaneState::NeedsYou, 100, 500), None);
        board.load(file(3, PaneState::NeedsYou, 100, 100), None);
        board.set_run_status(Pid::new(3), RunStatus::Stopped, EpochMillis::new(1_000));
        let pids: Vec<u32> = board
            .rows()
            .iter()
            .map(|session| session.file.pid.as_u32())
            .collect();
        assert_eq!(pids, vec![2, 1, 3]);
    }

    #[test]
    fn state_counts_count_a_frozen_session_as_frozen() {
        let board = frozen_board(PaneState::NeedsYou);
        let counts = board.state_counts();
        let of = |state: PaneState| {
            let count = counts
                .iter()
                .find(|count| count.state == state)
                .expect("each state has a count");
            (count.unmarked, count.handled)
        };
        assert_eq!(of(PaneState::Frozen), (1, 0));
        assert_eq!(of(PaneState::NeedsYou), (0, 0));
    }

    #[test]
    fn a_newer_file_of_a_frozen_session_changes_nothing_on_the_screen() {
        let mut board = frozen_board(PaneState::Idle);
        let applied = board.apply(file(7, PaneState::Working, 100, 1_500), None);
        assert_eq!(applied, None);
        assert_eq!(board.changes().count(), 1);
        let session = board.get(Pid::new(7)).expect("on the board");
        assert_eq!(session.since, EpochMillis::new(1_000));
        assert_eq!(session.state(), PaneState::Frozen);
        assert_eq!(session.file.state, PaneState::Working);
        assert_eq!(session.file.stamp, EpochMillis::new(1_500));
        let thaw = board
            .set_run_status(Pid::new(7), RunStatus::Running, EpochMillis::new(2_000))
            .expect("a thaw is a change");
        assert_eq!(thaw.old, Some(PaneState::Frozen));
        assert_eq!(thaw.new, Some(PaneState::Working));
    }

    #[test]
    fn a_reused_pid_of_a_frozen_session_is_a_running_session() {
        let mut board = frozen_board(PaneState::Idle);
        let change = board
            .apply(file(7, PaneState::Working, 200, 1_500), None)
            .expect("a new process is a change");
        assert_eq!(change.old, None);
        assert_eq!(change.new, Some(PaneState::Working));
        let session = board.get(Pid::new(7)).expect("on the board");
        assert_eq!(session.state(), PaneState::Working);
    }
}
