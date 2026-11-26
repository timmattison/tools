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

#[test]
fn a_clone_taken_inside_an_escape_sequence_finishes_it() {
    let mut original = terminal();
    original.process_output("A\x1b[3").unwrap();
    let mut copy = original.clone();

    original.process_output("1mB").unwrap();
    copy.process_output("1mB").unwrap();

    assert_eq!(copy.get_grid(), original.get_grid());
    let cell = &original.get_grid()[0][1];
    assert_eq!(cell.ch, 'B');
    assert_eq!(cell.fg_color, TerminalTheme::auto().red);
}

/// Fragments that the random test joins. They cover text, wide and accented
/// characters, controls, and the pieces of CSI, OSC, DCS, and SOS sequences.
const FRAGMENTS: &[&str] = &[
    "A",
    "b",
    "日",
    "é",
    " ",
    "\n",
    "\r",
    "\t",
    "\x08",
    "\x07",
    "\x1b",
    "[",
    "]",
    "?",
    ";",
    "0",
    "1",
    "3",
    "7",
    "25",
    "31",
    "1049",
    "38;5;",
    "48;2;",
    "m",
    "H",
    "J",
    "K",
    "h",
    "l",
    "r",
    "P",
    "@",
    "X",
    "L",
    "M",
    "S",
    "T",
    "C",
    "8",
    "c",
    "(",
    "B",
    "\x1b[",
    "\x1b]",
    "4;1;#123456",
    "10;rgb:ff/00/00",
    "\x1b\\",
    "\x1bP",
    "\x1bX",
    "\x18",
    "\x1a",
    "\u{9c}",
    "\u{85}",
    "\x7f",
];

/// A small SplitMix64 generator. The seed alone decides the sequence.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a number below `bound`, which must not be zero.
    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).unwrap()).unwrap()
    }
}

#[test]
fn clones_at_random_split_points_match_one_uninterrupted_run() {
    for seed in 0..500_u64 {
        let mut rng = SplitMix64(seed);
        let fragments: Vec<&str> = (0..40)
            .map(|_| FRAGMENTS[rng.below(FRAGMENTS.len())])
            .collect();
        let text: String = fragments.concat();

        let mut reference = terminal();
        reference.process_output(&text).unwrap();

        // Three random fragment boundaries, in order.
        let mut cuts: Vec<usize> = (0..3).map(|_| rng.below(fragments.len() + 1)).collect();
        cuts.sort_unstable();
        let mut state = terminal();
        let mut start = 0;
        for cut in cuts.into_iter().chain([fragments.len()]) {
            state
                .process_output(&fragments[start..cut].concat())
                .unwrap();
            state = state.clone();
            start = cut;
        }

        let context = format!("seed {seed}, text {text:?}");
        assert_eq!(state.get_grid(), reference.get_grid(), "grid: {context}");
        assert_eq!(
            state.get_cursor_position(),
            reference.get_cursor_position(),
            "cursor: {context}"
        );
        assert_eq!(
            state.is_cursor_visible(),
            reference.is_cursor_visible(),
            "visibility: {context}"
        );
        for (row_state, row_reference) in state.get_grid().iter().zip(reference.get_grid()) {
            for (cell_state, cell_reference) in row_state.iter().zip(row_reference) {
                assert_eq!(
                    state.resolve_cell_colors(cell_state),
                    reference.resolve_cell_colors(cell_reference),
                    "colors: {context}"
                );
            }
        }
    }
}
