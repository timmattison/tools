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
