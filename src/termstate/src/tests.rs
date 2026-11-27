use crate::{Cell, TerminalState, TerminalTheme};

#[test]
fn test_terminal_theme_creation() {
    let theme = TerminalTheme::dracula();
    assert_eq!(theme.background, (40, 42, 54));
    assert_eq!(theme.foreground, (248, 248, 242));

    let theme = TerminalTheme::from_name("monokai");
    assert_eq!(theme.background, (39, 40, 34));

    let theme = TerminalTheme::from_name("unknown");
    assert_eq!(theme.background, (0, 0, 0)); // defaults to black
}

#[test]
fn test_terminal_state() {
    let theme = TerminalTheme::auto();
    let mut state = TerminalState::new(80, 24, theme);

    state.process_output("Hello").unwrap();

    {
        let grid = state.get_grid();
        let first_row = &grid[0];
        assert_eq!(first_row[0].ch, 'H');
        assert_eq!(first_row[1].ch, 'e');
        assert_eq!(first_row[2].ch, 'l');
    }

    assert_eq!(state.get_width(), 80);
    assert_eq!(state.get_height(), 24);
    assert_eq!(state.get_theme().foreground, (255, 255, 255));
}

#[test]
fn test_scroll_up_via_newline_at_bottom() {
    // Tests scroll_up() logic via newline when cursor is at bottom of screen.
    let theme = TerminalTheme::auto();
    let mut state = TerminalState::new(10, 3, theme);

    // Fill all three rows
    state.process_output("A\r\nB\r\nC").unwrap();
    assert_eq!(state.get_grid()[0][0].ch, 'A');
    assert_eq!(state.get_grid()[1][0].ch, 'B');
    assert_eq!(state.get_grid()[2][0].ch, 'C');

    // Newline at the bottom should scroll up
    state.process_output("\r\n").unwrap();

    let grid = state.get_grid();
    assert_eq!(grid[0][0].ch, 'B', "Row 0 should have B after scroll up");
    assert_eq!(grid[1][0].ch, 'C', "Row 1 should have C after scroll up");
    assert_eq!(
        grid[2][0].ch, ' ',
        "Bottom row should be cleared after scroll up"
    );
}

#[test]
fn test_scroll_down_via_reverse_index() {
    // Tests scroll_down logic via ESC M (reverse index) at top of scroll region.
    let theme = TerminalTheme::auto();
    let mut state = TerminalState::new(10, 3, theme);

    // Write "A" on row 0
    state.process_output("A").unwrap();
    assert_eq!(state.get_grid()[0][0].ch, 'A');

    // Move cursor to row 0 col 0
    state.process_output("\x1b[1;1H").unwrap();

    // ESC M (reverse index) at the top should scroll content down
    state.process_output("\x1bM").unwrap();

    let grid = state.get_grid();
    assert_eq!(
        grid[0][0].ch, ' ',
        "Row 0 should be cleared after scroll down"
    );
    assert_eq!(
        grid[1][0].ch, 'A',
        "Content should have moved down to row 1"
    );
}

#[test]
fn test_delete_characters_large_count_no_panic() {
    let theme = TerminalTheme::auto();
    let mut state = TerminalState::new(10, 5, theme);

    state.process_output("Hello").unwrap();

    // Move cursor to column 2, then delete 999 chars (way more than width)
    // This should NOT panic from usize underflow
    state.process_output("\x1b[1;3H\x1b[999P").unwrap();

    let grid = state.get_grid();
    assert_eq!(grid[0][0].ch, 'H');
    assert_eq!(grid[0][1].ch, 'e');
    // Remaining columns should be blank
    assert_eq!(grid[0][2].ch, ' ');
}

#[test]
fn test_insert_characters_large_count_no_panic() {
    let theme = TerminalTheme::auto();
    let mut state = TerminalState::new(10, 5, theme);

    state.process_output("Hello").unwrap();

    // Move cursor to column 2, then insert 999 chars (way more than width)
    // This should NOT panic from usize underflow
    state.process_output("\x1b[1;3H\x1b[999@").unwrap();

    let grid = state.get_grid();
    assert_eq!(grid[0][0].ch, 'H');
    assert_eq!(grid[0][1].ch, 'e');
    // Inserted blanks should fill the rest
    assert_eq!(grid[0][2].ch, ' ');
}

#[test]
fn test_parse_color_spec_ascii_guard() {
    let theme = TerminalTheme::auto();
    let state = TerminalState::new(10, 5, theme);

    // Valid hex
    assert_eq!(state.parse_color_spec_pub("#ff0000"), Some((255, 0, 0)));
    assert_eq!(state.parse_color_spec_pub("#000000"), Some((0, 0, 0)));

    // Valid rgb:
    assert_eq!(
        state.parse_color_spec_pub("rgb:ff/00/ff"),
        Some((255, 0, 255))
    );
    assert_eq!(
        state.parse_color_spec_pub("rgb:ffff/0000/ffff"),
        Some((255, 0, 255))
    );

    // Named
    assert_eq!(state.parse_color_spec_pub("red"), Some((255, 0, 0)));

    // Non-ASCII should return None, not panic
    assert_eq!(state.parse_color_spec_pub("#ff\u{00e9}000"), None);

    // Invalid
    assert_eq!(state.parse_color_spec_pub("garbage"), None);
}

#[test]
fn test_resolve_cell_colors_with_unmodified_palette() {
    let theme = TerminalTheme::auto();
    let state = TerminalState::new(10, 5, theme);

    let cell = Cell {
        ch: 'A',
        fg_color: (255, 255, 255),
        bg_color: (0, 0, 0),
        bold: false,
        italic: false,
        underline: false,
    };

    let (fg, bg) = state.resolve_cell_colors(&cell);
    // With no palette modifications, colors should pass through
    assert_eq!(fg, (255, 255, 255));
    assert_eq!(bg, (0, 0, 0));
}

#[test]
fn the_replay_buffer_holds_only_an_unfinished_sequence() {
    let feed = |text: &str| {
        let mut state = TerminalState::new(8, 4, TerminalTheme::auto());
        state.process_output(text).unwrap();
        state
    };

    assert!(feed("plain\x1b[31mred\x1b[0m\r\n").pending.is_empty());
    assert!(feed("日本語").pending.is_empty());
    assert!(feed("\x1b]0;title\x07\n").pending.is_empty());
    assert!(feed("\x1bP1$qm\x1b\\").pending.is_empty());

    let mut state = feed("\x1b[3");
    assert_eq!(state.pending, b"\x1b[3");
    state.process_output("1mX").unwrap();
    assert!(state.pending.is_empty());
}

/// A terminal of 10 columns and 3 rows that read `text`.
fn fed(text: &str) -> TerminalState {
    let mut state = TerminalState::new(10, 3, TerminalTheme::auto());
    state.process_output(text).unwrap();
    state
}

#[test]
fn two_terminals_that_read_the_same_bytes_are_equal() {
    assert!(fed("ab\x1b[31mc\r\n") == fed("ab\x1b[31mc\r\n"));
}

#[test]
fn an_echo_and_its_erase_leave_a_terminal_equal_to_one_that_read_neither() {
    assert!(fed("$ h") == fed("$ hr\x08 \x08"));
}

#[test]
fn the_pen_counts_although_the_screen_is_the_same() {
    assert!(fed("ab") != fed("ab\x1b[31m"));
}

#[test]
fn the_saved_cursor_counts_although_the_cursor_is_the_same() {
    assert!(fed("") != fed("\x1b[5C\x1b7\x1b[5D"));
}

#[test]
fn an_unfinished_sequence_counts_until_it_ends() {
    let mut unfinished = fed("x\x1b[");
    assert!(unfinished != fed("x"));

    unfinished.process_output("m").unwrap();
    assert!(unfinished == fed("x"));
}

#[test]
fn the_theme_counts_although_only_a_palette_color_differs() {
    // The two themes have the same default colors, so the screens are the
    // same. A later `CSI 31 m` paints in different reds.
    let mut other = TerminalTheme::auto();
    other.red = (1, 2, 3);
    assert!(TerminalState::new(10, 3, TerminalTheme::auto()) != TerminalState::new(10, 3, other));
}
