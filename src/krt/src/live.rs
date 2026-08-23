//! The live display of a run, and the keys that drive it.
//!
//! A live run holds the terminal in raw mode, so the terminal sends the bytes
//! of every key straight to this process. Raw mode clears `ISIG`, which is the
//! setting that turns Ctrl-C into a `SIGINT`. Ctrl-C therefore arrives here as
//! a key press, and the signal handler of `main.rs` never sees it. That is the
//! reason this module classifies the keys itself: it is the one part of the
//! live run that can stop a run that the user asked to stop.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// What one key press asks for.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the tests of this module read the whole table, and the display that acts on a command joins it in the next step of this work. The expectation fails once that display lands, which takes the attribute back off"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Stop the run.
    Quit,
    /// Hold the display where it stands, or let it move again.
    Pause,
    /// Show the names of the addresses, or show the addresses.
    Names,
    /// Empty the table of the display, and count the rounds again from zero.
    Reset,
    /// Show the list of the keys, or hide it.
    Help,
}

/// What one key press means, or nothing.
///
/// - A key release means nothing. A kitty terminal and a Windows terminal both
///   report a release, and only a press acts.
/// - Ctrl-C stops the run, and `q` stops it too. Raw mode clears `ISIG`, so
///   Ctrl-C arrives as this key press and the process takes no `SIGINT`. A run
///   that ignored it would need a second terminal to stop.
/// - `p` holds the display, `n` turns the names on or off, `r` empties the
///   table of the display, and `?` shows the list of the keys.
/// - Every other key means nothing.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the tests of this module read the whole table, and the display that polls the keyboard joins it in the next step of this work. The expectation fails once that display lands, which takes the attribute back off"
    )
)]
pub(crate) fn classify(key: KeyEvent) -> Option<Command> {
    let KeyEvent {
        code,
        modifiers,
        kind,
        ..
    } = key;

    if kind == KeyEventKind::Release {
        return None;
    }

    // Checked in front of the table of the letters, so no mode that a later
    // display adds can trap the user.
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        return Some(Command::Quit);
    }

    match code {
        KeyCode::Char('q') => Some(Command::Quit),
        KeyCode::Char('p') => Some(Command::Pause),
        KeyCode::Char('n') => Some(Command::Names),
        KeyCode::Char('r') => Some(Command::Reset),
        KeyCode::Char('?') => Some(Command::Help),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, Command};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    /// Builds the press of a key that no modifier holds.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_q_key_asks_for_a_quit() {
        assert_eq!(classify(press(KeyCode::Char('q'))), Some(Command::Quit));
    }

    #[test]
    fn ctrl_c_asks_for_a_quit() {
        // Raw mode clears `ISIG`, so Ctrl-C arrives as a key press and not as a
        // signal. A run that ignored it would need a second terminal to stop.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(classify(ctrl_c), Some(Command::Quit));
    }

    #[test]
    fn the_p_key_asks_for_a_pause() {
        assert_eq!(classify(press(KeyCode::Char('p'))), Some(Command::Pause));
    }

    #[test]
    fn the_n_key_asks_for_the_names() {
        assert_eq!(classify(press(KeyCode::Char('n'))), Some(Command::Names));
    }

    #[test]
    fn the_r_key_asks_for_a_reset() {
        assert_eq!(classify(press(KeyCode::Char('r'))), Some(Command::Reset));
    }

    #[test]
    fn the_question_mark_asks_for_the_help() {
        assert_eq!(classify(press(KeyCode::Char('?'))), Some(Command::Help));
    }

    #[test]
    fn a_release_of_a_mapped_key_asks_for_nothing() {
        // A kitty terminal and a Windows terminal both report a release. Only a
        // press acts, or one press of `q` stops the run two times.
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press(KeyCode::Char('q'))
        };
        assert_eq!(classify(release), None);
    }

    #[test]
    fn a_release_of_ctrl_c_asks_for_nothing() {
        // The check of the release stands in front of the check of Ctrl-C, so
        // the escape that no table can trap also obeys the rule of the press.
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        };
        assert_eq!(classify(release), None);
    }

    #[test]
    fn a_letter_that_the_table_does_not_hold_asks_for_nothing() {
        assert_eq!(classify(press(KeyCode::Char('x'))), None);
    }

    #[test]
    fn a_key_that_carries_no_letter_asks_for_nothing() {
        assert_eq!(classify(press(KeyCode::Enter)), None);
    }
}
