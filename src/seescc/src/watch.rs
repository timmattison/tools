//! Watch mode: the pure decision helpers that drive the live refresh loop.
//!
//! Phase 4 adds a self-refreshing watch view to seescc. Every *decision* the
//! watch loop makes that does not require a live terminal — which rendering
//! mode to run in, whether a key press means "quit", and whether colors should
//! survive being piped through a watch-like wrapper — lives here as a pure,
//! terminal-free function so it can be unit-tested without a pty. The terminal
//! lifecycle and event loop that consume these helpers are wired in later in
//! this phase.

use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

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
