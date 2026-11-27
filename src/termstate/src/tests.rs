use crate::{Cell, ColorSource, TerminalState, TerminalTheme};

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
        fg_source: ColorSource::Default,
        bg_source: ColorSource::Default,
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

/// Feeds `text` to a 10 by 3 terminal with the automatic theme.
fn feed_text(text: &str) -> TerminalState {
    let mut state = TerminalState::new(10, 3, TerminalTheme::auto());
    state.process_output(text).unwrap();
    state
}

/// The sources of the foreground and the background of the cell at row 0.
fn sources_at(state: &TerminalState, x: usize) -> (ColorSource, ColorSource) {
    let cell = &state.get_grid()[0][x];
    (cell.fg_source, cell.bg_source)
}

#[test]
fn rgb_values_stay_the_same_beside_the_sources() {
    let state =
        feed_text("\x1b[31;42mA\x1b[91;102mB\x1b[38;5;200;48;5;17mC\x1b[38;2;1;2;3;48;2;4;5;6mD");
    let colors: Vec<_> = (0..4)
        .map(|x| {
            (
                state.get_grid()[0][x].fg_color,
                state.get_grid()[0][x].bg_color,
            )
        })
        .collect();
    assert_eq!(
        colors,
        vec![
            ((255, 85, 85), (85, 255, 85)),
            ((255, 85, 85), (85, 255, 85)),
            ((255, 0, 215), (0, 0, 95)),
            ((1, 2, 3), (4, 5, 6)),
        ]
    );
}

#[test]
fn base_colors_30_to_37_and_40_to_47_give_base_0_to_7() {
    let state = feed_text("\x1b[31;42mA");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Base(1), ColorSource::Base(2))
    );
}

#[test]
fn bright_colors_90_to_97_and_100_to_107_give_base_8_to_15() {
    let state = feed_text("\x1b[91;102mA");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Base(9), ColorSource::Base(10))
    );
}

#[test]
fn the_256_color_form_gives_an_indexed_source() {
    let state = feed_text("\x1b[38;5;200;48;5;17mA");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Indexed(200), ColorSource::Indexed(17))
    );
}

#[test]
fn the_24_bit_form_gives_an_rgb_source() {
    let state = feed_text("\x1b[38;2;1;2;3;48;2;4;5;6mA");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Rgb(1, 2, 3), ColorSource::Rgb(4, 5, 6))
    );
}

#[test]
fn sgr_zero_gives_the_default_source() {
    let state = feed_text("\x1b[31;42m\x1b[0mA");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Default, ColorSource::Default)
    );
    assert_eq!(state.current_fg_source, ColorSource::Default);
    assert_eq!(state.current_bg_source, ColorSource::Default);
}

#[test]
fn a_reset_gives_the_default_source_to_the_next_character() {
    let state = feed_text("\x1b[31;42mA\x1bcB");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Default, ColorSource::Default)
    );
    assert_eq!(state.current_fg_source, ColorSource::Default);
    assert_eq!(state.current_bg_source, ColorSource::Default);
}

#[test]
fn the_pen_sources_follow_the_sgr_codes() {
    let state = feed_text("\x1b[31;102m");
    assert_eq!(state.current_fg_source, ColorSource::Base(1));
    assert_eq!(state.current_bg_source, ColorSource::Base(10));

    let state = feed_text("\x1b[38;5;200;48;2;4;5;6m");
    assert_eq!(state.current_fg_source, ColorSource::Indexed(200));
    assert_eq!(state.current_bg_source, ColorSource::Rgb(4, 5, 6));
}

#[test]
fn an_ignored_color_sequence_leaves_the_pen_sources_alone() {
    let state = feed_text("\x1b[31;41m\x1b[39m\x1b[49m\x1b[38;5m");
    assert_eq!(state.current_fg_source, ColorSource::Base(1));
    assert_eq!(state.current_bg_source, ColorSource::Base(1));
}

#[test]
fn a_cleared_cell_has_the_default_source() {
    let state = feed_text("\x1b[31;41mA\x1b[2J");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Default, ColorSource::Default)
    );
}

#[test]
fn an_erased_cell_has_the_default_source_while_the_pen_stays() {
    let state = feed_text("\x1b[31;41mABC\x1b[1;1H\x1b[1X\x1b[1;2H\x1b[K");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Default, ColorSource::Default)
    );
    assert_eq!(
        sources_at(&state, 2),
        (ColorSource::Default, ColorSource::Default)
    );
    assert_eq!(state.current_fg_source, ColorSource::Base(1));
}

#[test]
fn a_scroll_up_keeps_the_sources_of_the_moved_cell() {
    let state = feed_text("\r\n\x1b[38;5;9;48;2;7;8;9mB\x1b[1S");
    assert_eq!(state.get_grid()[0][0].ch, 'B');
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Indexed(9), ColorSource::Rgb(7, 8, 9))
    );
    // The new bottom row is blank.
    let blank = &state.get_grid()[2][0];
    assert_eq!(
        (blank.fg_source, blank.bg_source),
        (ColorSource::Default, ColorSource::Default)
    );
}

#[test]
fn a_line_feed_at_the_bottom_keeps_the_sources_of_the_moved_cell() {
    let state = feed_text("\r\n\n\x1b[31;42mB\r\n");
    let cell = &state.get_grid()[1][0];
    assert_eq!(cell.ch, 'B');
    assert_eq!(
        (cell.fg_source, cell.bg_source),
        (ColorSource::Base(1), ColorSource::Base(2))
    );
}

#[test]
fn an_inserted_line_has_default_sources_and_the_moved_line_keeps_its_own() {
    let state = feed_text("\x1b[31;42mA\x1b[1;1H\x1b[1L");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Default, ColorSource::Default)
    );
    let moved = &state.get_grid()[1][0];
    assert_eq!(
        (moved.fg_source, moved.bg_source),
        (ColorSource::Base(1), ColorSource::Base(2))
    );
}

#[test]
fn an_inserted_blank_character_has_default_sources() {
    let state = feed_text("\x1b[31;42mAB\x1b[1;1H\x1b[1@");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Default, ColorSource::Default)
    );
    assert_eq!(
        sources_at(&state, 1),
        (ColorSource::Base(1), ColorSource::Base(2))
    );
}

#[test]
fn a_deleted_character_pulls_its_neighbor_with_its_own_sources() {
    let state = feed_text("\x1b[0mA\x1b[31;42mB\x1b[1;1H\x1b[1P");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Base(1), ColorSource::Base(2))
    );
    assert_eq!(
        sources_at(&state, 9),
        (ColorSource::Default, ColorSource::Default)
    );
}

#[test]
fn a_deleted_line_pulls_its_neighbor_with_its_own_sources() {
    let state = feed_text("A\r\n\x1b[31;42mB\x1b[1;1H\x1b[1M");
    assert_eq!(
        sources_at(&state, 0),
        (ColorSource::Base(1), ColorSource::Base(2))
    );
}

#[test]
fn a_clone_keeps_the_pen_sources_and_the_cell_sources() {
    let state = feed_text("\x1b[31;48;5;17mA\x1b[38;2;1;2;3m");
    let copy = state.clone();
    assert_eq!(copy.get_grid(), state.get_grid());
    assert_eq!(copy.current_fg_source, ColorSource::Rgb(1, 2, 3));
    assert_eq!(copy.current_bg_source, ColorSource::Indexed(17));
    assert_eq!(
        sources_at(&copy, 0),
        (ColorSource::Base(1), ColorSource::Indexed(17))
    );
}
