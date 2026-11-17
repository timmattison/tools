//! The logic of the main loop, with no terminal in it. The controller takes
//! one event, changes the app, and returns the effects that the caller runs.

use crossterm::event::Event;
use proctree::Pid;

use crate::app::{Action, App};
use crate::board::Change;
use crate::focus::{FocusReport, FocusTarget};
use crate::liveness::ProcessClock;
use crate::notification::needs_you_message;
use crate::state::EpochMillis;
use crate::store::DirEvent;
use crate::sync::Sync;

/// One event of the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// An event of the terminal: a key or a resize.
    Terminal(Event),
    /// A change in the state directory.
    Dir(DirEvent),
    /// A focus request ended.
    FocusDone(FocusReport),
    /// A notification did not go out. The text says why.
    NotifyFailed(String),
    /// One second passed.
    Tick,
}

/// A job that the caller runs after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Leave the program.
    Quit,
    /// Focus the terminal of a session.
    Focus(FocusTarget),
    /// Send a notification with this text.
    Notify(String),
}

/// The app, and the parts that keep it in step with the system.
pub struct Controller {
    app: App,
    sync: Sync,
    clock: Box<dyn ProcessClock + Send>,
    notify: bool,
}

impl Controller {
    /// A controller with an empty app. When `notify` is false, it asks for
    /// no notification.
    #[must_use]
    pub fn new(sync: Sync, clock: Box<dyn ProcessClock + Send>, notify: bool) -> Self {
        Self {
            app: App::new(),
            sync,
            clock,
            notify,
        }
    }

    /// Load the sessions that exist now. This is the first read.
    ///
    /// # Errors
    ///
    /// Returns the error of the read of the state directory.
    pub fn start(&mut self) -> std::io::Result<()> {
        self.sync.initial(self.app.board_mut(), self.clock.as_mut())
    }

    /// The app, for the draw.
    #[must_use]
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Handle one event at the time `now`. Return the effects to run.
    pub fn handle(&mut self, event: AppEvent, now: EpochMillis) -> Vec<Effect> {
        match event {
            AppEvent::Terminal(Event::Key(key)) => match self.app.handle_key(key, now) {
                Action::Nothing => Vec::new(),
                Action::Quit => vec![Effect::Quit],
                Action::Focus(target) => vec![Effect::Focus(target)],
            },
            AppEvent::Terminal(_) => Vec::new(),
            AppEvent::Dir(event) => {
                let changes =
                    self.sync
                        .on_dir_event(self.app.board_mut(), event, self.clock.as_mut(), now);
                let needs_you: Vec<&Change> = changes
                    .iter()
                    .filter(|change| change.is_needs_you())
                    .collect();
                let pids: Vec<Pid> = needs_you.iter().map(|change| change.pid).collect();
                self.app.on_needs_you(&pids, now);
                if !self.notify {
                    return Vec::new();
                }
                needs_you
                    .into_iter()
                    .map(|change| Effect::Notify(needs_you_message(change)))
                    .collect()
            }
            AppEvent::Tick => {
                let _ended = self
                    .sync
                    .on_tick(self.app.board_mut(), self.clock.as_mut(), now);
                self.app.on_tick(now);
                Vec::new()
            }
            AppEvent::FocusDone(report) => {
                self.app.set_status(report.status_line());
                Vec::new()
            }
            AppEvent::NotifyFailed(text) => {
                self.app
                    .set_status(format!("The notification failed: {text}"));
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use proctree::Pid;

    use super::*;
    use crate::app::JUMP_HOLD_MS;
    use crate::store::test_support::{clock_with, state_json, FakeClock};

    const STARTED_MS: u64 = 1_000_000;
    const START_SECS: u64 = 900;
    const NOW: EpochMillis = EpochMillis::new(5_000_000);

    fn write(dir: &Path, pid: u32, state: &str, stamp: u64) {
        std::fs::write(
            dir.join(format!("{pid}.json")),
            state_json(pid, state, STARTED_MS, stamp),
        )
        .unwrap();
    }

    fn controller(dir: &Path, notify: bool) -> Controller {
        let sync = Sync::with_dirs(dir.to_path_buf(), dir.join("sessions"));
        Controller::new(sync, Box::new(clock_with(100, START_SECS)), notify)
    }

    /// A controller whose clock knows every pid in `pids`.
    fn controller_of(dir: &Path, pids: &[u32], notify: bool) -> Controller {
        let clock = FakeClock(
            pids.iter()
                .map(|&pid| (Pid::new(pid), START_SECS))
                .collect(),
        );
        let sync = Sync::with_dirs(dir.to_path_buf(), dir.join("sessions"));
        Controller::new(sync, Box::new(clock), notify)
    }

    /// The pid of the selected row.
    fn selected_pid(controller: &Controller) -> Pid {
        let app = controller.app();
        let index = app.selected_index().unwrap();
        app.board().rows()[index].file.pid
    }

    fn key(code: KeyCode) -> AppEvent {
        AppEvent::Terminal(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn file_event(pid: u32) -> AppEvent {
        AppEvent::Dir(DirEvent::File(Pid::new(pid)))
    }

    #[test]
    fn start_loads_the_files_with_no_effect() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "idle", 10);
        let mut controller = controller(dir.path(), true);
        controller.start().unwrap();
        assert_eq!(controller.app().board().len(), 1);
        assert!(controller.app().board().changes().next().is_none());
    }

    #[test]
    fn a_resize_changes_the_page_size_that_page_down_uses() {
        let dir = tempfile::tempdir().unwrap();
        let pids: Vec<u32> = (100..160).collect();
        for &pid in &pids {
            write(dir.path(), pid, "idle", 10);
        }
        let mut controller = controller_of(dir.path(), &pids, true);
        controller.start().unwrap();
        let small = crate::ui::page_size(20).get();
        let large = crate::ui::page_size(40).get();
        assert!(small > 1, "a page of 1 cannot prove a resize: {small}");
        assert!(large > small, "a taller terminal shows more rows");
        assert!(small + large < 60, "the rows must outlast both pages");

        controller.handle(AppEvent::Terminal(Event::Resize(80, 20)), NOW);
        controller.handle(key(KeyCode::PageDown), NOW);
        assert_eq!(controller.app().selected_index(), Some(small));

        controller.handle(AppEvent::Terminal(Event::Resize(80, 40)), NOW);
        controller.handle(key(KeyCode::PageDown), NOW);
        assert_eq!(controller.app().selected_index(), Some(small + large));
    }

    #[test]
    fn q_quits_and_a_resize_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = controller(dir.path(), true);
        assert_eq!(
            controller.handle(key(KeyCode::Char('q')), NOW),
            vec![Effect::Quit]
        );
        let resize = AppEvent::Terminal(Event::Resize(80, 24));
        assert_eq!(controller.handle(resize, NOW), vec![]);
    }

    #[test]
    fn enter_focuses_the_selected_session() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "idle", 10);
        let mut controller = controller(dir.path(), true);
        controller.start().unwrap();
        let effects = controller.handle(key(KeyCode::Enter), NOW);
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            Effect::Focus(target) => assert_eq!(target.pid, Pid::new(100)),
            other => panic!("expected a focus, got {other:?}"),
        }
    }

    #[test]
    fn a_change_to_needs_you_gives_one_notification() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "working", 10);
        let mut controller = controller(dir.path(), true);
        controller.start().unwrap();
        write(dir.path(), 100, "needs-you", 20);
        let effects = controller.handle(file_event(100), NOW);
        assert_eq!(effects, vec![Effect::Notify("worktree needs you".into())]);
    }

    #[test]
    fn no_notify_gives_no_notification() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "working", 10);
        let mut controller = controller(dir.path(), false);
        controller.start().unwrap();
        write(dir.path(), 100, "needs-you", 20);
        assert_eq!(controller.handle(file_event(100), NOW), vec![]);
        assert_eq!(controller.app().board().changes().count(), 1);
    }

    #[test]
    fn a_change_to_idle_gives_no_notification() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "working", 10);
        let mut controller = controller(dir.path(), true);
        controller.start().unwrap();
        write(dir.path(), 100, "idle", 20);
        assert_eq!(controller.handle(file_event(100), NOW), vec![]);
        assert_eq!(controller.app().board().changes().count(), 1);
    }

    #[test]
    fn a_tick_drops_a_stale_session_with_no_effect() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "needs-you", 10);
        // The process started after the session claims: the pid was reused.
        let sync = Sync::with_dirs(dir.path().to_path_buf(), dir.path().join("sessions"));
        let mut controller = Controller::new(sync, Box::new(clock_with(100, START_SECS)), true);
        controller.start().unwrap();
        assert_eq!(controller.app().board().len(), 1);
        controller.clock = Box::new(clock_with(100, STARTED_MS / 1000 + 50));
        assert_eq!(controller.handle(AppEvent::Tick, NOW), vec![]);
        assert!(controller.app().board().is_empty());
    }

    #[test]
    fn focus_done_and_notify_failed_set_the_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = controller(dir.path(), true);
        let report = FocusReport {
            label: "tools".into(),
            zellij: None,
            wezterm: Ok(7),
        };
        let line = report.status_line();
        assert_eq!(controller.handle(AppEvent::FocusDone(report), NOW), vec![]);
        assert_eq!(controller.app().status(), Some(line.as_str()));
        let failed = AppEvent::NotifyFailed("osascript failed".into());
        assert_eq!(controller.handle(failed, NOW), vec![]);
        assert_eq!(
            controller.app().status(),
            Some("The notification failed: osascript failed")
        );
    }

    /// Pid 100 waits in `needs-you` and pid 101 works, so a jump to pid 101
    /// moves the selection off row 0.
    fn jump_fixture(dir: &Path, notify: bool) -> Controller {
        write(dir, 100, "needs-you", 10);
        write(dir, 101, "working", 10);
        let mut controller = controller_of(dir, &[100, 101], notify);
        controller.start().unwrap();
        assert_eq!(selected_pid(&controller), Pid::new(100));
        controller
    }

    #[test]
    fn a_change_to_needs_you_selects_that_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = jump_fixture(dir.path(), true);
        write(dir.path(), 101, "needs-you", 20);
        controller.handle(file_event(101), NOW);
        assert_eq!(selected_pid(&controller), Pid::new(101));
    }

    #[test]
    fn a_change_to_needs_you_selects_that_session_with_no_notify() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = jump_fixture(dir.path(), false);
        write(dir.path(), 101, "needs-you", 20);
        controller.handle(file_event(101), NOW);
        assert_eq!(selected_pid(&controller), Pid::new(101));
    }

    #[test]
    fn a_new_session_in_needs_you_selects_that_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = jump_fixture(dir.path(), true);
        write(dir.path(), 102, "needs-you", 20);
        controller.clock = Box::new(FakeClock(
            [100, 101, 102]
                .iter()
                .map(|&pid| (Pid::new(pid), START_SECS))
                .collect(),
        ));
        controller.handle(file_event(102), NOW);
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    /// A guard. It passes before the jump exists. A jump on each change fails it.
    #[test]
    fn a_change_to_another_state_does_not_move_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = jump_fixture(dir.path(), true);
        write(dir.path(), 101, "idle", 20);
        controller.handle(file_event(101), NOW);
        assert_eq!(selected_pid(&controller), Pid::new(100));
    }

    #[test]
    fn several_changes_to_needs_you_select_the_first_row_of_the_board() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 100, "needs-you", 10);
        for pid in [101, 102, 103] {
            write(dir.path(), pid, "working", 10);
        }
        let mut controller = controller_of(dir.path(), &[100, 101, 102, 103], true);
        controller.start().unwrap();
        write(dir.path(), 101, "needs-you", 30);
        write(dir.path(), 102, "needs-you", 20);
        write(dir.path(), 103, "needs-you", 40);
        controller.handle(AppEvent::Dir(DirEvent::Rescan), NOW);
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    /// The time `ms` milliseconds after `NOW`.
    fn after_now(ms: u64) -> EpochMillis {
        EpochMillis::new(NOW.as_u64() + ms)
    }

    fn before_now(ms: u64) -> EpochMillis {
        EpochMillis::new(NOW.as_u64() - ms)
    }

    /// Pid 100 waits in `needs-you`. Pids 101 and 102 work. A key press
    /// `j` at `NOW` selects pid 101. A change of pid 102 to `needs-you` then
    /// sorts to row 1, above the selected row 2.
    fn hold_fixture(dir: &Path) -> Controller {
        write(dir, 100, "needs-you", 10);
        write(dir, 101, "working", 10);
        write(dir, 102, "working", 10);
        let mut controller = controller_of(dir, &[100, 101, 102, 103], true);
        controller.start().unwrap();
        controller.handle(key(KeyCode::Char('j')), NOW);
        assert_eq!(selected_pid(&controller), Pid::new(101));
        controller
    }

    fn release(code: KeyCode) -> AppEvent {
        AppEvent::Terminal(Event::Key(KeyEvent::new_with_kind(
            code,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )))
    }

    #[test]
    fn a_key_press_holds_a_jump_until_the_hold_ends() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(file_event(102), after_now(500));
        assert_eq!(selected_pid(&controller), Pid::new(101));
        controller.handle(AppEvent::Tick, after_now(JUMP_HOLD_MS - 1));
        assert_eq!(selected_pid(&controller), Pid::new(101));
        controller.handle(AppEvent::Tick, after_now(JUMP_HOLD_MS));
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    /// A clock that moves back after a key press still ends the hold, `JUMP_HOLD_MS` after the change.
    #[test]
    fn a_held_jump_still_comes_after_the_clock_moves_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        let step = before_now(10_000);
        controller.handle(file_event(102), step);
        assert_eq!(selected_pid(&controller), Pid::new(101));
        controller.handle(
            AppEvent::Tick,
            EpochMillis::new(step.as_u64() + JUMP_HOLD_MS - 1),
        );
        assert_eq!(selected_pid(&controller), Pid::new(101));
        controller.handle(
            AppEvent::Tick,
            EpochMillis::new(step.as_u64() + JUMP_HOLD_MS),
        );
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    #[test]
    fn a_key_press_exactly_the_hold_before_the_change_does_not_hold() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(file_event(102), after_now(JUMP_HOLD_MS));
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    #[test]
    fn a_second_key_press_starts_the_hold_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(file_event(102), after_now(500));
        controller.handle(key(KeyCode::Char('j')), after_now(1_000));
        controller.handle(AppEvent::Tick, after_now(JUMP_HOLD_MS));
        assert_eq!(selected_pid(&controller), Pid::new(101));
        controller.handle(AppEvent::Tick, after_now(1_000 + JUMP_HOLD_MS));
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    #[test]
    fn a_key_release_does_not_hold_a_jump() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(release(KeyCode::Char('j')), after_now(JUMP_HOLD_MS + 500));
        controller.handle(file_event(102), after_now(JUMP_HOLD_MS + 600));
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    #[test]
    fn a_change_after_the_hold_ends_jumps_to_the_first_of_it_and_the_held_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(file_event(102), after_now(500));
        // Pid 103 sorts above pid 102, because its stamp is older.
        write(dir.path(), 103, "needs-you", 15);
        controller.handle(file_event(103), after_now(JUMP_HOLD_MS + 100));
        assert_eq!(selected_pid(&controller), Pid::new(103));
    }

    /// The same as above, but the held session sorts first.
    #[test]
    fn a_held_session_that_sorts_first_wins_over_a_later_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(file_event(102), after_now(500));
        write(dir.path(), 103, "needs-you", 25);
        controller.handle(file_event(103), after_now(JUMP_HOLD_MS + 100));
        assert_eq!(selected_pid(&controller), Pid::new(102));
    }

    /// Hold a jump to pid 102, run `change` during the hold, then send a tick
    /// after the hold. The selection must stay on pid 101.
    fn assert_held_jump_stops(change: impl FnOnce(&Path, &mut Controller)) {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        controller.handle(file_event(102), after_now(500));
        change(dir.path(), &mut controller);
        controller.handle(AppEvent::Tick, after_now(JUMP_HOLD_MS));
        assert_eq!(selected_pid(&controller), Pid::new(101));
    }

    #[test]
    fn a_held_jump_stops_when_the_session_leaves_needs_you() {
        assert_held_jump_stops(|dir, controller| {
            write(dir, 102, "idle", 30);
            controller.handle(file_event(102), after_now(1_000));
        });
    }

    #[test]
    fn a_held_jump_stops_when_the_session_ends() {
        assert_held_jump_stops(|dir, controller| {
            std::fs::remove_file(dir.join("102.json")).unwrap();
            controller.handle(file_event(102), after_now(1_000));
        });
    }

    #[test]
    fn a_held_jump_stops_when_the_session_gets_a_handled_mark() {
        assert_held_jump_stops(|_dir, controller| {
            controller
                .app
                .board_mut()
                .toggle_handled(Pid::new(102))
                .unwrap();
        });
    }

    #[test]
    fn a_held_jump_goes_to_the_next_session_when_the_first_one_leaves_needs_you() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = hold_fixture(dir.path());
        write(dir.path(), 102, "needs-you", 20);
        write(dir.path(), 103, "needs-you", 25);
        controller.handle(AppEvent::Dir(DirEvent::Rescan), after_now(500));
        write(dir.path(), 102, "idle", 30);
        controller.handle(file_event(102), after_now(1_000));
        controller.handle(AppEvent::Tick, after_now(JUMP_HOLD_MS));
        assert_eq!(selected_pid(&controller), Pid::new(103));
    }
}
