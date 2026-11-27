//! The full-screen view of `faulte tui`.
//!
//! - [`app`] — the state of the view. One pure value takes a key or an update
//!   and changes. It holds no terminal and no clock.
//! - [`view`] — the paint of one screen. One function reads the state and
//!   draws it. It changes nothing but the room that the state knows of.

pub mod app;
pub mod view;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use chrono::TimeZone;
use crossterm::event::Event;
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::history::{History, Update};
use crate::tui::app::App;
use crate::tui::view::{draw, ViewContext};

/// A source of terminal events.
pub trait Events {
    /// Waits up to `timeout` for one event.
    ///
    /// Returns `Ok(None)` when no event arrives in time.
    ///
    /// # Errors
    /// Returns the error of the terminal when it cannot be read.
    fn next(&mut self, timeout: Duration) -> io::Result<Option<Event>>;
}

/// The events of the real terminal.
pub struct TerminalEvents;

impl Events for TerminalEvents {
    fn next(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        if crossterm::event::poll(timeout)? {
            crossterm::event::read().map(Some)
        } else {
            Ok(None)
        }
    }
}

/// Why the event loop ended with an error.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The terminal could not draw or be read.
    #[error(transparent)]
    Terminal(#[from] io::Error),
}

/// The longest time that the loop waits for an event.
const TICK: Duration = Duration::from_millis(100);

/// The time after which the loop draws again with no change, because the ages
/// on the screen move.
const REDRAW_AFTER: Duration = Duration::from_secs(1);

/// Tells whether the ages on the screen moved since the last draw. The first
/// draw has no earlier one. A clock that moved back counts as no time.
fn ages_moved(last_draw: Option<SystemTime>, now: SystemTime) -> bool {
    last_draw.is_none_or(|last| now.duration_since(last).unwrap_or_default() >= REDRAW_AFTER)
}

/// Runs the view until the person quits or `signalled` is set.
///
/// The loop never reads `top`. It takes each update that waits in `updates`
/// with `try_recv`, so a slow `top` cannot hold back a key. A closed channel
/// does not end the view, because the history stays readable after the
/// sampler stops.
///
/// # Errors
/// Returns [`RunError::Terminal`] when the terminal cannot draw or be read.
pub fn run<B, E, Tz>(
    terminal: &mut Terminal<B>,
    events: &mut E,
    updates: &Receiver<Update>,
    signalled: &AtomicBool,
    now: &dyn Fn() -> SystemTime,
    tz: &Tz,
) -> Result<(), RunError>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    E: Events,
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    let mut app = App::new(History::new());
    let mut last_draw: Option<SystemTime> = None;
    let mut changed = false;
    while !signalled.load(Ordering::SeqCst) {
        while let Ok(update) = updates.try_recv() {
            app.on_update(update);
            changed = true;
        }
        let clock = now();
        if changed || ages_moved(last_draw, clock) {
            let context = ViewContext { now: clock, tz };
            terminal
                .draw(|frame| draw(frame, &mut app, &context))
                .map_err(io::Error::other)?;
            last_draw = Some(clock);
            changed = false;
        }
        match events.next(TICK)? {
            Some(Event::Key(key)) => {
                app.on_key(key);
                changed = true;
            }
            Some(Event::Resize(..)) => changed = true,
            Some(_) | None => {}
        }
        if app.should_quit() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::Arc;

    use chrono::FixedOffset;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::history::SampledFrame;
    use crate::machine::Observed;
    use crate::pid::{Pid, Uid};
    use crate::ranking::{ClaudeTotal, ClaudeView, RankedRow, Ranking, Skipped};
    use crate::render::Accounts;
    use crate::vm::{SwapUsage, VmDelta};

    /// The time of the test frame: 2023-11-14 22:13:20 UTC.
    const BASE_SECS: u64 = 1_700_000_000;

    /// The width of the test screen.
    const WIDTH: u16 = 120;

    /// The height of the test screen.
    const HEIGHT: u16 = 20;

    /// The command of the one row of the test frame.
    const COMMAND: &str = "/bin/test-command-1";

    /// Gives the time `millis` milliseconds after [`BASE_SECS`].
    fn time(millis: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_SECS) + Duration::from_millis(millis)
    }

    /// Makes the update that holds one sample with one row.
    fn frame(command: &str) -> Update {
        let window = Duration::from_secs(2);
        let rows = vec![RankedRow {
            pid: Pid::new(1),
            uid: Uid::new(501),
            faults: 100,
            rss_kib: Some(1_000),
            started_at_epoch_secs: Some(BASE_SECS - 3_600),
            command: command.to_string(),
            claude: ClaudeView::NotClaude,
        }];
        Update::Frame(Box::new(SampledFrame {
            observed: Observed {
                ranking: Ranking {
                    rows,
                    window,
                    total_faults: 100,
                    claude: ClaudeTotal {
                        processes: 0,
                        other_account: 0,
                        faults: 0,
                    },
                    skipped: Skipped::default(),
                },
                table: Vec::new(),
                swap: VmDelta::default(),
                usage: SwapUsage::default(),
                compressor_bytes: 0,
                swap_window: window,
                accounts: Accounts::new(),
                now: time(0),
                interval: "2s".parse().expect("2s is a span"),
            },
            at: time(0),
            own: BTreeSet::new(),
        }))
    }

    /// Makes a key press of `code`.
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Gives the stated events and then an error, so that a loop that does
    /// not stop fails at once and never hangs. It counts its calls, and it
    /// can set a flag at a stated call.
    struct ScriptedEvents {
        /// The events still to give.
        script: VecDeque<Option<Event>>,
        /// How many times the loop asked.
        calls: usize,
        /// The call at which the flag is set, and the flag.
        raise: Option<(usize, Arc<AtomicBool>)>,
        /// The call at which an update is sent, and the update.
        deliver: Option<(usize, Sender<Update>, Update)>,
    }

    impl ScriptedEvents {
        /// Makes a script of `events`.
        fn new(events: Vec<Option<Event>>) -> Self {
            Self {
                script: events.into(),
                calls: 0,
                raise: None,
                deliver: None,
            }
        }
    }

    impl Events for ScriptedEvents {
        fn next(&mut self, _timeout: Duration) -> io::Result<Option<Event>> {
            self.calls += 1;
            if let Some((call, flag)) = &self.raise {
                if *call == self.calls {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            if self
                .deliver
                .as_ref()
                .is_some_and(|(call, ..)| *call == self.calls)
            {
                if let Some((_, sender, update)) = self.deliver.take() {
                    sender.send(update).expect("the channel is open");
                }
            }
            self.script
                .pop_front()
                .ok_or_else(|| io::Error::other("script ended"))
        }
    }

    /// The parts of one test run.
    struct Harness {
        /// The terminal of the run.
        terminal: Terminal<TestBackend>,
        /// The sending end of the channel of updates.
        sender: Sender<Update>,
        /// The receiving end of the channel of updates.
        updates: Receiver<Update>,
        /// The flag that a signal sets.
        signalled: Arc<AtomicBool>,
    }

    impl Harness {
        /// Makes a harness with a screen of the test size.
        fn new() -> Self {
            let (sender, updates) = channel();
            Self {
                terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test backend"),
                sender,
                updates,
                signalled: Arc::new(AtomicBool::new(false)),
            }
        }

        /// Runs the loop over `events`, with a clock that reads `clock`.
        fn run(
            &mut self,
            events: &mut ScriptedEvents,
            clock: &dyn Fn() -> SystemTime,
        ) -> Result<(), RunError> {
            let utc = FixedOffset::east_opt(0).expect("zero is an offset");
            run(
                &mut self.terminal,
                events,
                &self.updates,
                &self.signalled,
                clock,
                &utc,
            )
        }

        /// Gives the text of the screen, one string with line breaks.
        fn screen(&self) -> String {
            text_of(self.terminal.backend().buffer())
        }
    }

    /// Gives the glyphs of `buffer`, line by line.
    fn text_of(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_loop_draws_the_frame_that_arrives_and_quits_on_q() {
        let mut harness = Harness::new();
        harness
            .sender
            .send(frame(COMMAND))
            .expect("the channel is open");
        let mut events = ScriptedEvents::new(vec![
            None,
            Some(key(KeyCode::Char('q'))),
            Some(key(KeyCode::Char('x'))),
        ]);

        let result = harness.run(&mut events, &|| time(1_000));

        assert!(
            result.is_ok(),
            "q ends the run without an error: {result:?}"
        );
        assert!(
            harness.screen().contains(COMMAND),
            "the screen shows the row of the frame:\n{}",
            harness.screen()
        );
        assert_eq!(events.calls, 2, "the loop reads no event after q");
    }

    #[test]
    fn the_loop_leaves_when_a_signal_set_the_flag() {
        let mut harness = Harness::new();
        let mut events = ScriptedEvents::new(vec![None, None, Some(key(KeyCode::Char('x')))]);
        events.raise = Some((2, Arc::clone(&harness.signalled)));

        let result = harness.run(&mut events, &|| time(1_000));

        assert!(
            result.is_ok(),
            "a signal ends the run without an error: {result:?}"
        );
        assert_eq!(
            events.calls, 2,
            "the loop looks at the flag on the pass after the signal"
        );
    }

    #[test]
    fn a_flag_that_is_set_before_the_start_ends_the_run_at_once() {
        let mut harness = Harness::new();
        harness.signalled.store(true, Ordering::SeqCst);
        let mut events = ScriptedEvents::new(vec![]);

        let result = harness.run(&mut events, &|| time(0));

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(events.calls, 0, "the loop reads nothing");
    }

    /// Gives a clock that reads `millis` in turn, and its last value after
    /// that.
    fn clock_of(millis: &[u64]) -> impl Fn() -> SystemTime + '_ {
        let next = std::cell::Cell::new(0_usize);
        move || {
            let at = next.get();
            next.set(at + 1);
            time(millis[at.min(millis.len() - 1)])
        }
    }

    #[test]
    fn a_quiet_pass_under_one_second_does_not_draw_again() {
        let mut harness = Harness::new();
        harness
            .sender
            .send(frame(COMMAND))
            .expect("the channel is open");
        let mut events = ScriptedEvents::new(vec![None, Some(key(KeyCode::Char('q')))]);

        let result = harness.run(&mut events, &clock_of(&[1_900, 2_100]));

        assert!(result.is_ok(), "{result:?}");
        let screen = harness.screen();
        assert!(
            screen.contains("(1s ago)"),
            "the age stays at the first draw:\n{screen}"
        );
    }

    #[test]
    fn a_pass_after_one_second_draws_again_with_no_change() {
        let mut harness = Harness::new();
        harness
            .sender
            .send(frame(COMMAND))
            .expect("the channel is open");
        let mut events = ScriptedEvents::new(vec![None, Some(key(KeyCode::Char('q')))]);

        let result = harness.run(&mut events, &clock_of(&[1_000, 2_100]));

        assert!(result.is_ok(), "{result:?}");
        let screen = harness.screen();
        assert!(
            screen.contains("(2s ago)"),
            "the age moved with the clock:\n{screen}"
        );
    }

    #[test]
    fn a_resize_makes_the_next_pass_draw() {
        let mut harness = Harness::new();
        harness
            .sender
            .send(frame(COMMAND))
            .expect("the channel is open");
        let mut events = ScriptedEvents::new(vec![
            Some(Event::Resize(WIDTH, HEIGHT)),
            Some(key(KeyCode::Char('q'))),
        ]);

        let result = harness.run(&mut events, &clock_of(&[1_900, 2_100]));

        assert!(result.is_ok(), "{result:?}");
        assert!(
            harness.screen().contains("(2s ago)"),
            "{}",
            harness.screen()
        );
    }

    #[test]
    fn a_key_makes_the_next_pass_draw() {
        let mut harness = Harness::new();
        harness
            .sender
            .send(frame(COMMAND))
            .expect("the channel is open");
        let mut events = ScriptedEvents::new(vec![
            Some(key(KeyCode::Char('w'))),
            Some(key(KeyCode::Char('q'))),
        ]);

        let result = harness.run(&mut events, &clock_of(&[1_900, 2_100]));

        assert!(result.is_ok(), "{result:?}");
        assert!(
            harness.screen().contains("(2s ago)"),
            "{}",
            harness.screen()
        );
    }

    #[test]
    fn an_update_that_arrives_later_makes_the_next_pass_draw() {
        let mut harness = Harness::new();
        let mut events = ScriptedEvents::new(vec![None, Some(key(KeyCode::Char('q')))]);
        events.deliver = Some((1, harness.sender.clone(), frame(COMMAND)));

        let result = harness.run(&mut events, &clock_of(&[1_000, 1_100]));

        assert!(result.is_ok(), "{result:?}");
        assert!(
            harness.screen().contains(COMMAND),
            "the late frame is on the screen:\n{}",
            harness.screen()
        );
    }

    #[test]
    fn a_closed_channel_does_not_end_the_view() {
        let mut harness = Harness::new();
        let sender = std::mem::replace(&mut harness.sender, channel().0);
        sender.send(frame(COMMAND)).expect("the channel is open");
        drop(sender);
        let mut events = ScriptedEvents::new(vec![None, None, Some(key(KeyCode::Char('q')))]);

        let result = harness.run(&mut events, &clock_of(&[1_000]));

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(events.calls, 3, "the loop kept reading keys until q");
        assert!(
            harness.screen().contains(COMMAND),
            "the history stays on the screen:\n{}",
            harness.screen()
        );
    }
}
