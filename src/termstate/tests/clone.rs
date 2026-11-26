//! Tests of `Clone` for `TerminalState`, through the public API only.

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use termstate::{TerminalState, TerminalTheme};

/// Makes a small terminal with the automatic theme.
fn terminal() -> TerminalState {
    TerminalState::new(8, 4, TerminalTheme::auto())
}

#[test]
fn a_clone_holds_the_screen_of_the_original() {
    let mut original = terminal();
    original.process_output("\x1b[31mhello\r\nworld").unwrap();

    let copy = original.clone();

    assert_eq!(copy.get_grid(), original.get_grid());
    assert_eq!(copy.get_cursor_position(), original.get_cursor_position());
    assert_eq!(copy.is_cursor_visible(), original.is_cursor_visible());
}

#[test]
fn feeding_a_clone_leaves_the_original_unchanged() {
    let mut original = terminal();
    original.process_output("hello").unwrap();
    let grid_before = original.get_grid().clone();
    let cursor_before = original.get_cursor_position();

    let mut copy = original.clone();
    copy.process_output("\x1b[2J\x1b[Hbye").unwrap();

    assert_eq!(original.get_grid(), &grid_before);
    assert_eq!(original.get_cursor_position(), cursor_before);
    assert_ne!(copy.get_grid(), &grid_before);
}
