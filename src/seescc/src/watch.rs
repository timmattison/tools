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
