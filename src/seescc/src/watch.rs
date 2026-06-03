//! Watch mode: the pure decision helpers that drive the live refresh loop.
//!
//! Phase 4 adds a self-refreshing watch view to seescc. Every *decision* the
//! watch loop makes that does not require a live terminal — which rendering
//! mode to run in, whether a key press means "quit", and whether colors should
//! survive being piped through a watch-like wrapper — lives here as a pure,
//! terminal-free function so it can be unit-tested without a pty. The terminal
//! lifecycle and event loop that consume these helpers are wired in later in
//! this phase.

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
    // Stub: deliberately wrong so the red test fails because the behavior is
    // missing, not because the symbol is undefined.
    let _ = (force_one_shot, stdout_is_tty);
    Mode::Watch
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
}
