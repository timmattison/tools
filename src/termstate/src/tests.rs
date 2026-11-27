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

/// Feeds `text` to a new terminal of 20 columns and 12 rows.
fn feed_new(text: &str) -> TerminalState {
    let mut state = TerminalState::new(20, 12, TerminalTheme::auto());
    state.process_output(text).unwrap();
    state
}

/// Checks that a save and a restore bring back the red color and clear flags.
fn assert_red_plain_x(state: &TerminalState) {
    let red = state.theme.get_color(1);
    let cell = &state.get_grid()[0][0];
    assert_eq!(cell.ch, 'x');
    assert_eq!(cell.fg_color, red);
    assert!(!cell.bold);
    assert!(!cell.italic);
    assert!(!cell.underline);
}

#[test]
fn decrc_restores_the_colors_and_the_flags_that_decsc_saved() {
    let state = feed_new("\x1b[31m\x1b7\x1b[32;1;3;4m\x1b8x");
    assert_red_plain_x(&state);
}

#[test]
fn csi_s_and_csi_u_save_and_restore_like_decsc() {
    let state = feed_new("\x1b[31m\x1b[s\x1b[32;1;3;4m\x1b[ux");
    assert_red_plain_x(&state);
}

#[test]
fn decrc_restores_the_character_set_that_decsc_saved() {
    let state = feed_new("\x1b7\x1b(0\x1b8q");
    assert_eq!(state.get_grid()[0][0].ch, 'q');
    let state = feed_new("\x1b(0\x1b7\x1b(B\x1b8q");
    assert_eq!(state.get_grid()[0][0].ch, '─');
}

#[test]
fn the_alternate_screen_has_its_own_save_slot() {
    let mut state = feed_new("\x1b[3;5H\x1b[?1049h\x1b[10;10H\x1b7\x1b[1;1H\x1b8");
    assert_eq!(state.get_cursor_position(), (9, 9));
    state.process_output("\x1b[?1049l").unwrap();
    assert_eq!(state.get_cursor_position(), (4, 2));
}

#[test]
fn leaving_the_alternate_screen_restores_the_attributes_of_the_normal_screen() {
    let state = feed_new("\x1b[31m\x1b[?1049h\x1b[32m\x1b[?1049lx");
    assert_eq!(state.get_grid()[0][0].fg_color, state.theme.get_color(1));
}

#[test]
fn ris_resets_the_save_slots() {
    let state = feed_new("\x1b[5;5H\x1b[31m\x1b7\x1bc\x1b8x");
    let cell = &state.get_grid()[0][0];
    assert_eq!(cell.ch, 'x');
    assert_eq!(cell.fg_color, state.theme.foreground);
}
