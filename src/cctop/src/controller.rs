//! The logic of the main loop, with no terminal in it. The controller takes
//! one event, changes the app, and returns the effects that the caller runs.

use crossterm::event::Event;

use crate::app::{Action, App};
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
            AppEvent::Terminal(Event::Key(key)) => match self.app.handle_key(key) {
                Action::Nothing => Vec::new(),
                Action::Quit => vec![Effect::Quit],
                Action::Focus(target) => vec![Effect::Focus(target)],
            },
            AppEvent::Terminal(_) => Vec::new(),
            AppEvent::Dir(event) => {
                let changes =
                    self.sync
                        .on_dir_event(self.app.board_mut(), event, self.clock.as_mut(), now);
                if !self.notify {
                    return Vec::new();
                }
                changes
                    .iter()
                    .filter(|change| change.is_needs_you())
                    .map(|change| Effect::Notify(needs_you_message(change)))
                    .collect()
            }
            AppEvent::Tick => {
                let _ended = self
                    .sync
                    .on_tick(self.app.board_mut(), self.clock.as_mut(), now);
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

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use proctree::Pid;

    use super::*;
    use crate::store::test_support::{clock_with, state_json};

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
        let mut controller =
            Controller::new(sync.clone(), Box::new(clock_with(100, START_SECS)), true);
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
}
