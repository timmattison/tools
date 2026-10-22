//! The event loop of `ha`: it draws the board, reads keys, and applies the
//! updates of the worker.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::board::Board;
use crate::cancel::{Canceller, Confirm};
use crate::first_cycle::{FirstCycle, NoTargetRead, Verdict};
use crate::render;
use crate::run::RunId;
use crate::status::{Message, Status};
use crate::target::RepoName;
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
pub struct App<'a, O: Opener, C: Canceller, Tz: TimeZone> {
    /// The runs to show.
    pub board: Board,
    /// The text for the status line.
    pub status: Status,
    opener: &'a O,
    canceller: &'a C,
    worker: &'a Worker,
    now: &'a dyn Fn() -> DateTime<Utc>,
    tz: &'a Tz,
    /// The set of the last `Command::WatchJobs` that the app sent. It is
    /// empty at the start.
    watched: BTreeSet<(RepoName, RunId)>,
    /// The box that asks the user to cancel a run. It is `None` when no box
    /// shows.
    confirm: Option<Confirm>,
    /// The tally of the first refresh cycle. It is `None` after the first
    /// cycle that gives a verdict. A cycle that ends with a spent API budget
    /// gives none, and the tally then starts again for the next cycle.
    first_cycle: Option<FirstCycle>,
}

/// Why the event loop ended with an error.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The terminal could not clear, draw or read.
    #[error(transparent)]
    Terminal(#[from] std::io::Error),
    /// The first refresh cycle that did not end with a spent API budget read
    /// no target.
    #[error(transparent)]
    NoTargetRead(#[from] NoTargetRead),
}

impl<'a, O: Opener, C: Canceller, Tz: TimeZone> App<'a, O, C, Tz>
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
        canceller: &'a C,
        worker: &'a Worker,
        now: &'a dyn Fn() -> DateTime<Utc>,
        tz: &'a Tz,
    ) -> Self {
        App {
            board,
            status,
            opener,
            canceller,
            worker,
            now,
            tz,
            watched: BTreeSet::new(),
            confirm: None,
            first_cycle: Some(FirstCycle::default()),
        }
    }

    /// Clears the whole screen one time, then runs the loop until the user
    /// quits, or until the first refresh cycle reads no target. A cycle that
    /// ends with a spent API budget does not count, and the next cycle
    /// decides. The loop draws no frame after it finds that.
    ///
    /// # Errors
    /// Returns [`RunError::Terminal`] when the terminal cannot clear, draw or
    /// read. Returns [`RunError::NoTargetRead`] when the first refresh cycle
    /// ends with no good repository fetch, at least one failed target, and an
    /// API budget that is not spent.
    pub fn run<B, E>(&mut self, terminal: &mut Terminal<B>, events: &mut E) -> Result<(), RunError>
    where
        B: Backend,
        B::Error: std::error::Error + Send + Sync + 'static,
        E: Events,
    {
        // This clear is not a duplicate of the entry to the alternate screen.
        // mosh ignores mode 1049, so the text of the shell is still on the
        // screen. ratatui sends no clear at startup, and its first frame
        // writes only the cells that are not blank. Without this clear, the
        // shell text stays in each cell where the view draws a blank.
        //
        // A resize to the current size clears the whole screen and resets
        // the back buffer. `Terminal::clear` does the same, but it also asks
        // the terminal for the cursor position and waits for the answer. If
        // the terminal does not answer, that wait stops `ha` at the start.
        let size = terminal.size().map_err(std::io::Error::other)?;
        terminal
            .resize(size.into())
            .map_err(std::io::Error::other)?;
        loop {
            self.apply_updates()?;
            self.sync_watch();
            let now = (self.now)();
            terminal
                .draw(|frame| {
                    render::draw(
                        frame,
                        &mut self.board,
                        &self.status,
                        now,
                        self.tz,
                        self.confirm.as_ref(),
                    );
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

    /// Posts each open that failed late and the result of each cancel, and
    /// applies each update that the worker has sent. A worker that has ended
    /// sends nothing more, and that is not an error.
    ///
    /// # Errors
    /// Returns [`NoTargetRead`] when the first refresh cycle ends with no
    /// good repository fetch, at least one failed target, and an API budget
    /// that is not spent. When the budget is spent, the tally starts again
    /// and the next cycle decides.
    fn apply_updates(&mut self) -> Result<(), NoTargetRead> {
        while let Some(failure) = self.opener.late_failure() {
            self.status.message = Some(failed_open(&failure.url, &failure.reason, (self.now)()));
        }
        while let Some(outcome) = self.canceller.finished() {
            // The run ID comes before the error, because the status line cuts
            // a long line at its end. Thus a narrow terminal still shows which
            // run the error is about.
            match outcome.result {
                Ok(()) => {
                    self.post(format!("GitHub is cancelling run {}", outcome.run.0));
                    self.refresh_now();
                }
                Err(error) => {
                    self.post(format!("could not cancel run {}: {error}", outcome.run.0));
                }
            }
        }
        while let Ok(update) = self.worker.updates.try_recv() {
            if let Some(first_cycle) = &mut self.first_cycle {
                first_cycle.note(&update);
            }
            match update {
                Update::Runs { repo, runs } => self.board.replace(repo, runs, (self.now)()),
                Update::Failed { repo, error } => self.board.fail(repo, error.to_string()),
                Update::Suspect { repo, reason } => self.board.fail(repo, reason.to_string()),
                Update::Jobs { run, jobs } => self.board.set_jobs(run, jobs),
                Update::JobsFailed { run, error } => self.board.fail_jobs(run, error),
                Update::Repos {
                    repos,
                    owner_errors,
                    listed_at: _,
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
                    if let Some(first_cycle) = self.first_cycle.take() {
                        if first_cycle.verdict(rate)? == Verdict::BudgetSpent {
                            self.first_cycle = Some(FirstCycle::default());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Tells the worker which expanded runs to watch.
    ///
    /// It reads the expanded runs from the board. When the set differs from
    /// the last set that it sent, it sends the whole set in a
    /// `Command::WatchJobs` and keeps it. When the set is the same, it sends
    /// nothing. A worker that has ended is not an error.
    fn sync_watch(&mut self) {
        let expanded = self.board.expanded((self.now)());
        if expanded != self.watched {
            let _ = self
                .worker
                .commands
                .send(Command::WatchJobs(expanded.clone()));
            self.watched = expanded;
        }
    }

    /// Acts on one key press. Each key press removes the status message
    /// first. The `Enter` key can then post a new one. `Right` expands the
    /// run of the highlighted row. `Left` collapses that run, also from a job
    /// row. `Enter` opens the highlighted run or job.
    ///
    /// `c` opens a box that asks to cancel the run of the highlighted row.
    /// On a finished run it posts that there is nothing to cancel instead.
    /// While the box shows, it takes every key. `y` cancels the run that the
    /// box names, Ctrl-C quits, and every other key closes the box and does
    /// nothing more. Thus `q`, `Esc`, and `Enter` do not quit, open, or move
    /// anything while the box shows.
    fn handle_key(&mut self, key: KeyEvent) -> Flow {
        self.status.message = None;
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(confirm) = self.confirm.take() {
            return match key.code {
                KeyCode::Char('c') if control => Flow::Quit,
                KeyCode::Char('y' | 'Y') if !control => {
                    self.canceller.cancel(&confirm.repo, confirm.run);
                    self.post(format!("cancelling run {}…", confirm.run.0));
                    Flow::Continue
                }
                _ => Flow::Continue,
            };
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Flow::Quit,
            KeyCode::Char('c') if control => Flow::Quit,
            KeyCode::Char('c') => {
                self.ask_to_cancel();
                Flow::Continue
            }
            KeyCode::Up => {
                self.board.select_previous((self.now)());
                Flow::Continue
            }
            KeyCode::Down => {
                self.board.select_next((self.now)());
                Flow::Continue
            }
            KeyCode::Right => {
                self.board.expand((self.now)());
                self.sync_watch();
                Flow::Continue
            }
            KeyCode::Left => {
                self.board.collapse((self.now)());
                self.sync_watch();
                Flow::Continue
            }
            KeyCode::Char('r') => {
                self.refresh_now();
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

    /// Opens the box that asks to cancel the highlighted run, or posts that a
    /// finished run has nothing to cancel. The box keeps a copy of what it
    /// shows, thus a later refresh does not change the run that `y` cancels.
    /// On a job row it names the run of the job.
    fn ask_to_cancel(&mut self) {
        let now = (self.now)();
        let target = self.board.selected_target(now).map(|(run, job)| {
            if run.state.is_active() {
                Ok(Confirm::new(run, job))
            } else {
                Err(run.id)
            }
        });
        match target {
            None => {}
            Some(Ok(confirm)) => self.confirm = Some(confirm),
            Some(Err(id)) => {
                self.post(format!("run {} finished: there is nothing to cancel", id.0));
            }
        }
    }

    /// Asks the worker for one refresh now and shows `refreshing…`. A worker
    /// that has ended is not an error.
    fn refresh_now(&mut self) {
        let _ = self.worker.commands.send(Command::RefreshNow);
        self.status.refreshing = true;
    }

    /// Shows `text` on the status line, stamped with the current time.
    fn post(&mut self, text: String) {
        self.status.message = Some(Message {
            text,
            posted_at: (self.now)(),
        });
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
    use ratatui::backend::{ClearType, TestBackend, WindowSize};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Size};

    use super::*;
    use crate::cancel::CancelOutcome;
    use crate::github::{RepoList, SourceError};
    use crate::run::{Run, RunId, RunState};
    use crate::status::{OwnerProblem, RateLimit};
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

    /// Records each cancel. Each outcome in `outcomes` comes back from one
    /// call of `finished`, in order.
    struct RecordingCanceller {
        cancels: RefCell<Vec<(RepoName, RunId)>>,
        outcomes: RefCell<VecDeque<CancelOutcome>>,
    }

    impl Canceller for RecordingCanceller {
        fn cancel(&self, repo: &RepoName, run: RunId) {
            self.cancels.borrow_mut().push((repo.clone(), run));
        }

        fn finished(&self) -> Option<CancelOutcome> {
            self.outcomes.borrow_mut().pop_front()
        }
    }

    fn canceller() -> RecordingCanceller {
        RecordingCanceller {
            cancels: RefCell::new(Vec::new()),
            outcomes: RefCell::new(VecDeque::new()),
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
            repo: repo(),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: format!("run {id}"),
            state: RunState::Running,
            started_at: now() - chrono::Duration::minutes(started),
            created_at: now() - chrono::Duration::minutes(started),
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
    ) -> (Result<(), RunError>, Vec<String>) {
        run_app_from(Status::default(), opener, channels, events)
    }

    /// Like `run_app`, with a given first status.
    fn run_app_from(
        status: Status,
        opener: &RecordingOpener,
        channels: &Channels,
        events: Vec<Event>,
    ) -> (Result<(), RunError>, Vec<String>) {
        run_app_with(status, opener, &canceller(), channels, events)
    }

    /// Does the work of `run_app_from` with a canceller that the caller
    /// keeps, so the caller can read the cancels that the app sent.
    fn run_app_with(
        status: Status,
        opener: &RecordingOpener,
        canceller: &RecordingCanceller,
        channels: &Channels,
        events: Vec<Event>,
    ) -> (Result<(), RunError>, Vec<String>) {
        run_app_on(
            TestBackend::new(120, 12),
            status,
            opener,
            canceller,
            channels,
            events,
        )
    }

    /// Does the work of `run_app_with` on `backend`, so the caller can give
    /// a screen that already holds text.
    fn run_app_on(
        backend: TestBackend,
        status: Status,
        opener: &RecordingOpener,
        canceller: &RecordingCanceller,
        channels: &Channels,
        events: Vec<Event>,
    ) -> (Result<(), RunError>, Vec<String>) {
        let mut terminal = Terminal::new(backend).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            status,
            opener,
            canceller,
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
                error: SourceError::Failed("boom".to_owned()),
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
            status_line.contains("acme/widgets: gh failed: boom"),
            "{status_line:?}"
        );
        assert!(
            !status_line.contains("could not open a browser"),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_suspect_answer_keeps_the_runs_and_shows_the_reason() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels
            .updates
            .send(Update::Suspect {
                repo: repo(),
                reason: crate::suspect::Suspicion::LeftOut(crate::suspect::Item::Run(RunId(7))),
            })
            .unwrap();
        let opener = opener(true);
        let (result, frame) = run_app(
            &opener,
            &channels,
            vec![press(KeyCode::Char('x')), press(KeyCode::Char('q'))],
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(
            frame.iter().any(|line| line.contains("run 1")),
            "the run stays: {frame:?}"
        );
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme/widgets: GitHub left out run 7"),
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

    /// The lines of the frame that show a run row of `acme/widgets`.
    fn run_rows(frame: &[String]) -> Vec<&String> {
        frame
            .iter()
            .filter(|line| line.contains("acme/widgets") && line.contains(" main "))
            .collect()
    }

    #[test]
    fn a_failed_read_marks_every_run_row_of_its_repository_as_stale() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 4)]);
        channels
            .updates
            .send(Update::Failed {
                repo: repo(),
                error: SourceError::Failed("boom".to_owned()),
            })
            .unwrap();
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let rows = run_rows(&frame);
        assert_eq!(rows.len(), 2, "{frame:?}");
        assert!(rows.iter().all(|row| row.contains('\u{1F4A4}')), "{rows:?}");
    }

    #[test]
    fn a_suspect_read_marks_every_run_row_of_its_repository_as_stale() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 4)]);
        channels
            .updates
            .send(Update::Suspect {
                repo: repo(),
                reason: crate::suspect::Suspicion::LeftOut(crate::suspect::Item::Run(RunId(9))),
            })
            .unwrap();
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let rows = run_rows(&frame);
        assert_eq!(rows.len(), 2, "{frame:?}");
        assert!(rows.iter().all(|row| row.contains('\u{1F4A4}')), "{rows:?}");
    }

    #[test]
    fn a_good_read_after_a_failure_clears_the_stale_mark() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels
            .updates
            .send(Update::Failed {
                repo: repo(),
                error: SourceError::Failed("boom".to_owned()),
            })
            .unwrap();
        send_runs(&channels, vec![active_run(1, 5)]);
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let rows = run_rows(&frame);
        assert_eq!(rows.len(), 1, "{frame:?}");
        assert!(!rows[0].contains('\u{1F4A4}'), "{rows:?}");
    }

    /// The status line of the last frame, after `quit`.
    fn status_line_after(channels: &Channels, status: Status) -> String {
        let (result, frame) = run_app_from(status, &opener(false), channels, quit());
        assert!(result.is_ok(), "{result:?}");
        frame.last().unwrap().clone()
    }

    fn failed_update() -> Update {
        Update::Failed {
            repo: repo(),
            error: SourceError::Failed("boom".to_owned()),
        }
    }

    #[test]
    fn a_repository_read_well_and_then_failed_shows_since_when_it_is_stale() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels.updates.send(failed_update()).unwrap();
        let line = status_line_after(&channels, Status::default());
        assert!(
            line.contains("\u{1F4A4} stale since 12:00 (1 repo)"),
            "{line:?}"
        );
    }

    #[test]
    fn a_cycle_in_which_each_fetch_fails_does_not_say_updated() {
        let channels = channels();
        channels.updates.send(failed_update()).unwrap();
        channels
            .updates
            .send(Update::CycleDone {
                finished_at: now(),
                rate: None,
                backoff_until: None,
            })
            .unwrap();
        let line = status_line_after(&channels, Status::default());
        assert!(!line.contains("updated 0s ago"), "{line:?}");
        assert!(line.contains("\u{1F4A4} stale (1 repo)"), "{line:?}");
    }

    #[test]
    fn a_live_message_and_the_stale_part_show_together() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels.updates.send(failed_update()).unwrap();
        let status = Status {
            message: Some(Message {
                text: "GitHub is cancelling run 1".to_owned(),
                posted_at: now(),
            }),
            ..Status::default()
        };
        let line = status_line_after(&channels, status);
        assert!(line.contains("GitHub is cancelling run 1"), "{line:?}");
        assert!(
            line.contains("\u{1F4A4} stale since 12:00 (1 repo)"),
            "{line:?}"
        );
    }

    #[test]
    fn the_stale_part_goes_away_after_a_good_read() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels.updates.send(failed_update()).unwrap();
        send_runs(&channels, vec![active_run(1, 5)]);
        let line = status_line_after(&channels, Status::default());
        assert!(!line.contains('\u{1F4A4}'), "{line:?}");
    }

    #[test]
    fn a_stale_owner_shows_the_time_of_its_last_good_list() {
        let channels = channels();
        let acme = OwnerName::parse("acme").unwrap();
        channels
            .updates
            .send(Update::Repos {
                repos: std::collections::BTreeSet::new(),
                listed_at: std::collections::BTreeMap::from([(
                    acme.clone(),
                    now() - chrono::Duration::minutes(18),
                )]),
                owner_errors: vec![(
                    acme,
                    OwnerProblem::Failed(SourceError::Failed("boom".to_owned())),
                )],
            })
            .unwrap();
        let line = status_line_after(&channels, Status::default());
        assert!(
            line.contains("\u{1F4A4} stale since 11:42 (1 owner)"),
            "{line:?}"
        );
    }

    #[test]
    fn a_failed_update_shows_the_error_on_the_status_line() {
        let channels = channels();
        channels
            .updates
            .send(Update::Failed {
                repo: repo(),
                error: SourceError::Failed("boom".to_owned()),
            })
            .unwrap();
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme/widgets: gh failed: boom"),
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
            ["Running", "   Loading…", "Last 8h", "   Loading…"]
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
                "   No runs in progress or in the queue.",
                "Last 8h · 0 passed · 0 failed · 0 cancelled",
                "   No runs finished in the last 8h.",
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

    fn not_found() -> SourceError {
        SourceError::Http {
            status: 404,
            message: "Not Found".to_owned(),
        }
    }

    fn cycle_done() -> Update {
        Update::CycleDone {
            finished_at: now(),
            rate: None,
            backoff_until: None,
        }
    }

    /// The error that GitHub gives when the API budget is spent.
    fn rate_limited() -> SourceError {
        SourceError::Http {
            status: 403,
            message: "rate limit hit".to_owned(),
        }
    }

    /// The end of a cycle that spent the whole API budget.
    fn cycle_done_with_spent_budget() -> Update {
        let reset = now() + chrono::Duration::minutes(20);
        Update::CycleDone {
            finished_at: now(),
            rate: Some(RateLimit {
                limit: 5000,
                remaining: 0,
                reset,
            }),
            backoff_until: Some(reset),
        }
    }

    /// Sends the repositories `acme/a` and `acme/b`, a 403 rate-limit failure
    /// for each, and the end of a cycle with a spent budget.
    fn send_first_cycle_with_spent_budget(channels: &Channels) {
        let a = RepoName::parse("acme/a").unwrap();
        let b = RepoName::parse("acme/b").unwrap();
        let updates = [
            Update::Repos {
                repos: BTreeSet::from([a.clone(), b.clone()]),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![],
            },
            Update::Failed {
                repo: a,
                error: rate_limited(),
            },
            Update::Failed {
                repo: b,
                error: rate_limited(),
            },
            cycle_done_with_spent_budget(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
    }

    #[test]
    fn a_first_cycle_that_ends_with_a_spent_budget_keeps_the_view() {
        let channels = channels();
        send_first_cycle_with_spent_budget(&channels);
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("API low 0/5000, next refresh"),
            "{status_line:?}"
        );
    }

    #[test]
    fn after_a_spent_budget_the_next_cycle_gives_the_verdict() {
        let channels = channels();
        send_first_cycle_with_spent_budget(&channels);
        let updates = [
            Update::Failed {
                repo: RepoName::parse("acme/a").unwrap(),
                error: not_found(),
            },
            Update::Failed {
                repo: RepoName::parse("acme/b").unwrap(),
                error: not_found(),
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, _) = run_app(&opener(false), &channels, vec![]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "the first refresh could not read any target:\n  \
             acme/a: GitHub answered 404: Not Found\n  \
             acme/b: GitHub answered 404: Not Found\n\
             GitHub answers 404 for a repository that does not exist, and for a private \
             repository that this gh account cannot read.\n\
             Run gh auth status to see the account."
        );
    }

    #[test]
    fn after_a_spent_budget_a_good_fetch_keeps_the_view() {
        let channels = channels();
        send_first_cycle_with_spent_budget(&channels);
        let a = RepoName::parse("acme/a").unwrap();
        let b = RepoName::parse("acme/b").unwrap();
        let updates = [
            Update::Runs {
                repo: a.clone(),
                runs: vec![],
            },
            Update::Failed {
                repo: b.clone(),
                error: not_found(),
            },
            cycle_done(),
            Update::Failed {
                repo: a,
                error: not_found(),
            },
            Update::Failed {
                repo: b,
                error: not_found(),
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, _) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn a_first_cycle_with_one_good_fetch_of_zero_runs_keeps_the_view() {
        let channels = channels();
        let a = RepoName::parse("acme/a").unwrap();
        let b = RepoName::parse("acme/b").unwrap();
        let updates = [
            Update::Repos {
                repos: BTreeSet::from([a.clone(), b.clone()]),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![],
            },
            Update::Runs {
                repo: a,
                runs: vec![],
            },
            Update::Failed {
                repo: b,
                error: not_found(),
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme/b: GitHub answered 404: Not Found"),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_later_cycle_in_which_every_fetch_fails_keeps_the_view_and_the_last_runs() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels.updates.send(cycle_done()).unwrap();
        let (frame, _) = run_steps(
            &channels,
            vec![
                Step::Idle,
                Step::Send(Update::Failed {
                    repo: repo(),
                    error: not_found(),
                }),
                Step::Send(cycle_done()),
                Step::Idle,
                key(KeyCode::Char('q')),
            ],
        );
        assert!(frame.iter().any(|line| line.contains("run 1")), "{frame:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme/widgets: GitHub answered 404: Not Found"),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_first_cycle_that_reads_no_repository_ends_the_view_with_each_error() {
        let channels = channels();
        let a = RepoName::parse("acme/a").unwrap();
        let b = RepoName::parse("acme/b").unwrap();
        let updates = [
            Update::Repos {
                repos: BTreeSet::from([a.clone(), b.clone()]),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![],
            },
            Update::Failed {
                repo: b,
                error: SourceError::TimedOut(Duration::from_secs(30)),
            },
            Update::Failed {
                repo: a,
                error: SourceError::Http {
                    status: 403,
                    message: "Resource not accessible by integration".to_owned(),
                },
            },
            Update::CycleDone {
                finished_at: now(),
                rate: None,
                backoff_until: None,
            },
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, _) = run_app(&opener(false), &channels, vec![]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "the first refresh could not read any target:\n  \
             acme/a: GitHub answered 403: Resource not accessible by integration\n  \
             acme/b: gh did not finish within 30s"
        );
    }

    #[test]
    fn a_404_adds_one_hint_about_a_wrong_name_or_a_private_repository() {
        let channels = channels();
        let a = RepoName::parse("acme/a").unwrap();
        let b = RepoName::parse("acme/b").unwrap();
        let c = RepoName::parse("acme/c").unwrap();
        let updates = [
            Update::Repos {
                repos: BTreeSet::from([a.clone(), b.clone(), c.clone()]),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![],
            },
            Update::Failed {
                repo: a,
                error: not_found(),
            },
            Update::Failed {
                repo: b,
                error: not_found(),
            },
            Update::Failed {
                repo: c,
                error: SourceError::TimedOut(Duration::from_secs(30)),
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, _) = run_app(&opener(false), &channels, vec![]);
        // Two failures are a 404, and the hint shows one time.
        assert_eq!(
            result.unwrap_err().to_string(),
            "the first refresh could not read any target:\n  \
             acme/a: GitHub answered 404: Not Found\n  \
             acme/b: GitHub answered 404: Not Found\n  \
             acme/c: gh did not finish within 30s\n\
             GitHub answers 404 for a repository that does not exist, and for a private \
             repository that this gh account cannot read.\n\
             Run gh auth status to see the account."
        );
    }

    #[test]
    fn a_first_cycle_whose_only_owner_fails_to_list_ends_the_view() {
        let channels = channels();
        let message = "GraphQL: Could not resolve to a User with the login of 'acme'.";
        let updates = [
            Update::Repos {
                repos: BTreeSet::new(),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(
                    OwnerName::parse("acme").unwrap(),
                    OwnerProblem::Failed(SourceError::Failed(message.to_owned())),
                )],
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, _) = run_app(&opener(false), &channels, vec![]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "the first refresh could not read any target:\n  \
             acme: gh failed: GraphQL: Could not resolve to a User with the login of 'acme'."
        );
    }

    #[test]
    fn an_owner_with_an_empty_list_and_no_other_target_keeps_the_view() {
        let channels = channels();
        let updates = [
            Update::Repos {
                repos: BTreeSet::new(),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![],
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        assert!(
            frame.contains(&"   No runs in progress or in the queue.".to_owned()),
            "{frame:?}"
        );
        assert!(
            frame.contains(&"   No runs finished in the last 8h.".to_owned()),
            "{frame:?}"
        );
    }

    #[test]
    fn a_cut_owner_list_is_not_called_a_failure() {
        let channels = channels();
        let a = RepoName::parse("acme/a").unwrap();
        let cut = RepoList {
            repos: vec![],
            cut: true,
        };
        let updates = [
            Update::Repos {
                repos: BTreeSet::from([a.clone()]),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(
                    OwnerName::parse("acme").unwrap(),
                    OwnerProblem::Cut(cut.warning().unwrap()),
                )],
            },
            Update::Failed {
                repo: a,
                error: SourceError::TimedOut(Duration::from_secs(30)),
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, _) = run_app(&opener(false), &channels, vec![]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "the first refresh could not read any target:\n  \
             acme/a: gh did not finish within 30s"
        );
    }

    #[test]
    fn a_failed_owner_and_one_good_fetch_keep_the_view() {
        let channels = channels();
        let x = RepoName::parse("other/x").unwrap();
        let updates = [
            Update::Repos {
                repos: BTreeSet::from([x.clone()]),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(
                    OwnerName::parse("acme").unwrap(),
                    OwnerProblem::Failed(SourceError::Failed("boom".to_owned())),
                )],
            },
            Update::Runs {
                repo: x,
                runs: vec![],
            },
            cycle_done(),
        ];
        for update in updates {
            channels.updates.send(update).unwrap();
        }
        let (result, frame) = run_app(&opener(false), &channels, quit());
        assert!(result.is_ok(), "{result:?}");
        let status_line = frame.last().unwrap();
        assert!(
            status_line.contains("acme: gh failed: boom"),
            "{status_line:?}"
        );
    }

    #[test]
    fn a_repository_list_without_a_repository_removes_its_runs() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        channels
            .updates
            .send(Update::Repos {
                repos: std::collections::BTreeSet::new(),
                listed_at: std::collections::BTreeMap::new(),
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
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: vec![(
                    OwnerName::parse("acme").unwrap(),
                    OwnerProblem::Cut("no such owner".to_owned()),
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
        let canceller = canceller();
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            Status::default(),
            &opener,
            &canceller,
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

    /// One step of a `StepEvents` script.
    enum Step {
        /// A key or other event for the loop.
        Key(Event),
        /// A pass with no event.
        Idle,
        /// An update that the worker sends before the next pass.
        Send(Update),
    }

    /// Gives the steps of a script. Each leading `Send` step goes to the
    /// loop first, so an update can arrive after a key. When the steps end,
    /// it gives an error, so a loop that does not stop fails and never hangs.
    struct StepEvents {
        steps: VecDeque<Step>,
        updates: Sender<Update>,
    }

    impl Events for StepEvents {
        fn next(&mut self, _timeout: Duration) -> std::io::Result<Option<Event>> {
            while let Some(Step::Send(_)) = self.steps.front() {
                if let Some(Step::Send(update)) = self.steps.pop_front() {
                    self.updates.send(update).unwrap();
                }
            }
            match self.steps.pop_front() {
                Some(Step::Key(event)) => Ok(Some(event)),
                Some(Step::Idle | Step::Send(_)) => Ok(None),
                None => Err(std::io::Error::other("script ended")),
            }
        }
    }

    /// Runs the loop on the steps and returns the text of the last frame and
    /// each command that the app sent to the worker.
    fn run_steps(channels: &Channels, steps: Vec<Step>) -> (Vec<String>, Vec<Command>) {
        run_steps_with(channels, &opener(false), steps)
    }

    /// Does the work of `run_steps` with an opener that the caller keeps, so
    /// the caller can read the addresses that the app opened.
    fn run_steps_with(
        channels: &Channels,
        opener: &RecordingOpener,
        steps: Vec<Step>,
    ) -> (Vec<String>, Vec<Command>) {
        run_steps_all(channels, opener, &canceller(), steps)
    }

    /// Does the work of `run_steps` with a canceller that the caller keeps,
    /// so the caller can read the cancels that the app sent.
    fn run_steps_cancelling(
        channels: &Channels,
        canceller: &RecordingCanceller,
        steps: Vec<Step>,
    ) -> (Vec<String>, Vec<Command>) {
        run_steps_all(channels, &opener(false), canceller, steps)
    }

    /// Runs the steps with an opener and a canceller that the caller keeps.
    fn run_steps_all(
        channels: &Channels,
        opener: &RecordingOpener,
        canceller: &RecordingCanceller,
        steps: Vec<Step>,
    ) -> (Vec<String>, Vec<Command>) {
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            Status::default(),
            opener,
            canceller,
            &channels.worker,
            &clock,
            &tz,
        );
        let mut events = StepEvents {
            steps: steps.into(),
            updates: channels.updates.clone(),
        };
        let result = app.run(&mut terminal, &mut events);
        assert!(result.is_ok(), "{result:?}");
        (
            lines(terminal.backend().buffer()),
            channels.commands.try_iter().collect::<Vec<_>>(),
        )
    }

    fn key(code: KeyCode) -> Step {
        Step::Key(press(code))
    }

    /// The set that `Command::WatchJobs` carries for runs with these IDs.
    fn watch(ids: &[u64]) -> Command {
        Command::WatchJobs(ids.iter().map(|id| (repo(), RunId(*id))).collect())
    }

    fn job(id: u64, name: &str) -> crate::job::Job {
        crate::job::Job {
            id: crate::job::JobId(id),
            name: name.to_owned(),
            state: RunState::Running,
            runner: None,
            created_at: None,
            started_at: Some(now()),
            completed_at: None,
            url: String::new(),
        }
    }

    fn row_of(frame: &[String], needle: &str) -> usize {
        frame
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?} in {frame:?}"))
    }

    #[test]
    fn right_watches_the_highlighted_run_and_shows_loading() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let (frame, commands) = run_steps(
            &channels,
            vec![key(KeyCode::Right), key(KeyCode::Char('q'))],
        );
        assert_eq!(commands, [watch(&[1])]);
        assert_eq!(
            row_of(&frame, "└ Loading jobs…"),
            row_of(&frame, "run 1") + 1,
            "{frame:?}"
        );
    }

    #[test]
    fn right_on_an_expanded_run_sends_no_second_command() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            key(KeyCode::Right),
            key(KeyCode::Char('q')),
        ];
        let (_, commands) = run_steps(&channels, steps);
        assert_eq!(commands, [watch(&[1])]);
    }

    #[test]
    fn left_after_right_stops_the_watch_and_removes_the_job_block() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            key(KeyCode::Left),
            key(KeyCode::Char('q')),
        ];
        let (frame, commands) = run_steps(&channels, steps);
        assert_eq!(commands, [watch(&[1]), watch(&[])]);
        assert!(
            frame.iter().all(|line| !line.contains("Loading jobs")),
            "{frame:?}"
        );
    }

    #[test]
    fn left_on_a_collapsed_run_sends_no_command() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let (_, commands) = run_steps(&channels, vec![key(KeyCode::Left), key(KeyCode::Char('q'))]);
        assert!(commands.is_empty(), "{commands:?}");
    }

    #[test]
    fn right_and_left_on_an_empty_board_send_no_command() {
        let channels = channels();
        let steps = vec![
            key(KeyCode::Right),
            key(KeyCode::Left),
            key(KeyCode::Char('q')),
        ];
        let (_, commands) = run_steps(&channels, steps);
        assert!(commands.is_empty(), "{commands:?}");
    }

    #[test]
    fn two_runs_can_be_expanded_at_one_time() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 30)]);
        let steps = vec![
            key(KeyCode::Right),
            key(KeyCode::Down),
            key(KeyCode::Right),
            key(KeyCode::Char('q')),
        ];
        let (_, commands) = run_steps(&channels, steps);
        assert_eq!(commands, [watch(&[2]), watch(&[1, 2])]);
    }

    #[test]
    fn down_skips_the_loading_row_of_an_expanded_run() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 30)]);
        let opener = opener(false);
        let (result, _) = run_app(
            &opener,
            &channels,
            vec![
                press(KeyCode::Right),
                press(KeyCode::Down),
                press(KeyCode::Enter),
                press(KeyCode::Char('q')),
            ],
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(*opener.urls.borrow(), ["https://example.test/runs/1"]);
    }

    /// A job that has an address of its own.
    fn job_with_url(id: u64, name: &str) -> crate::job::Job {
        crate::job::Job {
            url: format!("https://example.test/runs/1/job/{id}"),
            ..job(id, name)
        }
    }

    #[test]
    fn right_the_jobs_down_and_enter_open_the_job_page() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Jobs {
                run: RunId(1),
                jobs: vec![job_with_url(10, "build")],
            }),
            Step::Idle,
            key(KeyCode::Down),
            key(KeyCode::Enter),
            key(KeyCode::Char('q')),
        ];
        let opener = opener(false);
        run_steps_with(&channels, &opener, steps);
        assert_eq!(
            *opener.urls.borrow(),
            ["https://example.test/runs/1/job/10"]
        );
    }

    #[test]
    fn left_on_a_job_row_stops_the_watch_of_its_run() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Jobs {
                run: RunId(1),
                jobs: vec![job_with_url(10, "zebra-job")],
            }),
            Step::Idle,
            key(KeyCode::Down),
            key(KeyCode::Left),
            key(KeyCode::Char('q')),
        ];
        let (frame, commands) = run_steps(&channels, steps);
        assert_eq!(commands, [watch(&[1]), watch(&[])]);
        assert!(
            frame.iter().all(|line| !line.contains("zebra-job")),
            "{frame:?}"
        );
    }

    #[test]
    fn jobs_that_arrive_after_right_show_below_the_run() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Jobs {
                run: RunId(1),
                jobs: vec![job(10, "build"), job(11, "lint")],
            }),
            Step::Idle,
            key(KeyCode::Char('q')),
        ];
        let (frame, _) = run_steps(&channels, steps);
        let run_row = row_of(&frame, "run 1");
        assert!(frame[run_row + 1].contains("├ build"), "{frame:?}");
        assert!(frame[run_row + 2].contains("└ lint"), "{frame:?}");
    }

    #[test]
    fn the_jobs_of_a_collapsed_queued_run_show_the_running_marker() {
        let channels = channels();
        send_runs(
            &channels,
            vec![Run {
                state: RunState::Queued,
                ..active_run(1, 5)
            }],
        );
        let steps = vec![
            Step::Send(Update::Jobs {
                run: RunId(1),
                jobs: vec![job(10, "build")],
            }),
            Step::Idle,
            key(KeyCode::Char('q')),
        ];
        let (frame, _) = run_steps(&channels, steps);
        let run_row = row_of(&frame, "run 1");
        assert!(frame[run_row].trim_start().starts_with('🏃'), "{frame:?}");
        assert!(
            !frame[run_row + 1].contains('└') && !frame[run_row + 1].contains('├'),
            "{frame:?}"
        );
    }

    #[test]
    fn a_failed_fetch_after_jobs_shows_the_last_jobs_and_the_error() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Jobs {
                run: RunId(1),
                jobs: vec![job(10, "build")],
            }),
            Step::Idle,
            Step::Send(Update::JobsFailed {
                run: RunId(1),
                error: "rate limited".to_owned(),
            }),
            Step::Idle,
            key(KeyCode::Char('q')),
        ];
        let (frame, _) = run_steps(&channels, steps);
        let run_row = row_of(&frame, "run 1");
        assert!(frame[run_row + 1].contains("build"), "{frame:?}");
        assert!(frame[run_row + 2].contains("└ ⚠ rate limited"), "{frame:?}");
    }

    #[test]
    fn a_run_that_leaves_its_repository_answer_stops_the_watch() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Runs {
                repo: repo(),
                runs: vec![active_run(2, 5)],
            }),
            Step::Idle,
            key(KeyCode::Char('q')),
        ];
        let (_, commands) = run_steps(&channels, steps);
        assert_eq!(commands, [watch(&[1]), watch(&[])]);
    }

    #[test]
    fn a_repository_list_without_the_repository_stops_the_watch() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Repos {
                repos: std::collections::BTreeSet::new(),
                listed_at: std::collections::BTreeMap::new(),
                owner_errors: Vec::new(),
            }),
            Step::Idle,
            key(KeyCode::Char('q')),
        ];
        let (_, commands) = run_steps(&channels, steps);
        assert_eq!(commands, [watch(&[1]), watch(&[])]);
    }

    #[test]
    fn c_and_then_y_cancel_the_highlighted_run_and_show_cancelling() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let canceller = canceller();
        let (frame, _) = run_steps_cancelling(
            &channels,
            &canceller,
            vec![
                Step::Idle,
                key(KeyCode::Char('c')),
                key(KeyCode::Char('y')),
                Step::Idle,
                key(KeyCode::Char('q')),
            ],
        );
        assert_eq!(
            *canceller.cancels.borrow(),
            vec![(repo(), RunId(1))],
            "{frame:?}"
        );
        assert!(frame[11].contains("cancelling run 1…"), "{:?}", frame[11]);
    }

    /// The title that the box shows in the top border.
    const BOX_TITLE: &str = " Cancel run? ";

    fn shows_box(frame: &[String]) -> bool {
        frame.iter().any(|line| line.contains(BOX_TITLE))
    }

    fn ctrl_c() -> Event {
        key_event(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        )
    }

    #[test]
    fn c_shows_the_box_with_the_repository_and_the_run() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let (_, frame) = run_app_with(
            Status::default(),
            &opener(false),
            &canceller(),
            &channels,
            vec![press(KeyCode::Char('c'))],
        );
        assert!(shows_box(&frame), "{frame:?}");
        assert!(
            frame.iter().any(|l| l.contains("acme/widgets")),
            "{frame:?}"
        );
        assert!(
            frame.iter().any(|l| l.contains("ci 1 on main")),
            "{frame:?}"
        );
    }

    #[test]
    fn c_on_a_job_row_shows_the_name_of_the_job_in_the_box() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let steps = vec![
            key(KeyCode::Right),
            Step::Send(Update::Jobs {
                run: RunId(1),
                jobs: vec![job(10, "build")],
            }),
            Step::Idle,
            key(KeyCode::Down),
            key(KeyCode::Char('c')),
            Step::Idle,
        ];
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let canceller = canceller();
        let opener = opener(false);
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            Status::default(),
            &opener,
            &canceller,
            &channels.worker,
            &clock,
            &tz,
        );
        let mut events = StepEvents {
            steps: steps.into(),
            updates: channels.updates.clone(),
        };
        let result = app.run(&mut terminal, &mut events);
        assert_eq!(result.unwrap_err().to_string(), "script ended");
        let frame = lines(terminal.backend().buffer());
        assert!(shows_box(&frame), "{frame:?}");
        assert!(frame.iter().any(|l| l.contains("job: build")), "{frame:?}");
    }

    #[test]
    fn c_and_y_on_a_job_row_cancel_the_run_of_the_job() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let canceller = canceller();
        run_steps_cancelling(
            &channels,
            &canceller,
            vec![
                key(KeyCode::Right),
                Step::Send(Update::Jobs {
                    run: RunId(1),
                    jobs: vec![job(10, "build")],
                }),
                Step::Idle,
                key(KeyCode::Down),
                key(KeyCode::Char('c')),
                key(KeyCode::Char('y')),
                key(KeyCode::Char('q')),
            ],
        );
        assert_eq!(*canceller.cancels.borrow(), vec![(repo(), RunId(1))]);
    }

    /// Runs the keys `c` and then `other` on a board of two runs, and returns
    /// the result, the last frame, the cancels, and the addresses opened.
    fn answer_the_box_with(
        other: Event,
    ) -> (Result<(), RunError>, Vec<String>, usize, Vec<String>) {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 30)]);
        let canceller = canceller();
        let opener = opener(false);
        let (result, frame) = run_app_with(
            Status::default(),
            &opener,
            &canceller,
            &channels,
            vec![press(KeyCode::Char('c')), other, press(KeyCode::Enter)],
        );
        let cancels = canceller.cancels.borrow().len();
        let urls = opener.urls.borrow().clone();
        (result, frame, cancels, urls)
    }

    #[test]
    fn every_key_but_y_closes_the_box_and_cancels_nothing() {
        let others = [
            press(KeyCode::Char('n')),
            press(KeyCode::Esc),
            press(KeyCode::Char('q')),
            press(KeyCode::Enter),
            press(KeyCode::Down),
        ];
        for other in others {
            let (result, frame, cancels, _) = answer_the_box_with(other.clone());
            // The loop did not quit: it read the last key, then the script ended.
            assert_eq!(result.unwrap_err().to_string(), "script ended", "{other:?}");
            assert_eq!(cancels, 0, "{other:?}");
            assert!(!shows_box(&frame), "{other:?} {frame:?}");
        }
    }

    #[test]
    fn enter_with_the_box_open_opens_nothing_and_down_does_not_move_the_highlight() {
        // After `c` and Enter, the Enter that follows opens the first row.
        let (_, _, _, urls) = answer_the_box_with(press(KeyCode::Enter));
        assert_eq!(urls, ["https://example.test/runs/2"]);
        // After `c` and Down, the Enter that follows opens the first row too.
        let (_, _, _, urls) = answer_the_box_with(press(KeyCode::Down));
        assert_eq!(urls, ["https://example.test/runs/2"]);
    }

    #[test]
    fn a_run_that_moves_the_highlight_does_not_change_the_run_that_y_cancels() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5), active_run(2, 30)]);
        let canceller = canceller();
        run_steps_cancelling(
            &channels,
            &canceller,
            vec![
                Step::Idle,
                key(KeyCode::Char('c')),
                Step::Send(Update::Runs {
                    repo: repo(),
                    runs: vec![active_run(1, 5), active_run(3, 60)],
                }),
                Step::Idle,
                key(KeyCode::Char('y')),
                key(KeyCode::Char('q')),
            ],
        );
        assert_eq!(*canceller.cancels.borrow(), vec![(repo(), RunId(2))]);
    }

    #[test]
    fn c_on_an_empty_board_does_nothing() {
        // A key press clears the message, thus read the frame after `c` alone.
        let (result, frame) = run_app_with(
            Status::default(),
            &opener(false),
            &canceller(),
            &channels(),
            vec![press(KeyCode::Char('c'))],
        );
        assert_eq!(result.unwrap_err().to_string(), "script ended");
        assert!(!shows_box(&frame), "{frame:?}");
        assert!(!frame[11].contains("cancelling"), "{:?}", frame[11]);
        assert!(!frame[11].contains("nothing to cancel"), "{:?}", frame[11]);
    }

    #[test]
    fn y_after_c_on_an_empty_board_cancels_nothing() {
        let canceller = canceller();
        let (result, _) = run_app_with(
            Status::default(),
            &opener(false),
            &canceller,
            &channels(),
            vec![press(KeyCode::Char('c')), press(KeyCode::Char('y'))],
        );
        assert_eq!(result.unwrap_err().to_string(), "script ended");
        assert!(canceller.cancels.borrow().is_empty());
    }

    #[test]
    fn control_c_with_the_box_open_quits_and_cancels_nothing() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let canceller = canceller();
        let (result, _) = run_app_with(
            Status::default(),
            &opener(false),
            &canceller,
            &channels,
            vec![press(KeyCode::Char('c')), ctrl_c()],
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(canceller.cancels.borrow().is_empty());
    }

    /// A finished run, updated inside the window.
    fn passed_run(id: u64) -> Run {
        Run {
            state: RunState::Passed,
            ..active_run(id, 5)
        }
    }

    #[test]
    fn c_on_a_finished_run_says_there_is_nothing_to_cancel_and_shows_no_box() {
        let channels = channels();
        send_runs(&channels, vec![passed_run(4)]);
        let (result, frame) = run_app_with(
            Status::default(),
            &opener(false),
            &canceller(),
            &channels,
            vec![press(KeyCode::Char('c'))],
        );
        assert_eq!(result.unwrap_err().to_string(), "script ended");
        assert!(!shows_box(&frame), "{frame:?}");
        assert!(
            frame[11].contains("run 4 finished: there is nothing to cancel"),
            "{:?}",
            frame[11]
        );
    }

    #[test]
    fn y_after_c_on_a_finished_run_cancels_nothing() {
        let channels = channels();
        send_runs(&channels, vec![passed_run(4)]);
        let canceller = canceller();
        let (result, _) = run_app_with(
            Status::default(),
            &opener(false),
            &canceller,
            &channels,
            vec![press(KeyCode::Char('c')), press(KeyCode::Char('y'))],
        );
        assert_eq!(result.unwrap_err().to_string(), "script ended");
        assert!(canceller.cancels.borrow().is_empty());
    }

    /// An outcome for run 7 that the canceller of a test gives to the loop.
    fn outcome(result: Result<(), SourceError>) -> CancelOutcome {
        CancelOutcome {
            repo: repo(),
            run: RunId(7),
            result,
        }
    }

    #[test]
    fn an_accepted_cancel_says_so_and_asks_for_a_refresh_now() {
        let channels = channels();
        let canceller = canceller();
        canceller.outcomes.borrow_mut().push_back(outcome(Ok(())));
        let (frame, commands) = run_steps_cancelling(
            &channels,
            &canceller,
            vec![Step::Idle, key(KeyCode::Char('q'))],
        );
        assert!(commands.contains(&Command::RefreshNow), "{commands:?}");
        assert!(frame[11].contains("refreshing…"), "{:?}", frame[11]);
    }

    #[test]
    fn an_accepted_cancel_posts_that_github_is_cancelling_the_run() {
        let channels = channels();
        let canceller = canceller();
        canceller.outcomes.borrow_mut().push_back(outcome(Ok(())));
        let (frame, _) = run_steps_cancelling(
            &channels,
            &canceller,
            vec![Step::Idle, key(KeyCode::Char('q'))],
        );
        assert!(
            frame[11].contains("GitHub is cancelling run 7"),
            "{:?}",
            frame[11]
        );
    }

    #[test]
    fn a_refused_cancel_posts_the_run_and_the_error() {
        let channels = channels();
        let canceller = canceller();
        canceller
            .outcomes
            .borrow_mut()
            .push_back(outcome(Err(SourceError::Http {
                status: 409,
                message: "Cannot cancel a workflow run that is completed.".to_owned(),
            })));
        let (frame, commands) = run_steps_cancelling(
            &channels,
            &canceller,
            vec![Step::Idle, key(KeyCode::Char('q'))],
        );
        assert!(
            frame[11].contains(
                "could not cancel run 7: GitHub answered 409: Cannot cancel a workflow run that is completed."
            ),
            "{:?}",
            frame[11]
        );
        assert!(!commands.contains(&Command::RefreshNow), "{commands:?}");
    }

    /// The text that the shell wrote on the screen before `ha` started.
    const SHELL_TEXT: &str = "old-shell-text";

    /// A screen of `width` by `height` cells that the shell filled before
    /// `ha` started. Each cell holds a glyph, and no cell is blank.
    fn screen_with_shell_text(width: u16, height: u16) -> TestBackend {
        TestBackend::with_lines((0..height).map(|row| {
            format!("{SHELL_TEXT}-{row}|")
                .chars()
                .cycle()
                .take(usize::from(width))
                .collect::<String>()
        }))
    }

    #[test]
    fn the_first_frame_hides_the_text_that_was_on_the_screen_before() {
        let quit = || vec![press(KeyCode::Char('q'))];
        let on_blank = channels();
        send_runs(&on_blank, vec![active_run(1, 5)]);
        let (result, expected) = run_app_on(
            TestBackend::new(120, 12),
            Status::default(),
            &opener(false),
            &canceller(),
            &on_blank,
            quit(),
        );
        assert!(result.is_ok(), "{result:?}");
        let on_shell_text = channels();
        send_runs(&on_shell_text, vec![active_run(1, 5)]);
        let (result, frame) = run_app_on(
            screen_with_shell_text(120, 12),
            Status::default(),
            &opener(false),
            &canceller(),
            &on_shell_text,
            quit(),
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(!frame.iter().any(|l| l.contains(SHELL_TEXT)), "{frame:#?}");
        assert_eq!(frame, expected);
    }

    /// A terminal that does not answer the query for the cursor position.
    /// A recording under `script` does not answer it, and a slow link
    /// answers too late. Each other call goes to the `TestBackend` inside.
    struct NoCursorReport(TestBackend);

    impl Backend for NoCursorReport {
        type Error = std::io::Error;

        fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            let Ok(()) = self.0.draw(content);
            Ok(())
        }

        fn hide_cursor(&mut self) -> std::io::Result<()> {
            let Ok(()) = self.0.hide_cursor();
            Ok(())
        }

        fn show_cursor(&mut self) -> std::io::Result<()> {
            let Ok(()) = self.0.show_cursor();
            Ok(())
        }

        fn get_cursor_position(&mut self) -> std::io::Result<Position> {
            Err(std::io::Error::other(
                "the terminal does not report the cursor position",
            ))
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
            let Ok(()) = self.0.set_cursor_position(position);
            Ok(())
        }

        fn clear(&mut self) -> std::io::Result<()> {
            let Ok(()) = self.0.clear();
            Ok(())
        }

        fn clear_region(&mut self, clear_type: ClearType) -> std::io::Result<()> {
            let Ok(()) = self.0.clear_region(clear_type);
            Ok(())
        }

        fn size(&self) -> std::io::Result<Size> {
            let Ok(size) = self.0.size();
            Ok(size)
        }

        fn window_size(&mut self) -> std::io::Result<WindowSize> {
            let Ok(size) = self.0.window_size();
            Ok(size)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            let Ok(()) = self.0.flush();
            Ok(())
        }
    }

    #[test]
    fn the_view_starts_on_a_terminal_that_does_not_report_the_cursor() {
        let channels = channels();
        send_runs(&channels, vec![active_run(1, 5)]);
        let backend = NoCursorReport(screen_with_shell_text(120, 12));
        let mut terminal = Terminal::new(backend).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let clock = now;
        let opener = opener(false);
        let canceller = canceller();
        let mut app = App::new(
            Board::new(Duration::from_hours(8)),
            Status::default(),
            &opener,
            &canceller,
            &channels.worker,
            &clock,
            &tz,
        );
        let events = vec![press(KeyCode::Char('q')).into()];
        let result = app.run(&mut terminal, &mut script(events));
        assert!(result.is_ok(), "{result:?}");
        let frame = lines(terminal.backend().0.buffer());
        assert!(!frame.iter().any(|l| l.contains(SHELL_TEXT)), "{frame:#?}");
    }
}
