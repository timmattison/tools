//! The event loop of `ha`: it draws the board, reads keys, and applies the
//! updates of the worker.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::board::Board;
use crate::render;
use crate::status::{Message, Status};
use crate::worker::{Command, Update, Worker};

/// How long the loop waits for an event before it draws again.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Opens a web address in the browser of the user.
pub trait Opener {
    /// Starts to open `url`, and returns without a wait for the browser to
    /// open. A failure that comes later arrives through
    /// [`Opener::late_failure`].
    ///
    /// # Errors
    /// Returns the reason as text when no browser starts. The status line
    /// shows the reason after the address.
    fn open(&self, url: &str) -> Result<(), String>;

    /// Gives the next open that failed after [`Opener::open`] returned, or
    /// `None` when there is none.
    ///
    /// A forwarder can need some seconds to report that the browser did not
    /// open, and `open` does not wait for it. The event loop thus calls this
    /// method on each pass.
    fn late_failure(&self) -> Option<OpenFailure>;
}

/// An open that failed after [`Opener::open`] returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFailure {
    /// The address that did not open.
    pub url: String,
    /// Why it did not open.
    pub reason: String,
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
    pub fn run<B, E>(&mut self, terminal: &mut Terminal<B>, events: &mut E) -> std::io::Result<()>
    where
        B: Backend,
        B::Error: std::error::Error + Send + Sync + 'static,
        E: Events,
    {
        loop {
            self.apply_updates();
            let now = (self.now)();
            terminal
                .draw(|frame| {
                    render::draw(frame, &mut self.board, &self.status, now, self.tz);
                })
                .map_err(std::io::Error::other)?;
            let Some(event) = events.next(POLL_INTERVAL)? else {
                continue;
            };
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Press && self.handle_key(key) == Flow::Quit {
                    return Ok(());
                }
            }
        }
    }

    /// Posts each open that failed late, and applies each update that the
    /// worker has sent. A worker that has ended sends nothing more, and that
    /// is not an error.
    fn apply_updates(&mut self) {
        while let Some(failure) = self.opener.late_failure() {
            self.status.message = Some(failed_open(&failure.url, &failure.reason, (self.now)()));
        }
        while let Ok(update) = self.worker.updates.try_recv() {
            match update {
                Update::Runs { repo, runs } => self.board.replace(repo, runs),
                Update::Failed { repo, error } => self.board.fail(repo, error),
                Update::Jobs { run, jobs } => self.board.set_jobs(run, jobs),
                Update::JobsFailed { run, error } => self.board.fail_jobs(run, error),
                Update::Repos {
                    repos,
                    owner_errors,
                } => {
                    self.board.keep_only(&repos);
                    self.status.owner_errors = owner_errors;
                }
                Update::CycleDone {
                    finished_at,
                    rate,
                    backoff_until,
                } => {
                    self.status.last_refresh = Some(finished_at);
                    self.status.rate = rate;
                    self.status.backoff_until = backoff_until;
                    self.status.refreshing = false;
                }
            }
        }
    }

    /// Acts on one key press. Each key press removes the status message
    /// first. The `Enter` key can then post a new one.
    fn handle_key(&mut self, key: KeyEvent) -> Flow {
        self.status.message = None;
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Flow::Quit,
            KeyCode::Char('c') if control => Flow::Quit,
            KeyCode::Up => {
                self.board.select_previous((self.now)());
                Flow::Continue
            }
            KeyCode::Down => {
                self.board.select_next((self.now)());
                Flow::Continue
            }
            KeyCode::Char('r') => {
                let _ = self.worker.commands.send(Command::RefreshNow);
                self.status.refreshing = true;
                Flow::Continue
            }
            KeyCode::Enter => {
                if let Some(url) = self.board.selected_url((self.now)()) {
                    self.status.message = match self.opener.open(&url) {
                        Ok(()) => None,
                        Err(reason) => Some(failed_open(&url, &reason, (self.now)())),
                    };
                }
                Flow::Continue
            }
            _ => Flow::Continue,
        }
    }
}

/// Builds the status message for an open of `url` that failed for `reason`.
///
/// The address comes first, because the status line cuts a long line at its
/// end. Thus a narrow terminal still shows the address, and the user can open
/// it by hand.
fn failed_open(url: &str, reason: &str, posted_at: DateTime<Utc>) -> Message {
    Message {
        text: format!("could not open a browser: {url} ({reason})"),
        posted_at,
    }
}

/// What the loop does after a key.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::mpsc::{channel, Receiver, Sender};

    use chrono::FixedOffset;
    use crossterm::event::KeyEventState;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::run::{Run, RunId, RunNumber, RunState};
    use crate::target::{OwnerName, RepoName};
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

    /// Records each address and can be set to fail. Each failure in `late`
    /// comes back from one call of `late_failure`, in order.
    struct RecordingOpener {
        urls: RefCell<Vec<String>>,
        fails: bool,
        late: RefCell<VecDeque<OpenFailure>>,
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

        fn late_failure(&self) -> Option<OpenFailure> {
            self.late.borrow_mut().pop_front()
        }
    }

    fn opener(fails: bool) -> RecordingOpener {
        RecordingOpener {
            urls: RefCell::new(Vec::new()),
            fails,
            late: RefCell::new(VecDeque::new()),
        }
    }

    fn key_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        })
    }

    fn press(code: KeyCode) -> Event {
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

    fn repo() -> RepoName {
        RepoName::parse("acme/widgets").unwrap()
    }

    /// An active run that started `started` minutes before `now`.
    fn active_run(id: u64, started: i64) -> Run {
        Run {
            id: RunId(id),
            number: RunNumber(id + 100),
            repo: repo(),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: format!("run {id}"),
            state: RunState::Running,
            started_at: now() - chrono::Duration::minutes(started),
            updated_at: now(),
            url: format!("https://example.test/runs/{id}"),
        }
    }

    fn send_runs(channels: &Channels, runs: Vec<Run>) {
        channels
            .updates
            .send(Update::Runs { repo: repo(), runs })
            .unwrap();
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut skip = 0_usize;
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
        events: Vec<Event>,
    ) -> (std::io::Result<()>, Vec<String>) {
        run_app_from(Status::default(), opener, channels, events)
    }

    /// Like `run_app`, with a given first status.
    fn run_app_from(
        status: Status,
        opener: &RecordingOpener,
        channels: &Channels,
        events: Vec<Event>,
    ) -> (std::io::Result<()>, Vec<String>) {
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            status,
            opener,
            &channels.worker,
            &clock,
            &tz,
        );
        let result = app.run(
            &mut terminal,
            &mut script(events.into_iter().map(Some).collect()),
        );
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

    #[test]
    fn enter_opens_the_address_of_the_highlighted_run() {
        let channels = channels();
        // The older run (id 2) is the first row, the newer run (id 1) the second.
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 30)]);
        let opener = opener(false);
        let (result, _) = run_app(
            &opener,
            &channels,
            vec![
                press(KeyCode::Down),
                press(KeyCode::Enter),
                press(KeyCode::Char('q')),
            ],
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(*opener.urls.borrow(), ["https://example.test/runs/1"]);
    }

    #[test]
    fn a_failed_open_shows_the_address_and_the_loop_goes_on() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let opener = opener(true);
        let (result, frame) = run_app(
            &opener,
            &channels,
            vec![press(KeyCode::Enter), press(KeyCode::Char('q'))],
        );
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("could not open a browser: https://example.test/runs/1"),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_failed_open_shows_the_reason_of_the_opener_after_the_address() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let opener = opener(true);
        let (result, frame) = run_app(
            &opener,
            &channels,
            vec![press(KeyCode::Enter), press(KeyCode::Char('q'))],
        );
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line
                .contains("could not open a browser: https://example.test/runs/1 (no browser)"),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_failure_after_the_open_shows_the_address_and_the_reason() {
        let channels = channels();
        let opener = opener(false);
        opener.late.borrow_mut().push_back(OpenFailure {
            url: "https://example.test/runs/1".to_owned(),
            reason: "mosh-open failed".to_owned(),
        });
        let (result, frame) = run_app(&opener, &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains(
                "could not open a browser: https://example.test/runs/1 (mosh-open failed)"
            ),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_key_press_clears_the_message_and_the_error_shows() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels
            .updates
            .send(Update::Failed {
                repo: repo(),
                error: "boom".to_owned(),
            })
            .unwrap();
        let opener = opener(true);
        let (result, frame) = run_app(
            &opener,
            &channels,
            vec![
                press(KeyCode::Enter),
                press(KeyCode::Char('x')),
                press(KeyCode::Char('q')),
            ],
        );
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme/widgets: boom"),
            "{status_line:?}"
        );
        assert!(
            !status_line.contains("could not open a browser"),
            "{status_line:?}"
        );
    }

    #[test]
    fn enter_on_an_empty_board_opens_nothing() {
        let opener = opener(false);
        let (result, _) = run_app(
            &opener,
            &channels(),
            vec![press(KeyCode::Enter), press(KeyCode::Char('q'))],
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(opener.urls.borrow().is_empty());
    }

    #[test]
    fn r_asks_the_worker_for_one_refresh_and_shows_refreshing() {
        let channels = channels();
        let (result, frame) = run_app(
            &opener(false),
            &channels,
            vec![press(KeyCode::Char('r')), press(KeyCode::Char('q'))],
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(matches!(
            channels.commands.try_recv(),
            Ok(Command::RefreshNow)
        ));
        assert!(channels.commands.try_recv().is_err(), "exactly one command");
        let status_line = frame.last().unwrap();
        assert!(status_line.contains("refreshing…"), "{status_line:?}");
    }

    fn quit() -> Vec<Event> {
        vec![press(KeyCode::Char('q'))]
    }

    #[test]
    fn a_failed_update_shows_the_error_on_the_status_line() {
        let channels = channels();
        channels
            .updates
            .send(Update::Failed {
                repo: repo(),
                error: "boom".to_owned(),
            })
            .unwrap();
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme/widgets: boom"),
            "{status_line:?}"
        );
    }

    #[test]
    fn the_board_says_loading_until_the_first_cycle_ends() {
        let channels = channels();
        let (result, before) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            before[1..5],
            ["Running", "  Loading…", "Last 8h", "  Loading…"]
        );
        channels
            .updates
            .send(Update::CycleDone {
                finished_at: now(),
                rate: None,
                backoff_until: None,
            })
            .unwrap();
        let (result, after) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            after[1..5],
            [
                "Running · 0",
                "  No runs in progress or in the queue.",
                "Last 8h · 0 passed · 0 failed",
                "  No runs finished in the last 8h.",
            ]
        );
    }

    #[test]
    fn a_finished_cycle_shows_the_time_and_ends_refreshing() {
        let channels = channels();
        channels
            .updates
            .send(Update::CycleDone {
                finished_at: now(),
                rate: None,
                backoff_until: None,
            })
            .unwrap();
        let refreshing = Status {
            refreshing: true,
            ..Status::default()
        };
        let (result, frame) = run_app_from(refreshing, &opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(status_line.contains("updated 0s ago"), "{status_line:?}");
        assert!(!status_line.contains("refreshing…"), "{status_line:?}");
    }

    #[test]
    fn a_repository_list_without_a_repository_removes_its_runs() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels
            .updates
            .send(Update::Repos {
                repos: std::collections::BTreeSet::new(),
                owner_errors: Vec::new(),
            })
            .unwrap();
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        assert!(
            frame.iter().all(|line| !line.contains("run 1")),
            "{frame:?}"
        );
    }

    #[test]
    fn owner_errors_show_on_the_status_line() {
        let channels = channels();
        channels
            .updates
            .send(Update::Repos {
                repos: std::collections::BTreeSet::new(),
                owner_errors: vec![(
                    OwnerName::parse("acme").unwrap(),
                    "no such owner".to_owned(),
                )],
            })
            .unwrap();
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme: no such owner"),
            "{status_line:?}"
        );
    }

    #[test]
    fn up_and_down_move_the_highlight() {
        let channels = channels();
        send_runs(
            &channels,
            vec![active_run(1, 5), active_run(2, 20), active_run(3, 40)],
        );
        let opener = opener(false);
        // Oldest first: run 3, run 2, run 1. Down, Up, Down ends on run 2.
        let (result, _) = run_app(
            &opener,
            &channels,
            vec![
                press(KeyCode::Down),
                press(KeyCode::Up),
                press(KeyCode::Down),
                press(KeyCode::Enter),
                press(KeyCode::Char('q')),
            ],
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(*opener.urls.borrow(), ["https://example.test/runs/2"]);
    }

    #[test]
    fn an_idle_tick_and_a_resize_only_redraw() {
        let channels = channels();
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let opener = opener(false);
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            Status::default(),
            &opener,
            &channels.worker,
            &clock,
            &tz,
        );
        let events = vec![
            None,
            Some(Event::Resize(100, 10)),
            press(KeyCode::Char('q')).into(),
        ];
        let result = app.run(&mut terminal, &mut script(events));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn a_worker_that_has_ended_does_not_stop_the_loop() {
        let Channels {
            updates,
            commands,
            worker,
        } = channels();
        drop(updates);
        drop(commands);
        let channels = Channels {
            updates: channel().0,
            commands: channel().1,
            worker,
        };
        let opener = opener(false);
        let (result, _) = run_app(
            &opener,
            &channels,
            vec![press(KeyCode::Char('r')), press(KeyCode::Char('q'))],
        );
        assert!(result.is_ok(), "{result:?}");
    }
}
