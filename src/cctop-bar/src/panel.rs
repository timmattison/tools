//! When the panel shows and hides. The panel gives its keys to the cctop
//! controller and turns the effects of the controller into jobs for the shell.

use cctop::controller::{AppEvent, Controller, Effect};
use cctop::focus::FocusTarget;
use cctop::state::EpochMillis;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::keymap::PanelKey;

/// What the panel needs from the cctop controller. `cctop-bar` implements it
/// for `Controller`, and the tests use a fake.
pub trait PanelTarget {
    /// The panel is `rows` cells high.
    fn set_rows(&mut self, rows: u16);
    /// Give one key to the app at the time `now`. Return its effects.
    fn key(&mut self, key: KeyEvent, now: EpochMillis) -> Vec<Effect>;
}

impl PanelTarget for Controller {
    fn set_rows(&mut self, rows: u16) {
        let _ = rows;
    }

    fn key(&mut self, key: KeyEvent, now: EpochMillis) -> Vec<Effect> {
        let _ = (key, now, AppEvent::Tick, Event::FocusGained);
        Vec::new()
    }
}

/// An input of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelInput {
    /// The person pressed the global hotkey. The panel is `rows` cells high.
    Hotkey {
        /// The rows of the panel.
        rows: u16,
    },
    /// The person chose the menu item. The panel is `rows` cells high.
    ShowItem {
        /// The rows of the panel.
        rows: u16,
    },
    /// The person pressed a key while the panel has the key focus.
    Key(PanelKey),
    /// The panel lost the key focus.
    ResignKey,
}

/// A job for the shell of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelJob {
    /// Show the panel and give it the key focus.
    Show,
    /// Hide the panel.
    Hide,
    /// Focus the terminal of a session.
    Focus(FocusTarget),
    /// Send a notification with this text.
    Notify(String),
}

/// The state of the panel: shown or hidden.
#[derive(Debug, Default)]
pub struct Panel {
    visible: bool,
}

impl Panel {
    /// A hidden panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tells if the panel is shown.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Handle one input at the time `now`. Return the jobs to run, in order.
    pub fn handle(
        &mut self,
        input: PanelInput,
        target: &mut impl PanelTarget,
        now: EpochMillis,
    ) -> Vec<PanelJob> {
        match input {
            PanelInput::Hotkey { rows } if !self.visible => self.show(rows, target, now),
            PanelInput::ShowItem { rows } => self.show(rows, target, now),
            _ => Vec::new(),
        }
    }

    /// Show the panel at `rows` rows with the first row selected.
    fn show(
        &mut self,
        rows: u16,
        target: &mut impl PanelTarget,
        now: EpochMillis,
    ) -> Vec<PanelJob> {
        target.set_rows(rows);
        // The keys of the panel select the first row. The effects of Home are none.
        target.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE), now);
        self.visible = true;
        vec![PanelJob::Show]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use cctop::state::parse_state_file;

    use super::*;

    const NOW: EpochMillis = EpochMillis::new(1_794_824_300_000);

    /// A call that the fake target got.
    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Rows(u16),
        Key(KeyEvent),
    }

    /// A target that records its calls and answers from a queue.
    #[derive(Default)]
    struct FakeTarget {
        calls: Vec<Call>,
        answers: VecDeque<Vec<Effect>>,
    }

    impl FakeTarget {
        fn answering(answers: Vec<Vec<Effect>>) -> Self {
            Self {
                calls: Vec::new(),
                answers: answers.into(),
            }
        }
    }

    impl PanelTarget for FakeTarget {
        fn set_rows(&mut self, rows: u16) {
            self.calls.push(Call::Rows(rows));
        }

        fn key(&mut self, key: KeyEvent, _now: EpochMillis) -> Vec<Effect> {
            self.calls.push(Call::Key(key));
            self.answers.pop_front().unwrap_or_default()
        }
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A focus target of a session of the fixture.
    fn focus_target() -> FocusTarget {
        let json = r#"{"version":1,"pid":4242,"sessionId":"s","startedAt":1,"state":"needs-you","stamp":2,"cwd":"/x"}"#;
        FocusTarget {
            pid: parse_state_file(json).unwrap().pid,
            label: "x".to_string(),
            zellij: None,
            run: cctop::liveness::RunStatus::Running,
        }
    }

    #[test]
    fn the_hotkey_shows_the_panel_and_selects_the_first_row() {
        let mut panel = Panel::new();
        let mut target = FakeTarget::answering(vec![vec![Effect::Quit]]);
        let jobs = panel.handle(PanelInput::Hotkey { rows: 30 }, &mut target, NOW);
        assert_eq!(jobs, vec![PanelJob::Show]);
        assert!(panel.is_visible());
        assert_eq!(
            target.calls,
            vec![Call::Rows(30), Call::Key(plain(KeyCode::Home))]
        );
    }

    #[test]
    fn the_menu_item_shows_the_panel_even_when_it_is_shown() {
        let mut panel = Panel::new();
        let mut target = FakeTarget::default();
        panel.handle(PanelInput::Hotkey { rows: 30 }, &mut target, NOW);
        let jobs = panel.handle(PanelInput::ShowItem { rows: 40 }, &mut target, NOW);
        assert_eq!(jobs, vec![PanelJob::Show]);
        assert!(panel.is_visible());
        assert_eq!(
            &target.calls[2..],
            &[Call::Rows(40), Call::Key(plain(KeyCode::Home))]
        );
    }
}
