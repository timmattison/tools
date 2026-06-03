//! Watch mode: the pure decision helpers that drive the live refresh loop.
//!
//! Phase 4 adds a self-refreshing watch view to seescc. Every *decision* the
//! watch loop makes that does not require a live terminal — which rendering
//! mode to run in, whether a key press means "quit", and whether colors should
//! survive being piped through a watch-like wrapper — lives here as a pure,
//! terminal-free function so it can be unit-tested without a pty. The terminal
//! lifecycle and event loop that consume these helpers are wired in later in
//! this phase.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

/// An external event the watch loop reacts to between polls.
///
/// Unlike `gsw` (which is filesystem-event driven), seescc's loop is
/// **timer-driven**: sccache exposes nothing to subscribe to, so a fresh poll is
/// triggered by a `recv_timeout` *timeout*, not by an event. The only events
/// carried on the channel are the two things a poll can't produce on its own — a
/// terminal resize and a user quit — both originating from the keyboard /
/// resize reader thread wired in by a later slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// The terminal was resized. The previously-painted bytes are now laid out
    /// for the old dimensions and are visually stale, so the loop must
    /// re-render at the new size and repaint **unconditionally**, bypassing the
    /// byte-compare suppression that would otherwise swallow an identical frame.
    Resize,
    /// The user asked to quit (`q`, `Esc`, or Ctrl-C). The loop returns cleanly.
    Quit,
}

/// The poll-outcome state machine that backs the watch loop's rendering.
///
/// Each poll of sccache feeds its outcome into [`WatchState::apply_poll`], which
/// is the *only* way the two displayed facts evolve:
///
/// - `last_good` — the most recent successfully-parsed [`crate::stats::Stats`].
///   The render closure draws the numeric rows from this, so it must survive a
///   failed poll: when the server is briefly unreachable we keep showing the
///   last numbers we trust rather than blanking the table.
/// - `error` — the banner message for the *current* poll, or `None`. A poll
///   failure is **non-fatal** (design §6: "error banner + keep last good
///   frame"), so it sets the banner without disturbing `last_good`; the next
///   successful poll clears the banner (recovery), so a transient blip leaves no
///   lingering error once the server is back.
#[derive(Debug, Default)]
pub(crate) struct WatchState {
    /// The most recent stats from a *successful* poll, retained across failures
    /// so the table keeps showing trustworthy numbers during an outage.
    last_good: Option<crate::stats::Stats>,
    /// The error banner for the current poll, or `None` when the last poll
    /// succeeded. Cleared on recovery.
    error: Option<String>,
}

impl WatchState {
    /// The last successfully-polled stats, if any poll has ever succeeded.
    ///
    /// The render closure draws the metric rows from this; `None` only before
    /// the first successful poll (e.g. a first-poll failure), where the caller
    /// renders a banner with no rows.
    pub(crate) fn last_good(&self) -> Option<&crate::stats::Stats> {
        self.last_good.as_ref()
    }

    /// The current error-banner message, or `None` when the last poll
    /// succeeded.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Fold a poll outcome into the state.
    ///
    /// - `Ok(stats)` is a successful poll: it becomes the new `last_good` and
    ///   clears any error banner (recovery — a prior failure must not linger
    ///   once the server is back).
    /// - `Err(e)` is a *non-fatal* failed poll: it sets the banner to `e`'s
    ///   display and leaves `last_good` untouched, so the table keeps showing
    ///   the last good numbers (design §6).
    pub(crate) fn apply_poll(&mut self, outcome: anyhow::Result<crate::stats::Stats>) {
        match outcome {
            Ok(stats) => {
                // Success: adopt the fresh numbers and clear any banner — a
                // recovered poll must not keep showing a stale error.
                self.last_good = Some(stats);
                self.error = None;
            }
            Err(e) => {
                // Failure is non-fatal: raise the banner but leave `last_good`
                // alone so the table keeps the last trustworthy numbers.
                self.error = Some(e.to_string());
            }
        }
    }
}

/// The terminal-free core of the watch loop, with every side effect injected.
///
/// seescc's loop is **timer-driven** (sccache has no events to subscribe to), so
/// the structure differs from `gsw`'s fs-event loop in two ways: a poll is
/// triggered by a `recv_timeout` *timeout* rather than an event, and the channel
/// only ever carries [`Event::Resize`] / [`Event::Quit`]. Three closures keep
/// the loop unit-testable with no TTY, no real sccache, and no real time:
///
/// - `poll` fetches fresh stats (production: shell out to sccache); its outcome
///   is folded into `state` via [`WatchState::apply_poll`], so a failed poll is
///   absorbed there as a non-fatal banner and **never** propagates out of the
///   loop — a transient outage must not tear the watch view down.
/// - `render` turns the current `state` into a frame string (production: query
///   terminal size + wall clock, draw the table with banner/footer).
/// - `paint` writes a frame to the screen.
///
/// Behavior:
///
/// - **First frame is immediate.** On entry the loop polls, applies, renders,
///   and paints once *before* waiting, so the user sees data right away instead
///   of staring at a blank pane for a full `poll_interval`.
/// - **Timeout ⇒ poll.** Each `recv_timeout(poll_interval)` timeout is a tick:
///   poll, apply, render, and repaint **only if the frame differs** from what's
///   displayed. Byte-identical suppression is what stops an idle server from
///   flickering — the poll still happens, but no paint.
/// - **Resize ⇒ forced repaint.** A resize re-renders from the *current* state
///   (no poll — the numbers haven't changed) and paints **unconditionally**,
///   even when the bytes match `displayed`, because the on-screen layout is
///   stale after the resize and suppression would wrongly skip the redraw.
/// - **Quit / disconnect ⇒ clean return.** [`Event::Quit`] returns `Ok(())`
///   with no further work; a disconnected channel (every sender dropped) is
///   likewise a clean shutdown.
fn event_loop<P, R, Pa>(
    rx: &Receiver<Event>,
    poll_interval: Duration,
    state: &mut WatchState,
    displayed: &mut String,
    mut poll: P,
    mut render: R,
    mut paint: Pa,
) -> Result<()>
where
    P: FnMut() -> Result<crate::stats::Stats>,
    R: FnMut(&WatchState) -> String,
    Pa: FnMut(&str) -> Result<()>,
{
    // The immediate first frame: poll, fold the outcome into state (a failure
    // becomes a non-fatal banner here, never an early return), render, and paint
    // unconditionally — there is nothing on screen yet to compare against. This
    // is why the user sees data without waiting a full poll_interval.
    poll_once(state, &mut poll, &mut render, &mut paint, displayed, true)?;

    loop {
        match rx.recv_timeout(poll_interval) {
            // A timeout is a tick: re-poll and repaint only if the frame
            // changed (suppression keeps an idle server from flickering).
            Err(RecvTimeoutError::Timeout) => {
                poll_once(state, &mut poll, &mut render, &mut paint, displayed, false)?;
            }
            // A resize re-renders from the *current* state at the new
            // dimensions (no poll — only the geometry changed) and paints
            // unconditionally: the previously-painted bytes are laid out for the
            // old size and are visually stale, so suppression must be bypassed
            // even when the new frame is byte-identical.
            Ok(Event::Resize) => {
                let frame = render(state);
                paint(&frame)?;
                *displayed = frame;
            }
            // Quit, or every sender gone: clean shutdown.
            Ok(Event::Quit) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Run one poll → apply → render → paint cycle, repainting either always
/// (`force`) or only when the new frame differs from `displayed`.
///
/// Factoring this out keeps the immediate first frame (`force = true`, nothing
/// on screen to compare) and each timeout-driven tick (`force = false`,
/// byte-compare suppression) on one code path, so they can never drift apart. A
/// failed poll is absorbed by [`WatchState::apply_poll`] into a non-fatal
/// banner, so this returns `Err` only when `paint` itself fails — never for a
/// poll error.
fn poll_once<P, R, Pa>(
    state: &mut WatchState,
    poll: &mut P,
    render: &mut R,
    paint: &mut Pa,
    displayed: &mut String,
    force: bool,
) -> Result<()>
where
    P: FnMut() -> Result<crate::stats::Stats>,
    R: FnMut(&WatchState) -> String,
    Pa: FnMut(&str) -> Result<()>,
{
    state.apply_poll(poll());
    let frame = render(state);
    if force || frame != *displayed {
        paint(&frame)?;
        *displayed = frame;
    }
    Ok(())
}

/// Which rendering mode `seescc` runs in.
///
/// Watch mode is the default, but it only makes sense when there is a live
/// terminal to take over; otherwise seescc renders a single frame and exits.
/// The choice is made by [`decide_mode`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Render a single frame and exit. Used for `--one-shot` and any non-TTY
    /// (piped or captured) stdout.
    OneShot,
    /// Long-lived watch loop that owns the whole terminal pane.
    Watch,
}

/// Which rendering mode to run in once sccache stats are available.
///
/// Watch mode is the default, but it only makes sense when there is a live
/// terminal to take over: `--one-shot` and any non-TTY stdout (a pipe, a file,
/// a watch-like wrapper) fall back to a single render.
pub(crate) fn decide_mode(force_one_shot: bool, stdout_is_tty: bool) -> Mode {
    if force_one_shot || !stdout_is_tty {
        Mode::OneShot
    } else {
        Mode::Watch
    }
}

/// Whether a key event should end the watch loop.
///
/// The watch loop quits on a **press** (never a key *release* — kitty and
/// Windows report releases too) of any of the conventional quit keys: `q`,
/// `Esc`, or Ctrl-C. Every other key — a plain `c` without Control, an
/// unrelated character, `Enter` — leaves the loop running.
pub(crate) fn is_quit_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> bool {
    // Ignore key-release events (kitty/Windows report them); only a press
    // should ever quit.
    if kind == KeyEventKind::Release {
        return false;
    }
    matches!(code, KeyCode::Char('q') | KeyCode::Esc)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')))
}

/// Whether colors should be force-enabled despite stdout not being a TTY.
///
/// True only when output is captured by a watch-like wrapper (stdout is not a
/// TTY *and* `COLUMNS` is set in env), and the user has not asked to suppress
/// colors via `NO_COLOR`. The wrapper renders the captured bytes inside its own
/// TTY-backed UI, so colors should pass through rather than be stripped as they
/// normally would be for a pipe.
pub(crate) fn should_force_colors(
    stdout_is_tty: bool,
    columns_env_present: bool,
    no_color_env: bool,
) -> bool {
    !stdout_is_tty && columns_env_present && !no_color_env
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;

    use super::*;

    /// The captured real-sccache fixture, parsed into [`crate::stats::Stats`] —
    /// the same data the rest of the crate's tests use, so [`WatchState`] is
    /// exercised against a realistic payload rather than an ad-hoc literal.
    const FIXTURE: &str = include_str!("../tests/fixtures/sccache-0.15.0.json");

    /// Parse the captured fixture into [`crate::stats::Stats`] for the state
    /// tests. The exact field values don't matter here — these tests assert
    /// *which* stats are retained, not their contents — so any successfully
    /// parsed payload works.
    fn fixture_stats() -> crate::stats::Stats {
        crate::stats::parse(FIXTURE).expect("fixture should parse")
    }

    #[test]
    fn apply_poll_ok_stores_stats_and_clears_a_prior_error() {
        // A successful poll must (1) record its stats as the new last-good frame
        // and (2) clear any banner left over from an earlier failure — recovery
        // wipes the error the moment the server is reachable again.
        let mut state = WatchState::default();
        // Seed a prior failure so we can prove recovery clears it.
        state.apply_poll(Err(anyhow::anyhow!("connection refused")));
        assert!(
            state.error().is_some(),
            "a failed poll must leave an error banner to be cleared",
        );

        state.apply_poll(Ok(fixture_stats()));
        assert!(
            state.last_good().is_some(),
            "a successful poll must store its stats as last_good",
        );
        assert_eq!(
            state.error(),
            None,
            "a successful poll must clear a prior error (recovery)",
        );
        // The retained stats are the ones we just supplied.
        assert_eq!(
            state.last_good().expect("last_good present").version,
            "0.15.0",
            "last_good must be the stats from the successful poll",
        );
    }

    #[test]
    fn apply_poll_err_sets_error_and_retains_previous_good_stats() {
        // After at least one good poll, a failure is non-fatal: it raises the
        // banner but must NOT discard the last good numbers — the table keeps
        // showing them through the outage (design §6).
        let mut state = WatchState::default();
        state.apply_poll(Ok(fixture_stats()));

        state.apply_poll(Err(anyhow::anyhow!("sccache --show-stats failed")));
        assert_eq!(
            state.error(),
            Some("sccache --show-stats failed"),
            "a failed poll must surface the error's display as the banner",
        );
        assert!(
            state.last_good().is_some(),
            "a failed poll must retain the previous good stats, not blank them",
        );
        assert_eq!(
            state.last_good().expect("last_good retained").version,
            "0.15.0",
            "the retained stats must be the last successful poll's, unchanged",
        );
    }

    #[test]
    fn apply_poll_err_with_no_prior_good_leaves_last_good_none() {
        // A first-poll failure (server down at startup) raises the banner but
        // has no good frame to keep — last_good stays None, and the caller
        // renders a banner with no rows.
        let mut state = WatchState::default();
        state.apply_poll(Err(anyhow::anyhow!("connection refused")));
        assert_eq!(
            state.error(),
            Some("connection refused"),
            "a first-poll failure must still raise the banner",
        );
        assert!(
            state.last_good().is_none(),
            "a first-poll failure has no good frame to keep: last_good stays None",
        );
    }

    /// A tiny poll interval so the timeout-driven tests fire a tick almost
    /// immediately instead of waiting a realistic second. The tick tests
    /// terminate by having a closure send [`Event::Quit`], so this value only
    /// affects how fast the first tick arrives, not correctness.
    const TEST_POLL_INTERVAL: Duration = Duration::from_millis(5);

    #[test]
    fn event_loop_paints_first_frame_immediately_before_waiting() {
        // The user must see data right away, not after a full poll_interval. On
        // entry the loop polls once, renders, and paints — all before its first
        // recv_timeout. A pre-queued Quit ends the loop right after that first
        // frame so the test stays fast and deterministic.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut state = WatchState::default();
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let mut renders = 0_usize;
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| {
                renders += 1;
                "first".to_string()
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        assert_eq!(polls, 1, "the immediate first frame must poll exactly once");
        assert_eq!(renders, 1, "the immediate first frame must render once");
        assert_eq!(paints, 1, "the immediate first frame must paint once");
        assert_eq!(displayed, "first", "displayed must hold the first frame");
    }

    #[test]
    fn event_loop_timeout_tick_repaints_a_changed_frame() {
        // A poll_interval timeout is a tick: poll, render, and — because the new
        // frame differs from what's displayed — repaint. The render closure
        // sends Quit so the loop ends right after this one tick-driven frame.
        // `polls` is a shared Cell so both the poll and render closures can read
        // it without one's mutable borrow conflicting with the other's read.
        let (tx, rx) = mpsc::channel();
        let mut state = WatchState::default();
        let mut displayed = String::new();
        let polls = Cell::new(0_usize);
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls.set(polls.get() + 1);
                Ok(fixture_stats())
            },
            |_state| {
                // Distinct frame per call so the immediate frame and the tick
                // frame differ, and end the loop after the tick frame. `tx`
                // outlives the call, so the channel stays open for the tick.
                let n = polls.get();
                let frame = format!("frame {n}");
                if n >= 2 {
                    let _ = tx.send(Event::Quit);
                }
                frame
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        // One immediate frame + one tick frame = two polls, two paints (both
        // frames differ from the prior displayed bytes).
        assert_eq!(polls.get(), 2, "immediate frame + one tick = two polls");
        assert_eq!(paints, 2, "both the immediate and changed tick frame paint");
        assert_eq!(displayed, "frame 2", "displayed holds the latest frame");
    }

    #[test]
    fn event_loop_timeout_tick_with_identical_frame_polls_but_does_not_repaint() {
        // An idle server: the tick still polls, but the rendered frame is
        // byte-identical to what's displayed, so suppression skips the paint.
        // The first (immediate) frame paints; the identical tick frame does not.
        let (tx, rx) = mpsc::channel();
        let mut state = WatchState::default();
        let mut displayed = String::new();
        let polls = Cell::new(0_usize);
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls.set(polls.get() + 1);
                Ok(fixture_stats())
            },
            |_state| {
                // Always the same bytes, so the tick frame matches the immediate
                // frame exactly. End after the tick.
                if polls.get() >= 2 {
                    let _ = tx.send(Event::Quit);
                }
                "steady".to_string()
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        assert_eq!(polls.get(), 2, "the idle tick still polls");
        assert_eq!(
            paints, 1,
            "only the immediate frame paints; the identical tick frame is suppressed",
        );
    }

    #[test]
    fn event_loop_quit_after_first_frame_returns_cleanly() {
        // After the immediate first frame, a Quit must end the loop with no
        // further poll/render/paint.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut state = WatchState::default();
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let mut paints = 0_usize;
        let result = event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| "frame".to_string(),
            |_frame| {
                paints += 1;
                Ok(())
            },
        );

        assert!(result.is_ok(), "Quit must return Ok cleanly");
        assert_eq!(polls, 1, "only the immediate frame polls; Quit adds none");
        assert_eq!(paints, 1, "only the immediate frame paints; Quit adds none");
    }

    #[test]
    fn event_loop_disconnected_channel_returns_cleanly() {
        // Every sender dropped (e.g. the reader thread exited) is a clean
        // shutdown: after the immediate frame, recv_timeout returns Disconnected
        // and the loop returns Ok rather than erroring or spinning.
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx); // no events will ever arrive; channel is disconnected

        let mut state = WatchState::default();
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let result = event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| "frame".to_string(),
            |_frame| Ok(()),
        );

        assert!(result.is_ok(), "a disconnected channel is a clean shutdown");
        assert_eq!(polls, 1, "only the immediate frame polls before disconnect");
    }

    #[test]
    fn event_loop_absorbs_a_failed_poll_and_keeps_running() {
        // A failed poll is non-fatal: the loop must NOT return Err, and render
        // must still be called with the error visible in state (so the banner
        // shows). We fail the very first poll, then end on the next tick.
        let (tx, rx) = mpsc::channel();
        let mut state = WatchState::default();
        let mut displayed = String::new();
        let polls = Cell::new(0_usize);
        let saw_error_in_render = Cell::new(false);
        let result = event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls.set(polls.get() + 1);
                // First poll fails; the loop must keep going regardless.
                if polls.get() == 1 {
                    Err(anyhow::anyhow!("connection refused"))
                } else {
                    Ok(fixture_stats())
                }
            },
            |state| {
                // The first (immediate) render runs after the failed poll, so
                // the error must be visible in state here.
                if state.error().is_some() {
                    saw_error_in_render.set(true);
                }
                let n = polls.get();
                if n >= 2 {
                    let _ = tx.send(Event::Quit);
                }
                format!("frame {n}")
            },
            |_frame| Ok(()),
        );

        assert!(
            result.is_ok(),
            "a failed poll must be absorbed, not propagated as Err",
        );
        assert!(
            saw_error_in_render.get(),
            "render must be called with the failed-poll error visible in state",
        );
    }

    #[test]
    fn event_loop_resize_forces_a_repaint_without_polling_even_when_bytes_match() {
        // A resize must re-render from the *current* state at the new dimensions
        // and repaint UNCONDITIONALLY — the on-screen layout is stale after a
        // resize, so suppression must NOT skip the redraw even when the bytes
        // are byte-identical to what's displayed. And a resize must NOT poll:
        // the numbers haven't changed, only the geometry has.
        let (tx, rx) = mpsc::channel();
        // One resize, then quit so the loop ends right after handling it.
        tx.send(Event::Resize).expect("queue resize");
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut state = WatchState::default();
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let mut renders = 0_usize;
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| {
                // Always identical bytes, so the resize's re-render matches what
                // the immediate frame already painted — the perfect adversary
                // for suppression. Only a forced repaint will paint again.
                renders += 1;
                "steady".to_string()
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        assert_eq!(
            polls, 1,
            "a resize must not poll — only the immediate frame polls",
        );
        assert_eq!(
            renders, 2,
            "a resize must re-render from current state (immediate + resize)",
        );
        assert_eq!(
            paints, 2,
            "a resize must paint unconditionally even when bytes are identical",
        );
        assert_eq!(displayed, "steady", "displayed holds the re-rendered frame");
    }

    #[test]
    fn decide_mode_watches_only_on_a_live_tty_without_force() {
        // A live terminal and no `--one-shot`: the default watch experience.
        assert_eq!(
            decide_mode(false, true),
            Mode::Watch,
            "a live TTY with no --one-shot must enter the watch loop",
        );

        // `--one-shot` always wins, even on a live terminal.
        assert_eq!(
            decide_mode(true, true),
            Mode::OneShot,
            "--one-shot must force a single render even on a TTY",
        );

        // A piped / captured stdout can't host the watch UI, so it falls back
        // to a single render regardless of the flag.
        assert_eq!(
            decide_mode(false, false),
            Mode::OneShot,
            "a non-TTY stdout must fall back to one-shot",
        );
        assert_eq!(
            decide_mode(true, false),
            Mode::OneShot,
            "--one-shot on a non-TTY is still one-shot",
        );
    }

    #[test]
    fn is_quit_event_only_quits_on_a_press_of_q_esc_or_ctrl_c() {
        // The three conventional quit keys, all as presses.
        assert!(
            is_quit_event(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Press),
            "pressing q must quit",
        );
        assert!(
            is_quit_event(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press),
            "pressing Esc must quit",
        );
        assert!(
            is_quit_event(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            ),
            "Ctrl-C must quit",
        );

        // A key *release* must never quit, even for a quit key — kitty and
        // Windows report releases and the loop should ignore them.
        assert!(
            !is_quit_event(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            ),
            "releasing q must not quit",
        );

        // A plain 'c' without Control is just a character, not Ctrl-C.
        assert!(
            !is_quit_event(KeyCode::Char('c'), KeyModifiers::NONE, KeyEventKind::Press),
            "plain c (no Control) must not quit",
        );

        // Unrelated keys leave the loop running.
        assert!(
            !is_quit_event(KeyCode::Char('x'), KeyModifiers::NONE, KeyEventKind::Press),
            "an unrelated character must not quit",
        );
        assert!(
            !is_quit_event(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press),
            "Enter must not quit",
        );
    }

    #[test]
    fn should_force_colors_only_when_captured_by_a_wrapper_without_no_color() {
        // The one case that forces colors: a wrapper has captured stdout (not a
        // TTY but COLUMNS is exported) and the user hasn't set NO_COLOR.
        assert!(
            should_force_colors(false, true, false),
            "a watch wrapper (no TTY, COLUMNS set, no NO_COLOR) must force colors",
        );

        // A live TTY needs no override — the colored crate already emits color.
        assert!(
            !should_force_colors(true, true, false),
            "a TTY must not force colors",
        );

        // No COLUMNS means a plain pipe, not a wrapper — leave colors stripped.
        assert!(
            !should_force_colors(false, false, false),
            "a plain pipe (no COLUMNS) must not force colors",
        );

        // NO_COLOR is an explicit opt-out that wins even inside a wrapper.
        assert!(
            !should_force_colors(false, true, true),
            "NO_COLOR must suppress colors even when captured by a wrapper",
        );
    }
}
