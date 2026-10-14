//! The event loop of `ha`: it draws the board, reads keys, and applies the
//! updates of the worker.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use crossterm::event::Event;
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::board::Board;
use crate::status::Status;
use crate::worker::Worker;

/// How long the loop waits for an event before it draws again.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Opens a web address in the browser of the user.
pub trait Opener {
    /// Opens `url`.
    ///
    /// # Errors
    /// Returns the reason as text when no browser starts.
    fn open(&self, url: &str) -> Result<(), String>;
}

/// The opener that starts the default browser of the system.
pub struct BrowserOpener;

impl Opener for BrowserOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        open::that_detached(url).map_err(|error| error.to_string())
    }
}

/// A source of terminal events.
pub trait Events {
    /// Waits up to `timeout` for one event.
    ///
    /// Returns `Ok(None)` when no event arrives in time.
    ///
    /// # Errors
    /// Returns the error of the terminal when it cannot be read.
    fn next(&mut self, timeout: Duration) -> std::io::Result<Option<Event>>;
}

/// The events of the real terminal.
pub struct TerminalEvents;

impl Events for TerminalEvents {
    fn next(&mut self, timeout: Duration) -> std::io::Result<Option<Event>> {
        if crossterm::event::poll(timeout)? {
            crossterm::event::read().map(Some)
        } else {
            Ok(None)
        }
    }
}

/// The state of one run of the event loop.
pub struct App<'a, O: Opener, Tz: TimeZone> {
    /// The runs to show.
    pub board: Board,
    /// The text for the status line.
    pub status: Status,
    opener: &'a O,
    worker: &'a Worker,
    now: &'a dyn Fn() -> DateTime<Utc>,
    tz: &'a Tz,
}

impl<'a, O: Opener, Tz: TimeZone> App<'a, O, Tz>
where
    Tz::Offset: std::fmt::Display,
{
    /// Builds the state of a run. `now` gives the current time and `tz` is
    /// the time zone for the clock times on screen.
    #[must_use]
    pub fn new(
        board: Board,
        status: Status,
        opener: &'a O,
        worker: &'a Worker,
        now: &'a dyn Fn() -> DateTime<Utc>,
        tz: &'a Tz,
    ) -> Self {
        App {
            board,
            status,
            opener,
            worker,
            now,
            tz,
        }
    }

    /// Runs the loop until the user quits.
    ///
    /// # Errors
    /// Returns the error of the terminal when it cannot draw or read.
    pub fn run<B: Backend, E: Events>(
        &mut self,
        terminal: &mut Terminal<B>,
        events: &mut E,
    ) -> std::io::Result<()> {
        let _ = (
            terminal,
            events,
            self.opener,
            self.worker,
            self.now,
            self.tz,
        );
        Err(std::io::Error::other("not implemented"))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::mpsc::{channel, Receiver, Sender};

    use chrono::{FixedOffset, TimeZone as _};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::worker::{Command, Update};

    /// Gives the events of a script, then an error, so a loop that does not
    /// stop fails at once and never hangs.
    struct ScriptedEvents(VecDeque<Option<Event>>);

    impl Events for ScriptedEvents {
        fn next(&mut self, _timeout: Duration) -> std::io::Result<Option<Event>> {
            self.0
                .pop_front()
                .ok_or_else(|| std::io::Error::other("script ended"))
        }
    }

    /// Records each address and can be set to fail.
    struct RecordingOpener {
        urls: RefCell<Vec<String>>,
        fails: bool,
    }

    impl Opener for RecordingOpener {
        fn open(&self, url: &str) -> Result<(), String> {
            self.urls.borrow_mut().push(url.to_owned());
            if self.fails {
                Err("no browser".to_owned())
            } else {
                Ok(())
            }
        }
    }

    fn opener(fails: bool) -> RecordingOpener {
        RecordingOpener {
            urls: RefCell::new(Vec::new()),
            fails,
        }
    }

    fn key_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Option<Event> {
        Some(Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }))
    }

    fn press(code: KeyCode) -> Option<Event> {
        key_event(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn script(events: Vec<Option<Event>>) -> ScriptedEvents {
        ScriptedEvents(events.into())
    }

    /// The channels that the test holds, and the worker built from the
    /// other ends.
    struct Channels {
        updates: Sender<Update>,
        commands: Receiver<Command>,
        worker: Worker,
    }

    fn channels() -> Channels {
        let (updates_tx, updates_rx) = channel();
        let (commands_tx, commands_rx) = channel();
        Channels {
            updates: updates_tx,
            commands: commands_rx,
            worker: Worker {
                updates: updates_rx,
                commands: commands_tx,
            },
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut skip = 0usize;
                for x in 0..area.width {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let symbol = buffer[(x, y)].symbol();
                    skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
                    line.push_str(symbol);
                }
                line.trim_end().to_owned()
            })
            .collect()
    }

    /// Runs the loop on a 120 by 12 terminal and returns the result and the
    /// text of the last frame.
    fn run_app(
        opener: &RecordingOpener,
        channels: &Channels,
        events: Vec<Option<Event>>,
    ) -> (std::io::Result<()>, Vec<String>) {
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let mut app = App::new(
            Board::new(Duration::from_secs(8 * 3600)),
            Status::default(),
            opener,
            &channels.worker,
            &clock,
            &tz,
        );
        let result = app.run(&mut terminal, &mut script(events));
        (result, lines(terminal.backend().buffer()))
    }

    #[test]
    fn q_stops_the_loop() {
        let (result, _) = run_app(&opener(false), &channels(), vec![press(KeyCode::Char('q'))]);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn escape_stops_the_loop() {
        let (result, _) = run_app(&opener(false), &channels(), vec![press(KeyCode::Esc)]);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn control_c_stops_the_loop() {
        let event = key_event(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        );
        let (result, _) = run_app(&opener(false), &channels(), vec![event]);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn a_released_q_does_not_stop_the_loop() {
        let release = key_event(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        let (result, _) = run_app(&opener(false), &channels(), vec![release]);
        let error = result.expect_err("the script ends before the loop stops");
        assert_eq!(error.to_string(), "script ended");
    }
}
