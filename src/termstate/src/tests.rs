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
        dim: false,
        blink: false,
        reverse: false,
        hidden: false,
        strikethrough: false,
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
fn each_restore_brings_back_the_color_sources_and_each_attribute_that_the_save_saw() {
    // The pen has an indexed foreground, a 24-bit background, and each flag
    // that SGR 2, 5, 7, 8 and 9 set. The change after the save gives base
    // colors and clears each flag.
    let pen = "\x1b[38;5;1;48;2;1;2;3;2;5;7;8;9m";
    let reference = feed_new(&format!("{pen}x"));
    let pairs = [
        ("\x1b7", "\x1b8"),
        ("\x1b[s", "\x1b[u"),
        ("\x1b[?1048h", "\x1b[?1048l"),
        ("\x1b[?1049h", "\x1b[?1049l"),
    ];
    for (save, restore) in pairs {
        let state = feed_new(&format!("{pen}{save}\x1b[0;31;41m{restore}x"));
        assert_eq!(
            state.get_grid()[0][0],
            reference.get_grid()[0][0],
            "{save:?} {restore:?}"
        );
    }
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

#[test]
fn modes_47_and_1047_switch_to_the_save_slot_of_the_alternate_screen() {
    for mode in ["47", "1047"] {
        let mut state = feed_new(&format!(
            "\x1b[3;5H\x1b7\x1b[?{mode}h\x1b[10;10H\x1b7\x1b[1;1H\x1b8"
        ));
        assert_eq!(state.get_cursor_position(), (9, 9), "mode {mode}");
        state
            .process_output(&format!("\x1b[?{mode}l\x1b8"))
            .unwrap();
        assert_eq!(state.get_cursor_position(), (4, 2), "mode {mode}");
    }
}

#[test]
fn mode_1048_saves_and_restores_in_the_slot_of_the_current_screen() {
    let state = feed_new("\x1b[3;5H\x1b[?1048h\x1b[1;1H\x1b[?1048l");
    assert_eq!(state.get_cursor_position(), (4, 2));
    // The save of 1048 and the restore of DECRC use one slot: 1048 does not
    // switch the screen.
    let state = feed_new("\x1b[3;5H\x1b[?1048h\x1b[1;1H\x1b8");
    assert_eq!(state.get_cursor_position(), (4, 2));
}

#[test]
fn ris_on_the_alternate_screen_resets_its_save_slot() {
    let state = feed_new("\x1b[?1049h\x1b[10;10H\x1b7\x1bc\x1b[?1049h\x1b8");
    assert_eq!(state.get_cursor_position(), (0, 0));
}

#[test]
fn ris_on_the_alternate_screen_returns_to_the_normal_screen() {
    let state = feed_new("\x1b[?1049h\x1bc\x1b[3;5H\x1b7\x1b[1;1H\x1b[?1049l");
    assert_eq!(state.get_cursor_position(), (4, 2));
}

#[test]
fn restore_count_increases_by_one_for_each_kind_of_restore() {
    for restore in ["\x1b8", "\x1b[u", "\x1b[?1048l", "\x1b[?1049l"] {
        let mut state = feed_new("\x1b7");
        assert_eq!(state.restore_count(), 0, "a save is not a restore");
        state.process_output(restore).unwrap();
        assert_eq!(state.restore_count(), 1, "{restore:?}");
        let copy = state.clone();
        assert_eq!(copy.restore_count(), 1, "a clone keeps the count");
    }
}

#[test]
fn is_settled_is_false_inside_a_sequence_and_true_after_it() {
    assert!(feed_new("").is_settled());
    assert!(!feed_new("\x1b[").is_settled());
    assert!(feed_new("\x1b[5;5H").is_settled());
}

#[test]
fn unfinished_sequence_reopens_the_sequence_in_a_viewer_that_ended_it() {
    let cases = [
        ("", "X"),
        ("\x1b[?25", "l"),
        ("\x1b[\r\n1", "mX"),
        ("\x1b[1\x18", "X"),
        ("\x1b", "[?25l"),
        ("\x1b(", "0q"),
        ("\x1b]0;ti", "tle\x07X"),
    ];
    for (start, rest) in cases {
        let reference = feed_new(&format!("ab{start}{rest}"));
        let reopen = feed_new(&format!("ab{start}")).unfinished_sequence();
        let viewer = feed_new(&format!("ab{start}\x1b7\x1b8{reopen}{rest}"));
        assert_eq!(viewer.get_grid(), reference.get_grid(), "{start:?}");
        assert_eq!(
            viewer.get_cursor_position(),
            reference.get_cursor_position(),
            "{start:?}"
        );
        assert_eq!(
            viewer.is_cursor_visible(),
            reference.is_cursor_visible(),
            "{start:?}"
        );
        assert_eq!(
            viewer.uses_line_drawing(),
            reference.uses_line_drawing(),
            "{start:?}"
        );
    }
}

#[test]
fn unfinished_sequence_is_empty_when_settled() {
    assert_eq!(feed_new("ab").unfinished_sequence(), "");
    assert_eq!(feed_new("\x1b[5;5H").unfinished_sequence(), "");
    assert_eq!(feed_new("\x1b[1\x18").unfinished_sequence(), "");
    assert_eq!(feed_new("\x1b[?25").unfinished_sequence(), "\x1b[?25");
    assert_eq!(feed_new("\x1b[\r\n1").unfinished_sequence(), "\x1b[1");
}

#[test]
fn unfinished_sequence_keeps_the_c0_bytes_after_a_dcs_introducer() {
    // vte 0.15 runs none of these controls in a DCS. In the payload, it gives
    // them to `put`. CAN and SUB end the DCS, and ESC starts a new sequence.
    let controls: String = (0x00..=0x1f_u8)
        .filter(|byte| !matches!(byte, 0x18 | 0x1a | 0x1b))
        .map(char::from)
        .collect();
    let start = format!("\x1bP{controls}q#0{controls}~-");
    assert_eq!(feed_new(&format!("ab{start}")).unfinished_sequence(), start);
    // Before the `P`, the parser is in the escape state, and a control runs.
    assert_eq!(
        feed_new("ab\x1b\r\x07P1$q\x01m").unfinished_sequence(),
        "\x1bP1$q\x01m"
    );
}

#[test]
fn a_sequence_that_can_or_sub_ends_leaves_the_parser_settled() {
    let starts = [
        "\x1b",
        "\x1b[1",
        "\x1b[1;?",
        "\x1b(",
        "\x1b]0;ti",
        "\x1bP1$q",
        "\x1b_Ga",
    ];
    for end in ["\x18", "\x1a"] {
        for start in starts {
            let state = feed_new(&format!("ab{start}{end}"));
            assert!(state.is_settled(), "{start:?} {end:?}");
            assert_eq!(state.unfinished_sequence(), "", "{start:?} {end:?}");
        }
    }
}

#[test]
fn a_csi_sequence_that_vte_ignores_leaves_the_parser_settled_at_its_final_byte() {
    let sequences = [
        "\x1b[1;?h",
        "\x1b[1;?\r\nh",
        "\x1b[ 1m",
        "\x1b[1;?h\r\n",
        "\x1b\r[1;?h",
        "\x1b\x7f[1;?h",
        "\x1bé[1;?h",
    ];
    for sequence in sequences {
        assert!(
            feed_new(&format!("ab{sequence}")).is_settled(),
            "{sequence:?}"
        );
    }
    assert!(!feed_new("ab\x1b[1;?").is_settled());
}

/// A performer that records whether the parser printed a character.
#[derive(Default)]
struct Printed(bool);

impl vte::Perform for Printed {
    fn print(&mut self, _: char) {
        self.0 = true;
    }
}

/// Tells if a new parser of vte is in the ground state after it reads `text`,
/// one byte at a time. Only the ground state prints the next `x`.
fn vte_is_in_ground(text: &str) -> bool {
    let mut parser = vte::Parser::new();
    for byte in text.as_bytes() {
        parser.advance(&mut Printed::default(), &[*byte]);
    }
    let mut probe = Printed::default();
    parser.advance(&mut probe, b"x");
    probe.0
}

/// The characters of the walks of
/// `is_settled_agrees_with_vte_after_each_character`. Each one moves the parser
/// in a different way. `ESC` and `[` are in the list twice, so that more
/// sequences start.
const WALK: [char; 22] = [
    '\x1b', '\x1b', '[', '[', ']', 'P', '_', '?', '1', ';', ' ', 'h', 'q', '\\', '\x18', '\x1a',
    '\x07', '\r', '\x7f', 'é', '\u{9c}', 'x',
];

#[test]
fn is_settled_agrees_with_vte_after_each_character() {
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let count = u64::try_from(WALK.len()).unwrap();
    for cloned in [false, true] {
        for _ in 0..400 {
            let mut state = TerminalState::new(20, 12, TerminalTheme::auto());
            let mut text = String::new();
            for _ in 0..40 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let ch = WALK[usize::try_from(seed % count).unwrap()];
                text.push(ch);
                if cloned {
                    state = state.clone();
                }
                state.process_output(ch.encode_utf8(&mut [0; 4])).unwrap();
                assert_eq!(
                    state.is_settled(),
                    vte_is_in_ground(&text),
                    "{text:?}, cloned {cloned}"
                );
            }
        }
    }
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
fn sgr_39_and_49_give_the_default_source_and_an_incomplete_color_is_ignored() {
    let state = feed_text("\x1b[31;41m\x1b[39m\x1b[49m");
    assert_eq!(state.current_fg_source, ColorSource::Default);
    assert_eq!(state.current_bg_source, ColorSource::Default);

    let state = feed_text("\x1b[31;41m\x1b[38;5m");
    assert_eq!(state.current_fg_source, ColorSource::Base(1));
    assert_eq!(state.current_bg_source, ColorSource::Base(1));
}

#[test]
fn a_cell_after_sgr_39_and_49_has_the_default_source_and_the_old_rgb_value() {
    let state = feed_text("\x1b[31;41mA\x1b[39;49mB");
    let red = &state.get_grid()[0][0];
    let after = &state.get_grid()[0][1];
    assert_eq!(
        (after.fg_source, after.bg_source),
        (ColorSource::Default, ColorSource::Default)
    );
    assert_eq!(after.fg_color, red.fg_color);
    assert_eq!(after.bg_color, red.bg_color);
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

#[test]
fn a_new_terminal_scrolls_the_whole_screen() {
    let state = TerminalState::new(80, 24, TerminalTheme::auto());

    assert_eq!(state.scroll_region(), (0, 23));
}

#[test]
fn decstbm_sets_the_scroll_region_counted_from_zero() {
    let mut state = TerminalState::new(80, 24, TerminalTheme::auto());

    state.process_output("\x1b[2;5r").unwrap();

    assert_eq!(state.scroll_region(), (1, 4));
}

#[test]
fn a_new_terminal_has_the_saved_cursor_at_the_top_left() {
    let state = TerminalState::new(80, 24, TerminalTheme::auto());

    assert_eq!(state.saved_cursor_position(), (0, 0));
}

#[test]
fn each_save_of_the_cursor_gives_the_saved_position() {
    for save in ["\x1b7", "\x1b[s", "\x1b[?1048h"] {
        let mut state = TerminalState::new(80, 24, TerminalTheme::auto());

        state.process_output("\x1b[3;4H").unwrap();
        state.process_output(save).unwrap();
        state.process_output("\x1b[1;1H").unwrap();

        assert_eq!(state.saved_cursor_position(), (3, 2), "{save:?}");
    }
}

#[test]
fn the_save_of_mode_1049_goes_into_the_slot_of_the_normal_screen() {
    let mut state = TerminalState::new(80, 24, TerminalTheme::auto());

    // The alternate screen has a save slot of its own, and nothing saved there.
    state
        .process_output("\x1b[3;4H\x1b[?1049h\x1b[1;1H")
        .unwrap();
    assert_eq!(state.saved_cursor_position(), (0, 0));

    // Mode 1047 goes back to the normal screen and restores nothing.
    state.process_output("\x1b[?1047l").unwrap();
    assert_eq!(state.saved_cursor_position(), (3, 2));
    assert_eq!(state.get_cursor_position(), (0, 0));
}

#[test]
fn line_drawing_is_on_after_the_designation_of_the_special_set() {
    let mut state = TerminalState::new(80, 24, TerminalTheme::auto());
    assert!(!state.uses_line_drawing());

    state.process_output("\x1b(0").unwrap();
    assert!(state.uses_line_drawing());

    state.process_output("\x1b(B").unwrap();
    assert!(!state.uses_line_drawing());
}

/// The text of row `y`, with the trailing blanks cut off.
fn row_text(state: &TerminalState, y: usize) -> String {
    let text: String = state.get_grid()[y].iter().map(|cell| cell.ch).collect();
    text.trim_end().to_string()
}

#[test]
fn a_missing_or_zero_count_of_a_cursor_move_is_one() {
    // The cursor starts at column 5, row 3, counted from zero.
    // A real terminal reads no count and a count of 0 as 1.
    let cases = [
        ("\x1b[A", (5, 2)),
        ("\x1b[0A", (5, 2)),
        ("\x1b[B", (5, 4)),
        ("\x1b[0B", (5, 4)),
        ("\x1b[C", (6, 3)),
        ("\x1b[0C", (6, 3)),
        ("\x1b[D", (4, 3)),
        ("\x1b[0D", (4, 3)),
        ("\x1b[2A", (5, 1)),
        ("\x1b[2D", (3, 3)),
    ];
    for (sequence, expected) in cases {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output("\x1b[4;6H").unwrap();
        state.process_output(sequence).unwrap();
        assert_eq!(state.get_cursor_position(), expected, "{sequence:?}");
    }
}

#[test]
fn a_missing_or_zero_count_of_a_character_edit_is_one() {
    // The cursor goes to column 1, counted from zero, on the row "abcdef".
    let cases = [
        ("\x1b[P", "acdef"),
        ("\x1b[0P", "acdef"),
        ("\x1b[@", "a bcdef"),
        ("\x1b[0@", "a bcdef"),
        ("\x1b[X", "a cdef"),
        ("\x1b[0X", "a cdef"),
        ("\x1b[2P", "adef"),
    ];
    for (sequence, expected) in cases {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output("abcdef\x1b[1;2H").unwrap();
        state.process_output(sequence).unwrap();
        assert_eq!(row_text(&state, 0), expected, "{sequence:?}");
    }
}

#[test]
fn a_missing_or_zero_count_of_a_line_edit_is_one() {
    // The three rows hold "1", "2" and "3". The cursor goes to row 0.
    let cases = [
        ("\x1b[L", ["", "1", "2", "3"]),
        ("\x1b[0L", ["", "1", "2", "3"]),
        ("\x1b[M", ["2", "3", "", ""]),
        ("\x1b[0M", ["2", "3", "", ""]),
        ("\x1b[S", ["2", "3", "", ""]),
        ("\x1b[0S", ["2", "3", "", ""]),
        ("\x1b[T", ["", "1", "2", "3"]),
        ("\x1b[0T", ["", "1", "2", "3"]),
    ];
    for (sequence, expected) in cases {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output("1\r\n2\r\n3\x1b[1;1H").unwrap();
        state.process_output(sequence).unwrap();
        let rows: Vec<String> = (0..4).map(|y| row_text(&state, y)).collect();
        assert_eq!(rows, expected, "{sequence:?}");
    }
}

#[test]
fn a_missing_or_zero_count_of_a_position_is_one() {
    for sequence in ["\x1b[H", "\x1b[0;0H", "\x1b[;H", "\x1b[f", "\x1b[0;0f"] {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output("\x1b[4;6H").unwrap();
        state.process_output(sequence).unwrap();
        assert_eq!(state.get_cursor_position(), (0, 0), "{sequence:?}");
    }
    let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
    state.process_output("\x1b[4;0H").unwrap();
    assert_eq!(state.get_cursor_position(), (0, 3));
    state.process_output("\x1b[0;4H").unwrap();
    assert_eq!(state.get_cursor_position(), (3, 0));
}

#[test]
fn a_missing_or_zero_margin_of_the_scroll_region_is_the_edge_of_the_screen() {
    let cases = [
        ("\x1b[r", (0, 5)),
        ("\x1b[0;0r", (0, 5)),
        ("\x1b[;4r", (0, 3)),
        ("\x1b[0;4r", (0, 3)),
        ("\x1b[3r", (2, 5)),
        ("\x1b[3;0r", (2, 5)),
        ("\x1b[2;4r", (1, 3)),
    ];
    for (sequence, expected) in cases {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output("\x1b[2;3r\x1b[4;6H").unwrap();
        state.process_output(sequence).unwrap();
        assert_eq!(state.scroll_region(), expected, "{sequence:?}");
        assert_eq!(state.get_cursor_position(), (0, 0), "{sequence:?}");
    }
}

/// Sends `sequence` to a terminal of 10 columns and 6 rows, with the cursor
/// at column 5 and row 3, and gives the place of the cursor.
fn cursor_after(sequence: &str) -> (usize, usize) {
    let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
    state.process_output("\x1b[4;6H").unwrap();
    assert_eq!(state.get_cursor_position(), (5, 3));
    state.process_output(sequence).unwrap();
    state.get_cursor_position()
}

#[test]
fn cha_sets_the_column_and_keeps_the_row() {
    assert_eq!(cursor_after("\x1b[G"), (0, 3));
    assert_eq!(cursor_after("\x1b[0G"), (0, 3));
    assert_eq!(cursor_after("\x1b[3G"), (2, 3));
    assert_eq!(cursor_after("\x1b[99G"), (9, 3));
}

#[test]
fn hpa_sets_the_column_and_keeps_the_row() {
    assert_eq!(cursor_after("\x1b[`"), (0, 3));
    assert_eq!(cursor_after("\x1b[0`"), (0, 3));
    assert_eq!(cursor_after("\x1b[3`"), (2, 3));
    assert_eq!(cursor_after("\x1b[99`"), (9, 3));
}

#[test]
fn vpa_sets_the_row_and_keeps_the_column() {
    assert_eq!(cursor_after("\x1b[d"), (5, 0));
    assert_eq!(cursor_after("\x1b[0d"), (5, 0));
    assert_eq!(cursor_after("\x1b[3d"), (5, 2));
    assert_eq!(cursor_after("\x1b[99d"), (5, 5));
}

#[test]
fn cnl_moves_down_and_sets_the_column_to_the_left_edge() {
    assert_eq!(cursor_after("\x1b[E"), (0, 4));
    assert_eq!(cursor_after("\x1b[0E"), (0, 4));
    assert_eq!(cursor_after("\x1b[2E"), (0, 5));
    assert_eq!(cursor_after("\x1b[99E"), (0, 5));
}

#[test]
fn cpl_moves_up_and_sets_the_column_to_the_left_edge() {
    assert_eq!(cursor_after("\x1b[F"), (0, 2));
    assert_eq!(cursor_after("\x1b[0F"), (0, 2));
    assert_eq!(cursor_after("\x1b[2F"), (0, 1));
    assert_eq!(cursor_after("\x1b[99F"), (0, 0));
}

#[test]
fn the_ink_form_of_an_erase_returns_to_the_first_column() {
    let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
    state.process_output("abc\x1b[Gx").unwrap();

    let row: String = state.get_grid()[0].iter().take(3).map(|c| c.ch).collect();
    assert_eq!(row, "xbc");
    assert_eq!(state.get_cursor_position(), (1, 0));
}

/// The five attributes that SGR 2, 5, 7, 8 and 9 set, as `(parameter, off
/// parameter, reader of the pen, reader of a cell)`.
type Attribute = (
    &'static str,
    &'static str,
    fn(&TerminalState) -> bool,
    fn(&Cell) -> bool,
);

fn attributes() -> [Attribute; 5] {
    [
        ("2", "22", |t| t.dim, |c| c.dim),
        ("5", "25", |t| t.blink, |c| c.blink),
        ("7", "27", |t| t.reverse, |c| c.reverse),
        ("8", "28", |t| t.hidden, |c| c.hidden),
        ("9", "29", |t| t.strikethrough, |c| c.strikethrough),
    ]
}

/// Whether the pen of `state` holds each of the eight attributes.
fn pen_attributes(state: &TerminalState) -> [bool; 8] {
    [
        state.bold,
        state.dim,
        state.italic,
        state.underline,
        state.blink,
        state.reverse,
        state.hidden,
        state.strikethrough,
    ]
}

const ALL_ON: &str = "\x1b[1;2;3;4;5;7;8;9m";

#[test]
fn each_sgr_attribute_sets_the_pen_and_the_printed_cell() {
    for (on, _, pen, cell) in attributes() {
        let mut state = TerminalState::new(10, 2, TerminalTheme::auto());
        state.process_output(&format!("\x1b[{on}mA")).unwrap();

        assert!(pen(&state), "SGR {on}: pen");
        assert!(cell(&state.get_grid()[0][0]), "SGR {on}: cell");
        assert!(!cell(&Cell::default()), "SGR {on}: default cell");
    }
}

#[test]
fn each_sgr_off_parameter_clears_its_attribute() {
    for (on, off, pen, cell) in attributes() {
        let mut state = TerminalState::new(10, 2, TerminalTheme::auto());
        state
            .process_output(&format!("\x1b[{on}m\x1b[{off}mA"))
            .unwrap();

        assert!(!pen(&state), "SGR {off}: pen");
        assert!(!cell(&state.get_grid()[0][0]), "SGR {off}: cell");
    }
}

#[test]
fn sgr_22_clears_bold_and_dim() {
    let mut state = TerminalState::new(10, 2, TerminalTheme::auto());
    state.process_output("\x1b[1;2m\x1b[22mA").unwrap();

    assert!(!state.bold);
    assert!(!state.dim);
    assert!(!state.get_grid()[0][0].bold);
    assert!(!state.get_grid()[0][0].dim);
}

const BOLD_ONLY: [bool; 8] = [true, false, false, false, false, false, false, false];

#[test]
fn an_underline_color_sets_no_attribute() {
    for sequence in [
        "\x1b[58;5;196m",
        "\x1b[58;2;7;8;9m",
        "\x1b[58;2;0;0;0m",
        "\x1b[58:5:196m",
        "\x1b[58:2::7:8:9m",
        "\x1b[59m",
    ] {
        let state = feed_text(&format!("\x1b[1m{sequence}"));

        assert_eq!(
            pen_attributes(&state),
            BOLD_ONLY,
            "{sequence:?}: attributes"
        );
        assert_eq!(
            state.current_fg_source,
            ColorSource::Default,
            "{sequence:?}: fg"
        );
        assert_eq!(
            state.current_bg_source,
            ColorSource::Default,
            "{sequence:?}: bg"
        );
    }
}

#[test]
fn a_styled_underline_sets_only_the_underline() {
    for (sequence, on) in [
        ("\x1b[4:0m", false),
        ("\x1b[4:1m", true),
        ("\x1b[4:2m", true),
        ("\x1b[4:3m", true),
        ("\x1b[4:4m", true),
        ("\x1b[4:5m", true),
    ] {
        let state = feed_text(&format!("\x1b[1m{sequence}"));

        assert_eq!(
            pen_attributes(&state),
            [true, false, false, on, false, false, false, false],
            "{sequence:?}"
        );
    }

    let state = feed_text("\x1b[1m\x1b[4m\x1b[4:0m");
    assert_eq!(pen_attributes(&state), BOLD_ONLY, "underline on, then 4:0");
}

#[test]
fn a_colon_extended_color_sets_the_color_and_no_attribute() {
    let fg = |source| (source, ColorSource::Default);
    let bg = |source| (ColorSource::Default, source);
    for (sequence, sources, rgb) in [
        (
            "\x1b[38:2::10:20:30m",
            fg(ColorSource::Rgb(10, 20, 30)),
            Some((10, 20, 30)),
        ),
        (
            "\x1b[38:2:10:20:30m",
            fg(ColorSource::Rgb(10, 20, 30)),
            Some((10, 20, 30)),
        ),
        ("\x1b[38:5:196m", fg(ColorSource::Indexed(196)), None),
        (
            "\x1b[48:2::10:20:30m",
            bg(ColorSource::Rgb(10, 20, 30)),
            Some((10, 20, 30)),
        ),
        ("\x1b[48:5:196m", bg(ColorSource::Indexed(196)), None),
    ] {
        let state = feed_text(sequence);

        assert_eq!(
            pen_attributes(&state),
            [false; 8],
            "{sequence:?}: attributes"
        );
        assert_eq!(
            (state.current_fg_source, state.current_bg_source),
            sources,
            "{sequence:?}: sources"
        );
        if let Some(rgb) = rgb {
            let painted = if sequence.starts_with("\x1b[38") {
                state.current_fg
            } else {
                state.current_bg
            };
            assert_eq!(painted, rgb, "{sequence:?}: rgb");
        }
    }
}

#[test]
fn an_incomplete_extended_color_reads_no_attribute() {
    for sequence in ["\x1b[38;5m", "\x1b[48;2;1;2m", "\x1b[58;5m"] {
        let state = feed_text(sequence);

        assert_eq!(
            pen_attributes(&state),
            [false; 8],
            "{sequence:?}: attributes"
        );
        assert_eq!(
            state.current_fg_source,
            ColorSource::Default,
            "{sequence:?}: fg"
        );
        assert_eq!(
            state.current_bg_source,
            ColorSource::Default,
            "{sequence:?}: bg"
        );
    }
}

#[test]
fn a_colon_group_takes_no_other_group() {
    let state = feed_text("\x1b[1;38:2::10:20:30;4m");

    assert_eq!(
        pen_attributes(&state),
        [true, false, false, true, false, false, false, false]
    );
    assert_eq!(state.current_fg_source, ColorSource::Rgb(10, 20, 30));
}

#[test]
fn sgr_0_clears_all_eight_attributes() {
    let mut state = TerminalState::new(10, 2, TerminalTheme::auto());
    state.process_output(ALL_ON).unwrap();
    assert_eq!(pen_attributes(&state), [true; 8]);

    state.process_output("\x1b[0m").unwrap();

    assert_eq!(pen_attributes(&state), [false; 8]);
}

#[test]
fn ris_clears_all_eight_attributes() {
    let mut state = TerminalState::new(10, 2, TerminalTheme::auto());
    state.process_output(ALL_ON).unwrap();

    state.process_output("\x1bc").unwrap();

    assert_eq!(pen_attributes(&state), [false; 8]);
}

#[test]
fn a_clone_keeps_the_eight_attributes() {
    let mut state = TerminalState::new(10, 2, TerminalTheme::auto());
    state.process_output(ALL_ON).unwrap();

    assert_eq!(pen_attributes(&state.clone()), [true; 8]);
}

// Tests of the autowrap at the last column.

/// A terminal of `width` columns and `height` rows that has read `text`.
fn feed_on(width: usize, height: usize, text: &str) -> TerminalState {
    let mut state = TerminalState::new(width, height, TerminalTheme::auto());
    state.process_output(text).unwrap();
    state
}

#[test]
fn a_line_longer_than_the_screen_wraps_to_the_next_row() {
    let state = feed_on(5, 3, "abcdefg");

    assert_eq!(row_text(&state, 0), "abcde");
    assert_eq!(row_text(&state, 1), "fg");
    assert_eq!(state.get_cursor_position(), (2, 1));
    assert!(state.uses_autowrap());
    assert!(!state.is_wrap_pending());
}

#[test]
fn a_character_in_the_last_column_keeps_the_cursor_there_until_the_next_character() {
    let state = feed_on(5, 3, "abcde");
    assert_eq!(state.get_cursor_position(), (4, 0));
    assert!(state.is_wrap_pending());

    let state = feed_on(5, 3, "abcde\r\nX");
    assert_eq!(row_text(&state, 0), "abcde");
    assert_eq!(row_text(&state, 1), "X");
    assert_eq!(row_text(&state, 2), "");
    assert_eq!(state.get_cursor_position(), (1, 1));
}

#[test]
fn each_cursor_move_ends_the_pending_wrap() {
    assert_eq!(row_text(&feed_on(5, 3, "abcde\x1b[1;1HX"), 0), "Xbcde");
    assert_eq!(row_text(&feed_on(5, 3, "abcde\x1b[1;1HX"), 1), "");
    assert_eq!(row_text(&feed_on(5, 3, "abcde\rX"), 0), "Xbcde");
    assert_eq!(row_text(&feed_on(5, 3, "abcde\rX"), 1), "");
    assert_eq!(row_text(&feed_on(5, 3, "abcde\x08X"), 0), "abcXe");
    assert_eq!(row_text(&feed_on(5, 3, "abcde\x08X"), 1), "");
}

#[test]
fn an_ignored_decstbm_keeps_the_pending_wrap() {
    // CASE_DECSTBM in xterm's charproc.c reads a bottom that is missing, 0,
    // or below the screen as the last row. It ignores a DECSTBM whose top is
    // not above that bottom. Such a DECSTBM moves no cursor, thus the next
    // character wraps.
    for ignored in ["\x1b[3;2r", "\x1b[2;2r"] {
        let state = feed_on(5, 3, &format!("abcde{ignored}X"));
        assert_eq!(row_text(&state, 0), "abcde", "{ignored:?}");
        assert_eq!(row_text(&state, 1), "X", "{ignored:?}");
        assert_eq!(state.scroll_region(), (0, 2), "{ignored:?}");
    }

    // A valid DECSTBM moves the cursor home, thus it ends the pending wrap.
    // A bottom below the screen gives a region to the last row.
    for (valid, region) in [("\x1b[1;2r", (0, 1)), ("\x1b[1;9r", (0, 2))] {
        let state = feed_on(5, 3, &format!("abcde{valid}X"));
        assert_eq!(row_text(&state, 0), "Xbcde", "{valid:?}");
        assert_eq!(row_text(&state, 1), "", "{valid:?}");
        assert_eq!(state.scroll_region(), region, "{valid:?}");
        assert!(!state.is_wrap_pending(), "{valid:?}");
    }
}

#[test]
fn a_wrap_on_the_last_row_scrolls_the_screen() {
    let state = feed_on(5, 2, "abcdefghijk");

    assert_eq!(row_text(&state, 0), "fghij");
    assert_eq!(row_text(&state, 1), "k");
    assert_eq!(state.get_cursor_position(), (1, 1));
}

#[test]
fn a_wrap_on_the_last_row_below_the_scrolling_region_does_not_scroll() {
    // The scrolling region is rows 1 to 3 of 6. On row 6, below the region,
    // xterm does not scroll: the wrap stays on row 6, thus characters 11 and
    // 12 go to columns 1 and 2 of row 6.
    let state = feed_on(10, 6, "1\r\n2\r\n3\x1b[1;3r\x1b[6;1Habcdefghijkl");

    assert_eq!(row_text(&state, 0), "1");
    assert_eq!(row_text(&state, 1), "2");
    assert_eq!(row_text(&state, 2), "3");
    assert_eq!(row_text(&state, 5), "klcdefghij");
    assert_eq!(state.get_cursor_position(), (2, 5));
}

#[test]
fn lf_ind_and_nel_below_the_scrolling_region_stop_at_the_last_row() {
    // The scrolling region is rows 1 to 3 of 6, and the cursor starts on row
    // 5, column 4. Two moves down end on row 6, and nothing scrolls.
    for (move_down, column) in [("\n", 3), ("\x1bD", 3), ("\x1bE", 0)] {
        let text = format!("1\r\n2\r\n3\x1b[1;3r\x1b[5;4H{move_down}{move_down}");
        let state = feed_on(10, 6, &text);

        assert_eq!(row_text(&state, 0), "1", "{move_down:?}");
        assert_eq!(row_text(&state, 1), "2", "{move_down:?}");
        assert_eq!(row_text(&state, 2), "3", "{move_down:?}");
        assert_eq!(state.get_cursor_position(), (column, 5), "{move_down:?}");
    }
}

#[test]
fn cnl_from_inside_the_scrolling_region_stops_at_its_bottom_margin() {
    // Rows 2 to 4 of 6 scroll. From row 3, nine rows down stop on row 4, the
    // bottom margin, as in xterm. Thus the x goes on row 4, not on row 6.
    let state = feed_on(10, 6, "\x1b[2;4r\x1b[3;1H\x1b[9Ex");

    assert_eq!(row_text(&state, 3), "x");
    assert_eq!(row_text(&state, 5), "");
    assert_eq!(state.get_cursor_position(), (1, 3));
}

/// Feeds a screen of 8 rows whose rows 3 to 6 scroll. The cursor starts in
/// column 5 of `start_row`, then `CSI <count> <final_byte>` moves it, then
/// an `x` prints.
///
/// Fails when the `x` is not on `x_row` in `x_column`, or when the cursor is
/// not just after it. Rows are counted from one, and columns from zero.
fn assert_move_in_region(
    final_byte: char,
    start_row: usize,
    count: usize,
    x_row: usize,
    x_column: usize,
) {
    let text = format!("\x1b[3;6r\x1b[{start_row};5H\x1b[{count}{final_byte}x");
    let state = feed_on(10, 8, &text);
    let case = format!("CSI {count} {final_byte} from row {start_row}");

    assert_eq!(state.get_grid()[x_row - 1][x_column].ch, 'x', "{case}");
    assert_eq!(
        state.get_cursor_position(),
        (x_column + 1, x_row - 1),
        "{case}"
    );
}

#[test]
fn cud_and_cnl_stop_at_the_bottom_margin_unless_the_cursor_starts_below_it() {
    // As CursorDown in xterm's cursor.c, a move down stops on the bottom
    // margin (row 6) when the cursor starts on it or above it. From below
    // it, the move stops on the last row of the screen.
    for (final_byte, x_column) in [('B', 4), ('E', 0)] {
        for (start_row, count, x_row) in [
            (1, 9, 6),
            (2, 1, 3),
            (3, 9, 6),
            (4, 1, 5),
            (5, 9, 6),
            (6, 9, 6),
            (7, 1, 8),
            (7, 9, 8),
        ] {
            assert_move_in_region(final_byte, start_row, count, x_row, x_column);
        }
    }
}

#[test]
fn cuu_and_cpl_stop_at_the_top_margin_unless_the_cursor_starts_above_it() {
    // As CursorUp in xterm's cursor.c, a move up stops on the top margin
    // (row 3) when the cursor starts on it or below it. From above it, the
    // move stops on the first row of the screen.
    for (final_byte, x_column) in [('A', 4), ('F', 0)] {
        for (start_row, count, x_row) in [
            (8, 9, 3),
            (7, 1, 6),
            (6, 9, 3),
            (5, 1, 4),
            (4, 9, 3),
            (3, 9, 3),
            (2, 1, 1),
            (2, 9, 1),
        ] {
            assert_move_in_region(final_byte, start_row, count, x_row, x_column);
        }
    }
}

#[test]
fn with_autowrap_off_each_character_after_the_last_column_overwrites_it() {
    let state = feed_on(5, 3, "\x1b[?7labcdefg");

    assert_eq!(row_text(&state, 0), "abcdg");
    assert_eq!(row_text(&state, 1), "");
    assert_eq!(state.get_cursor_position(), (4, 0));
    assert!(!state.is_wrap_pending());
    assert!(!state.uses_autowrap());
}

#[test]
fn ris_turns_autowrap_on_again() {
    let state = feed_on(5, 3, "\x1b[?7l\x1bcabcdefg");

    assert_eq!(row_text(&state, 0), "abcde");
    assert_eq!(row_text(&state, 1), "fg");
    assert!(state.uses_autowrap());
}

#[test]
fn a_clone_keeps_the_autowrap_mode_and_the_pending_wrap() {
    let state = feed_on(5, 3, "\x1b[?7labcde");
    let copy = state.clone();
    assert_eq!(copy.uses_autowrap(), state.uses_autowrap());
    assert_eq!(copy.is_wrap_pending(), state.is_wrap_pending());
    assert!(!copy.uses_autowrap());
    assert!(!copy.is_wrap_pending());

    let state = feed_on(5, 3, "abcde");
    let copy = state.clone();
    assert_eq!(copy.uses_autowrap(), state.uses_autowrap());
    assert_eq!(copy.is_wrap_pending(), state.is_wrap_pending());
    assert!(copy.uses_autowrap());
    assert!(copy.is_wrap_pending());
}

#[test]
fn a_tab_moves_the_cursor_and_writes_no_cell() {
    let state = feed_on(20, 2, "abcdefghij\r\tX");
    assert_eq!(row_text(&state, 0), "abcdefghXj");

    let state = feed_on(10, 2, "\x1b[1;9H\tX");
    assert_eq!(state.get_grid()[0][9].ch, 'X');
    assert_eq!(row_text(&state, 1), "");
}

/// The characters of the cells of row `y`, from column `from` to column `to`
/// (not included).
fn cells_of(state: &TerminalState, y: usize, from: usize, to: usize) -> Vec<char> {
    state.get_grid()[y][from..to]
        .iter()
        .map(|cell| cell.ch)
        .collect()
}

#[test]
fn a_wide_character_takes_two_cells() {
    let state = feed_on(10, 2, "日本");

    assert_eq!(cells_of(&state, 0, 0, 4), vec!['日', '\0', '本', '\0']);
    assert_eq!(state.get_cursor_position(), (4, 0));
}

#[test]
fn a_wide_character_that_does_not_fit_wraps_to_the_next_row() {
    let state = feed_on(5, 2, "abcd日");

    assert_eq!(cells_of(&state, 0, 0, 5), vec!['a', 'b', 'c', 'd', ' ']);
    assert_eq!(state.get_grid()[1][0].ch, '日');
    assert_eq!(state.get_grid()[1][1].ch, '\0');
    assert_eq!(state.get_cursor_position(), (2, 1));
}

#[test]
fn a_wide_character_in_the_last_two_columns_holds_a_pending_wrap() {
    let mut state = feed_on(5, 2, "abc日");

    assert_eq!(state.get_grid()[0][3].ch, '日');
    assert_eq!(state.get_grid()[0][4].ch, '\0');
    assert_eq!(state.get_cursor_position(), (4, 0));
    assert!(state.is_wrap_pending());

    state.process_output("x").unwrap();
    assert_eq!(state.get_grid()[1][0].ch, 'x');
}

#[test]
fn a_wide_character_with_autowrap_off_that_does_not_fit_gets_no_cell() {
    let state = feed_on(5, 2, "\x1b[?7labcd日");

    assert_eq!(cells_of(&state, 0, 0, 5), vec!['a', 'b', 'c', 'd', ' ']);
    assert_eq!(state.get_cursor_position(), (4, 0));
}

#[test]
fn a_write_on_the_second_cell_of_a_wide_character_blanks_its_first_cell() {
    let state = feed_on(10, 2, "日\x1b[1;2Hx");

    assert_eq!(cells_of(&state, 0, 0, 2), vec![' ', 'x']);
}

#[test]
fn a_write_on_the_first_cell_of_a_wide_character_blanks_its_second_cell() {
    let state = feed_on(10, 2, "日\x1b[1;1Hx");

    assert_eq!(cells_of(&state, 0, 0, 2), vec!['x', ' ']);
}

#[test]
fn a_wide_character_over_two_wide_characters_blanks_the_halves_it_cuts() {
    let state = feed_on(10, 2, "日本\x1b[1;2H語");

    assert_eq!(cells_of(&state, 0, 0, 4), vec![' ', '語', '\0', ' ']);
    assert_eq!(state.get_cursor_position(), (3, 0));
}

#[test]
fn a_combining_mark_takes_no_cell() {
    let state = feed_on(10, 2, "e\u{301}x");

    assert_eq!(cells_of(&state, 0, 0, 3), vec!['e', 'x', ' ']);
    assert_eq!(state.get_cursor_position(), (2, 0));
}

#[test]
fn a_combining_mark_after_the_last_column_keeps_the_pending_wrap() {
    let state = feed_on(5, 2, "\x1b[2;1Habcde\u{301}");

    assert_eq!(row_text(&state, 1), "abcde");
    assert_eq!(state.get_cursor_position(), (4, 1));
    assert!(state.is_wrap_pending());
}

#[test]
fn an_erase_that_cuts_a_wide_character_blanks_the_rest_of_it() {
    let state = feed_on(10, 2, "日本\x1b[1;2H\x1b[K");

    assert_eq!(cells_of(&state, 0, 0, 10), vec![' '; 10]);
}

#[test]
fn a_delete_that_cuts_a_wide_character_blanks_the_rest_of_it() {
    let state = feed_on(10, 2, "a日b\x1b[1;2H\x1b[P");

    assert_eq!(cells_of(&state, 0, 0, 4), vec!['a', ' ', 'b', ' ']);
}

#[test]
fn an_insert_that_cuts_a_wide_character_blanks_the_rest_of_it() {
    let state = feed_on(10, 2, "a日b\x1b[1;3H\x1b[@");

    assert_eq!(cells_of(&state, 0, 0, 5), vec!['a', ' ', ' ', ' ', 'b']);
}

/// Ten columns and six rows, each row full of one letter. The cursor ends on
/// the last row, in the last column, with a wrap pending.
const MARK_BASE: &str =
    "AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC\r\nDDDDDDDDDD\r\nEEEEEEEEEE\r\nFFFFFFFFFF";

#[test]
fn each_change_of_a_cell_marks_its_row() {
    // (name, setup, sequence). The setup follows `MARK_BASE`.
    let cases: &[(&str, &str, &str)] = &[
        ("print", "\x1b[2;3H", "x"),
        ("wrap", "\x1b[2;10Hy", "z"),
        ("wrap on the last row, a scroll", "", "z"),
        ("wide character", "\x1b[3;2H", "日"),
        (
            "print over the head of a wide character",
            "\x1b[3;2H日",
            "\x1b[3;2Hq",
        ),
        (
            "print over the tail of a wide character",
            "\x1b[3;2H日",
            "\x1b[3;3Hq",
        ),
        ("LF on the last row", "\x1b[6;1H", "\n"),
        ("ED 0", "\x1b[3;4H", "\x1b[J"),
        ("ED 1", "\x1b[3;4H", "\x1b[1J"),
        ("ED 2", "\x1b[3;4H", "\x1b[2J"),
        ("EL 0", "\x1b[3;4H", "\x1b[K"),
        ("EL 1", "\x1b[3;4H", "\x1b[1K"),
        ("EL 2", "\x1b[3;4H", "\x1b[2K"),
        ("ECH", "\x1b[3;4H", "\x1b[3X"),
        ("DCH", "\x1b[3;4H", "\x1b[2P"),
        (
            "DCH that cuts a wide character",
            "\x1b[3;1H日日",
            "\x1b[3;2H\x1b[P",
        ),
        ("ICH", "\x1b[3;4H", "\x1b[2@"),
        ("IL", "\x1b[3;1H", "\x1b[L"),
        ("DL", "\x1b[3;1H", "\x1b[M"),
        ("SU", "", "\x1b[2S"),
        ("SD", "", "\x1b[2T"),
        ("RI on the top row", "\x1b[H", "\x1bM"),
        ("IND on the last row", "\x1b[6;1H", "\x1bD"),
        ("NEL on the last row", "\x1b[6;1H", "\x1bE"),
        ("RIS", "", "\x1bc"),
        ("DECSTBM and LF at its bottom", "\x1b[2;4r\x1b[4;1H", "\n"),
        ("SU in a scrolling region", "\x1b[2;4r", "\x1b[S"),
        ("SD in a scrolling region", "\x1b[2;4r", "\x1b[T"),
        (
            "RI at the top of a scrolling region",
            "\x1b[2;4r\x1b[2;1H",
            "\x1bM",
        ),
        ("CSI 2 J after text", "\x1b[3;3Hhello", "\x1b[2J"),
    ];
    for (name, setup, sequence) in cases {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output(MARK_BASE).unwrap();
        state.process_output(setup).unwrap();
        state.take_changed_rows();
        let before = state.grid.clone();
        state.process_output(sequence).unwrap();
        let marked = state.take_changed_rows();
        let changed: Vec<usize> = before
            .iter()
            .zip(state.grid.iter())
            .enumerate()
            .filter_map(|(row, (old, new))| (old != new).then_some(row))
            .collect();
        assert!(
            !changed.is_empty(),
            "{name} ({sequence:?}) must change a cell, or the case proves nothing"
        );
        for row in &changed {
            assert!(
                marked.contains(row),
                "{name} ({sequence:?}) changed row {row}, marked rows: {marked:?}"
            );
        }
    }
}

#[test]
fn a_move_of_the_cursor_or_a_change_of_the_pen_marks_no_row() {
    for sequence in ["\x1b[3;4H", "\x1b[31m", "\x1b]2;title\x07", "\x1b[?25l"] {
        let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
        state.process_output(MARK_BASE).unwrap();
        state.take_changed_rows();
        state.process_output(sequence).unwrap();
        assert_eq!(
            state.take_changed_rows(),
            Vec::<usize>::new(),
            "{sequence:?}"
        );
    }
}

#[test]
fn take_changed_rows_clears_the_marks() {
    let mut state = TerminalState::new(10, 6, TerminalTheme::auto());
    state.process_output("a\r\nb").unwrap();
    assert_eq!(state.take_changed_rows(), vec![0, 1]);
    assert_eq!(state.take_changed_rows(), Vec::<usize>::new());
}

#[test]
fn two_terminals_that_read_the_same_bytes_are_equal() {
    assert!(feed_text("ab\x1b[31mc\r\n") == feed_text("ab\x1b[31mc\r\n"));
}

#[test]
fn an_echo_and_its_erase_leave_a_terminal_equal_to_one_that_read_neither() {
    assert!(feed_text("$ h") == feed_text("$ hr\x08 \x08"));
}

#[test]
fn the_pen_counts_although_the_screen_is_the_same() {
    assert!(feed_text("ab") != feed_text("ab\x1b[31m"));
}

#[test]
fn the_saved_cursor_counts_although_the_cursor_is_the_same() {
    assert!(feed_text("") != feed_text("\x1b[5C\x1b7\x1b[5D"));
}

#[test]
fn the_alternate_screen_counts_although_the_screen_is_the_same() {
    // Mode 47 sets only the flag: it does not clear the grid or save a cursor.
    // A later save goes to the other slot.
    assert!(feed_text("") != feed_text("\x1b[?47h"));
}

#[test]
fn the_saved_cursor_of_the_alternate_screen_counts() {
    assert!(feed_text("\x1b[?47h") != feed_text("\x1b[?47h\x1b[5C\x1b7\x1b[5D"));
}

#[test]
fn the_count_of_restores_does_not_count() {
    // No later byte reads the count, so a restore that brings back the same
    // state gives an equal terminal.
    assert!(feed_text("\x1b7\x1b8") == feed_text("\x1b7"));
}

#[test]
fn an_unfinished_sequence_counts_until_it_ends() {
    let mut unfinished = feed_text("x\x1b[");
    assert!(unfinished != feed_text("x"));

    unfinished.process_output("m").unwrap();
    assert!(unfinished == feed_text("x"));
}

#[test]
fn the_theme_counts_although_only_a_palette_color_differs() {
    // The two themes have the same default colors, so the screens are the
    // same. A later `CSI 31 m` paints in different reds.
    let mut other = TerminalTheme::auto();
    other.red = (1, 2, 3);
    assert!(TerminalState::new(10, 3, TerminalTheme::auto()) != TerminalState::new(10, 3, other));
}

#[test]
fn each_attribute_of_the_pen_counts_although_the_screen_is_the_same() {
    // The eight parameters of `ALL_ON`. `attributes()` holds only five.
    let parameters = ALL_ON
        .trim_start_matches("\x1b[")
        .trim_end_matches('m')
        .split(';');
    for on in parameters {
        assert!(
            feed_text("ab") != feed_text(&format!("ab\x1b[{on}m")),
            "SGR {on}"
        );
    }
}

#[test]
fn the_source_of_each_pen_color_counts_although_its_rgb_is_the_same() {
    // A base color and a 24-bit color of the same RGB value paint the same
    // cell, but a later change of the theme changes only the base color.
    let (r, g, b) = TerminalTheme::auto().red;

    let base = feed_text("ab\x1b[31m");
    let rgb = feed_text(&format!("ab\x1b[38;2;{r};{g};{b}m"));
    assert_eq!(base.current_fg, rgb.current_fg);
    assert!(base != rgb, "foreground");

    let base = feed_text("ab\x1b[41m");
    let rgb = feed_text(&format!("ab\x1b[48;2;{r};{g};{b}m"));
    assert_eq!(base.current_bg, rgb.current_bg);
    assert!(base != rgb, "background");
}

/// The name of one field of `TerminalState`, and a change of that field alone.
type FieldChange = (&'static str, fn(&mut TerminalState));

#[test]
fn a_change_of_one_field_makes_the_terminals_unequal() {
    // A byte sequence often changes two fields, and then a comparison that
    // skips one of them still finds the other. A 24-bit color changes the pen
    // color and its source, because `ColorSource::Rgb` holds the RGB value.
    // Thus each case changes one field of a copy.
    let cases: &[FieldChange] = &[
        ("cursor_x", |t| t.cursor_x += 1),
        ("cursor_y", |t| t.cursor_y += 1),
        ("width", |t| t.width += 1),
        ("height", |t| t.height += 1),
        ("current_fg", |t| t.current_fg = (1, 2, 3)),
        ("current_bg", |t| t.current_bg = (1, 2, 3)),
        ("saved", |t| t.saved[0].y += 1),
        ("alt_screen", |t| t.alt_screen = !t.alt_screen),
        ("track", |t| t.track = crate::Track::Csi),
        ("scroll_top", |t| t.scroll_top += 1),
        ("scroll_bottom", |t| t.scroll_bottom -= 1),
        ("use_acs", |t| t.use_acs = !t.use_acs),
        ("cursor_visible", |t| t.cursor_visible = !t.cursor_visible),
        ("selection_fg", |t| t.selection_fg = None),
        ("selection_bg", |t| t.selection_bg = None),
        ("override_fg", |t| t.override_fg = Some((1, 2, 3))),
        ("override_bg", |t| t.override_bg = Some((1, 2, 3))),
        ("dynamic_palette", |t| t.dynamic_palette[0] = (1, 2, 3)),
        ("grid", |t| t.grid[0][0].ch = 'x'),
    ];
    let base = feed_text("ab");
    for (field, change) in cases {
        let mut changed = base.clone();
        change(&mut changed);
        assert!(base != changed, "{field}");
    }
}

#[test]
fn the_autowrap_mode_counts_although_the_screen_is_the_same() {
    assert!(feed_text("ab") != feed_text("ab\x1b[?7l"));
}

#[test]
fn a_pending_wrap_counts_although_the_cursor_is_the_same() {
    // A move away and back clears the pending wrap, and the cursor ends in
    // the last column again.
    let pending = feed_text("0123456789");
    let cleared = feed_text("0123456789\x1b[D\x1b[C");
    assert_eq!(
        (pending.cursor_x, pending.cursor_y),
        (cleared.cursor_x, cleared.cursor_y)
    );
    assert!(pending != cleared);
}

#[test]
fn the_marks_of_the_changed_rows_do_not_count() {
    let mut taken = feed_text("ab");
    assert_eq!(taken.take_changed_rows(), vec![0]);
    assert!(taken == feed_text("ab"));
}
