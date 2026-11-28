//! Tests of the painter of the keystroke HUD. A test replays the output of the
//! painter in a viewer, and compares the viewer with a reference that gets only
//! the recording.

use super::{bg_code, fg_code, fits, layout, Overlay, Painter};
use crate::filters::keystroke_hud::options::{HudOptions, Position, Rgb};
use crate::filters::keystroke_hud::timeline::{Piece, Shown};
use crate::pipeline::TerminalSize;
use termstate::{ColorSource, TerminalState, TerminalTheme};

/// The size of a screen.
fn size(columns: u16, rows: u16) -> TerminalSize {
    TerminalSize {
        width: columns,
        height: rows,
    }
}

/// A piece of typed text.
fn typed(text: &str) -> Piece {
    Piece::Text(text.to_string())
}

/// A piece for a special key.
fn special(label: &str, count: u32) -> Piece {
    Piece::Special {
        label: label.to_string(),
        count,
    }
}

/// What the HUD shows, at full opacity.
fn shown(pieces: Vec<Piece>) -> Shown {
    Shown { pieces, alpha: 1.0 }
}

/// The options with a position.
fn at(position: Position) -> HudOptions {
    HudOptions {
        position,
        ..HudOptions::default()
    }
}

/// The text of each row of the box.
fn box_text(overlay: &Overlay) -> Vec<String> {
    overlay
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.ch).collect())
        .collect()
}

/// The box of the label `text` on `screen` with `position`.
fn box_of(screen: TerminalSize, position: Position, text: &str) -> Overlay {
    layout(screen, &at(position), &shown(vec![typed(text)])).expect("the screen has room")
}

/// A painter, a viewer that gets the output of the painter, and a reference
/// that gets only the recording.
struct Rig {
    /// The painter under test.
    painter: Painter,
    /// The screen of the viewer.
    viewer: TerminalState,
    /// The screen of the recording, with no HUD.
    reference: TerminalState,
}

impl Rig {
    /// A rig for a screen of `screen`, with the theme of the export `theme`.
    fn new(screen: TerminalSize, theme: &TerminalTheme) -> Self {
        let width = usize::from(screen.width);
        let height = usize::from(screen.height);
        Self {
            painter: Painter::new(screen).expect("the screen has room"),
            viewer: TerminalState::new(width, height, theme.clone()),
            reference: TerminalState::new(width, height, theme.clone()),
        }
    }

    /// Feed the recording to the painter, to the viewer, and to the reference.
    fn feed(&mut self, data: &str) {
        self.painter.feed(data);
        self.viewer.process_output(data).unwrap();
        self.reference.process_output(data).unwrap();
    }

    /// Paint, and replay the output in the viewer.
    fn paint(&mut self, overlay: Option<&Overlay>) -> Option<String> {
        let output = self.painter.paint(overlay);
        if let Some(text) = &output {
            self.viewer.process_output(text).unwrap();
        }
        output
    }

    /// Finish, and replay the output in the viewer.
    fn finish(&mut self) -> Option<String> {
        let output = self.painter.finish();
        if let Some(text) = &output {
            self.viewer.process_output(text).unwrap();
        }
        output
    }

    /// The text of the viewer in row `row`.
    fn row(&self, row: usize) -> String {
        self.viewer.get_grid()[row]
            .iter()
            .map(|cell| cell.ch)
            .collect()
    }

    /// The text of the viewer in row `row`, from column `left`, `width` cells.
    fn slice(&self, row: usize, left: usize, width: usize) -> String {
        self.row(row).chars().skip(left).take(width).collect()
    }

    /// Assert that the viewer shows the same as the reference: the grid, the
    /// cursor, the visibility of the cursor, and the current attributes.
    fn assert_viewer_is_reference(&self) {
        assert_eq!(self.viewer.get_grid(), self.reference.get_grid());
        assert_eq!(
            self.viewer.get_cursor_position(),
            self.reference.get_cursor_position()
        );
        assert_eq!(
            self.viewer.is_cursor_visible(),
            self.reference.is_cursor_visible()
        );
        assert_eq!(self.viewer.current_fg, self.reference.current_fg);
        assert_eq!(self.viewer.current_bg, self.reference.current_bg);
        assert_eq!(self.viewer.bold, self.reference.bold);
        assert_eq!(self.viewer.italic, self.reference.italic);
        assert_eq!(self.viewer.underline, self.reference.underline);
        assert_eq!(self.viewer.dim, self.reference.dim);
        assert_eq!(self.viewer.blink, self.reference.blink);
        assert_eq!(self.viewer.reverse, self.reference.reverse);
        assert_eq!(self.viewer.hidden, self.reference.hidden);
        assert_eq!(self.viewer.strikethrough, self.reference.strikethrough);
        assert_eq!(
            self.viewer.current_fg_source,
            self.reference.current_fg_source
        );
        assert_eq!(
            self.viewer.current_bg_source,
            self.reference.current_bg_source
        );
    }
}

/// The sources of the two colors of the first cell after `data`.
fn cell_sources(data: &str) -> (ColorSource, ColorSource) {
    let mut state = TerminalState::new(4, 2, TerminalTheme::auto());
    state.process_output(data).unwrap();
    let cell = &state.get_grid()[0][0];
    (cell.fg_source, cell.bg_source)
}

#[test]
fn each_color_source_maps_back_to_the_code_that_set_it() {
    let foregrounds = [
        "39",
        "30",
        "31",
        "32",
        "33",
        "34",
        "35",
        "36",
        "37",
        "90",
        "91",
        "92",
        "93",
        "94",
        "95",
        "96",
        "97",
        "38;5;21",
        "38;5;200",
        "38;2;10;20;30",
    ];
    let backgrounds = [
        "49",
        "40",
        "41",
        "42",
        "43",
        "44",
        "45",
        "46",
        "47",
        "100",
        "101",
        "102",
        "103",
        "104",
        "105",
        "106",
        "107",
        "48;5;21",
        "48;2;10;20;30",
    ];
    let dracula = TerminalTheme::dracula();
    let cell_after = |data: &str| {
        let mut state = TerminalState::new(4, 2, dracula.clone());
        state.process_output(data).unwrap();
        state.get_grid()[0][0].clone()
    };
    for code in foregrounds {
        let (fg, _) = cell_sources(&format!("\x1b[{code}mx"));
        let mapped = fg_code(fg);
        assert_eq!(
            cell_after(&format!("\x1b[{mapped}mx")),
            cell_after(&format!("\x1b[{code}mx")),
            "foreground {code} mapped to {mapped}"
        );
    }
    for code in backgrounds {
        let (_, bg) = cell_sources(&format!("\x1b[{code}mx"));
        let mapped = bg_code(bg);
        assert_eq!(
            cell_after(&format!("\x1b[{mapped}mx")),
            cell_after(&format!("\x1b[{code}mx")),
            "background {code} mapped to {mapped}"
        );
    }
}

#[test]
fn layout_draws_a_rounded_box_with_the_label() {
    let overlay = box_of(size(40, 10), Position::TopRight, "ls");
    assert_eq!(box_text(&overlay), ["╭────╮", "│ ls │", "╰────╯"]);
    assert_eq!((overlay.top, overlay.left), (1, 33));
}

#[test]
fn special_keys_are_bold_in_the_accent_color_and_apart_from_text() {
    let options = HudOptions::default();
    let pieces = vec![special("Ctrl+R", 1), typed("foo")];
    let overlay = layout(size(40, 10), &options, &shown(pieces)).unwrap();
    let label = box_text(&overlay).remove(1);
    assert_eq!(label, "│ Ctrl+R foo │");
    let accent = (
        options.accent.red,
        options.accent.green,
        options.accent.blue,
    );
    let text = (options.text.red, options.text.green, options.text.blue);
    let cells = &overlay.rows[1];
    // Skip the border and the padding.
    for cell in &cells[2..8] {
        assert_eq!((cell.fg, cell.bold), (accent, true), "cell {}", cell.ch);
    }
    // The space between the pieces and the typed text.
    for cell in &cells[8..12] {
        assert_eq!((cell.fg, cell.bold), (text, false), "cell {}", cell.ch);
    }

    let counted = layout(size(40, 10), &options, &shown(vec![special("↓", 3)])).unwrap();
    assert_eq!(box_text(&counted)[1], "│ ↓ ×3 │");
}

#[test]
fn each_position_puts_the_box_in_its_place() {
    let expected = [
        (Position::TopLeft, (1, 1)),
        (Position::TopCenter, (1, 17)),
        (Position::TopRight, (1, 33)),
        (Position::BottomLeft, (6, 1)),
        (Position::BottomCenter, (6, 17)),
        (Position::BottomRight, (6, 33)),
    ];
    assert_eq!(expected.len(), Position::ALL.len());
    for (position, place) in expected {
        let overlay = box_of(size(40, 10), position, "ls");
        assert_eq!((overlay.top, overlay.left), place, "{}", position.as_str());
    }
}

#[test]
fn a_long_label_keeps_its_end() {
    let overlay = box_of(
        size(20, 10),
        Position::TopLeft,
        "abcdefghijklmnopqrstuvwxyz0123",
    );
    let label: String = overlay.rows[1].iter().map(|cell| cell.ch).collect();
    assert_eq!(label, "│ …rstuvwxyz0123 │");
    assert_eq!(overlay.rows[1].len(), 14 + 4);
}

#[test]
fn a_cut_label_starts_with_no_second_half_of_a_wide_character() {
    // A screen of 12 columns has room for a label of 6 cells. The label
    // `a日日日` takes 7 cells, and a cut of its first cell leaves the second
    // half of the first `日`.
    let overlay = box_of(size(12, 6), Position::TopLeft, "a日日日");
    let label: String = overlay.rows[1].iter().map(|cell| cell.ch).collect();
    assert_eq!(label, "│ …日\0日\0 │");
}

#[test]
fn a_small_terminal_has_no_box() {
    assert!(!fits(size(6, 10)));
    assert!(!fits(size(40, 4)));
    assert!(fits(size(7, 5)));
    let options = HudOptions::default();
    for small in [size(6, 10), size(40, 4)] {
        assert!(layout(small, &options, &shown(vec![typed("a")])).is_none());
        assert!(Painter::new(small).is_none());
    }
    assert!(Painter::new(size(7, 5)).is_some());
}

#[test]
fn the_alpha_blends_from_the_background_of_the_theme() {
    let options = HudOptions {
        border: Rgb::new(0x80, 0x80, 0x80),
        ..HudOptions::default()
    };
    let pieces = vec![typed("a")];
    let half = Shown {
        pieces: pieces.clone(),
        alpha: 0.5,
    };
    let overlay = layout(size(40, 10), &options, &half).unwrap();
    assert_eq!(overlay.rows[0][0].fg, (64, 64, 64));
    assert_eq!(overlay.rows[1][2].fg, (128, 128, 128));
    assert_eq!(overlay.rows[1][2].bg, Some((24, 24, 24)));

    let full = layout(size(40, 10), &options, &shown(pieces)).unwrap();
    assert_eq!(overlay.rows[0][0].bg, None);
    assert_eq!(full.rows[0][0].fg, (128, 128, 128));
    assert_eq!(full.rows[1][2].fg, (255, 255, 255));
    assert_eq!(full.rows[1][2].bg, Some((48, 48, 48)));
}

#[test]
fn a_painter_with_nothing_to_draw_paints_nothing() {
    let mut rig = Rig::new(size(20, 6), &TerminalTheme::auto());
    assert_eq!(rig.paint(None), None);
    rig.feed("hello\r\nworld\x1b[31m");
    assert_eq!(rig.paint(None), None);
    rig.assert_viewer_is_reference();
}

#[test]
fn the_paint_shows_the_box_and_a_later_paint_removes_it() {
    let screen = size(20, 6);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    rig.feed("$ ls\r\n");
    assert!(rig.paint(Some(&overlay)).is_some());
    assert_eq!(rig.slice(2, 13, 6), "│ ls │");
    assert_eq!(rig.slice(1, 13, 6), "╭────╮");
    assert_eq!(rig.slice(3, 13, 6), "╰────╯");
    assert_eq!(rig.paint(Some(&overlay)), None);
    assert!(rig.paint(None).is_some());
    rig.assert_viewer_is_reference();
    assert_eq!(rig.paint(None), None);
}

#[test]
fn the_paint_keeps_the_box_in_place_after_a_scroll() {
    let screen = size(20, 6);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::BottomRight, "ls");
    let (top, left) = (overlay.top, overlay.left);
    for line in 0..12 {
        rig.feed(&format!("line number {line}\r\n"));
        rig.paint(Some(&overlay));
        assert_eq!(rig.slice(top, left, 6), "╭────╮", "after line {line}");
        assert_eq!(rig.slice(top + 1, left, 6), "│ ls │", "after line {line}");
        assert_eq!(rig.slice(top + 2, left, 6), "╰────╯", "after line {line}");
        for (row, (seen, wanted)) in rig
            .viewer
            .get_grid()
            .iter()
            .zip(rig.reference.get_grid())
            .enumerate()
        {
            for (column, (seen, wanted)) in seen.iter().zip(wanted).enumerate() {
                let inside = (top..top + 3).contains(&row) && (left..left + 6).contains(&column);
                if !inside {
                    assert_eq!(seen, wanted, "row {row}, column {column}, line {line}");
                }
            }
        }
    }
    rig.paint(None);
    rig.assert_viewer_is_reference();
}

#[test]
fn the_paint_restores_the_colors_of_the_cells_under_the_box() {
    let screen = size(30, 6);
    let mut rig = Rig::new(screen, &TerminalTheme::dracula());
    let overlay = box_of(screen, Position::BottomRight, "ls");
    assert_eq!((overlay.top, overlay.left), (2, 23));
    rig.feed("\x1b[3;24H\x1b[31;1mRED\x1b[0m \x1b[48;5;21mBLU\x1b[0m");
    rig.feed("\x1b[4;24H\x1b[4;3;32mabc\x1b[0m\x1b[41mdef\x1b[0m");
    rig.paint(Some(&overlay));
    assert_eq!(rig.slice(3, 23, 6), "│ ls │");
    rig.paint(None);
    rig.assert_viewer_is_reference();
}

#[test]
fn the_paint_restores_the_default_colors_that_sgr_39_and_49_set() {
    let screen = size(20, 6);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    assert_eq!((overlay.top, overlay.left), (1, 13));
    // SGR 39 and SGR 49 change only the source of a color in `termstate`, and
    // the RGB value of red and green stays. Thus the RGB value of the replay
    // does not tell the color that a viewer shows.
    rig.feed("\x1b[2;14H\x1b[31;42m\x1b[39;49mxyz");
    rig.paint(Some(&overlay));
    assert_eq!(rig.slice(1, 13, 6), "╭────╮");
    rig.paint(None);
    let cell = &rig.viewer.get_grid()[1][13];
    assert_eq!(cell.ch, 'x');
    assert_eq!(
        (cell.fg_source, cell.bg_source),
        (ColorSource::Default, ColorSource::Default)
    );
}

#[test]
fn the_paint_restores_each_text_flag_of_the_cells_under_the_box() {
    let screen = size(20, 6);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    assert_eq!((overlay.top, overlay.left), (1, 13));
    // Each cell under the top border has one flag: dim, blink, reverse,
    // hidden, and struck through. The pen keeps reverse and struck through.
    rig.feed("\x1b[2;14H\x1b[2mA\x1b[0;5mB\x1b[0;7mC\x1b[0;8mD\x1b[0;9mE\x1b[0;7;9m");
    rig.paint(Some(&overlay));
    assert_eq!(rig.slice(1, 13, 6), "╭────╮");
    rig.paint(None);
    rig.assert_viewer_is_reference();
}

#[test]
fn the_paint_repairs_the_cursor_after_a_restore_of_the_program() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    rig.feed("\x1b[5;5H\x1b7\x1b[1;1H");
    assert!(rig.paint(Some(&overlay)).is_some());
    rig.feed("\x1b8X");
    assert!(rig.paint(Some(&overlay)).is_some());
    assert_eq!(rig.viewer.get_grid()[4][4].ch, 'X');
    assert_eq!(rig.viewer.get_grid()[0][0].ch, ' ');
    assert_eq!(
        rig.viewer.get_cursor_position(),
        rig.reference.get_cursor_position()
    );
    rig.paint(None);
    rig.assert_viewer_is_reference();
}

#[test]
fn the_label_is_ascii_when_the_program_uses_the_line_drawing_set() {
    let screen = size(20, 6);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "lq");
    rig.feed("\x1b(0");
    rig.paint(Some(&overlay));
    assert_eq!(rig.slice(2, 13, 6), "│ lq │");
    rig.feed("q");
    assert_eq!(rig.paint(Some(&overlay)), None);
    assert_eq!(rig.viewer.get_grid()[0][0].ch, '─');
    assert_eq!(rig.viewer.get_grid()[0][0], rig.reference.get_grid()[0][0]);
}

#[test]
fn the_repair_selects_a_character_set_only_when_the_sets_differ() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    rig.feed("\x1b7\x1b[5;5H");
    rig.paint(Some(&overlay));
    rig.paint(None);
    // The recording restores its own save at the home position. The viewer
    // restores the save of the last paint.
    rig.feed("\x1b8");
    let repair = rig.paint(None).expect("the cursors differ");
    assert!(!repair.contains("\x1b("), "{repair:?}");
    rig.assert_viewer_is_reference();
}

#[test]
fn a_paint_with_no_box_scans_nothing_while_the_replays_agree() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    for _ in 0..100 {
        rig.feed("line\r\n");
        assert!(rig.paint(None).is_none());
    }
    assert_eq!(rig.painter.scans, 0);
    let overlay = box_of(screen, Position::TopRight, "ls");
    assert!(rig.paint(Some(&overlay)).is_some());
    rig.paint(None);
    rig.feed("y");
    rig.paint(None);
    let scans = rig.painter.scans;
    for _ in 0..100 {
        rig.feed("z");
        rig.paint(None);
    }
    assert_eq!(rig.painter.scans, scans);
    rig.assert_viewer_is_reference();
}

#[test]
fn a_restore_of_the_program_after_the_replays_agree_is_repaired() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    rig.feed("\x1b[5;5H\x1b7\x1b[1;1H");
    assert!(rig.paint(Some(&overlay)).is_some());
    rig.paint(None);
    rig.feed("y");
    rig.paint(None);
    rig.feed("\x1b8X");
    rig.paint(None);
    rig.assert_viewer_is_reference();
}

#[test]
fn a_paint_inside_an_unfinished_sequence_of_the_program_writes_nothing() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    let other = box_of(screen, Position::TopRight, "cd");
    rig.feed("ab");
    assert!(rig.paint(Some(&overlay)).is_some());
    rig.feed("\x1b[?25");
    assert_eq!(rig.paint(None), None);
    assert_eq!(rig.paint(Some(&other)), None);
    rig.feed("l");
    rig.paint(None);
    rig.assert_viewer_is_reference();
}

#[test]
fn a_finish_inside_an_unfinished_sequence_of_the_program_opens_it_again() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    rig.feed("ab");
    rig.paint(Some(&overlay));
    rig.feed("\x1b[?25");
    let output = rig.finish().expect("the box is on the screen");
    assert!(output.ends_with("\x1b[?25"), "{output:?}");
    rig.feed("l");
    rig.assert_viewer_is_reference();
}

#[test]
fn a_finish_inside_a_dcs_of_the_program_opens_it_again_with_its_controls() {
    let screen = size(20, 8);
    let mut rig = Rig::new(screen, &TerminalTheme::auto());
    let overlay = box_of(screen, Position::TopRight, "ls");
    rig.feed("ab");
    rig.paint(Some(&overlay));
    // The `\x01` and the `\n` are payload of the DCS, not controls that run.
    let start = "\x1bPq#0\x01\n-";
    rig.feed(start);
    let output = rig.finish().expect("the box is on the screen");
    assert!(output.ends_with(start), "{output:?}");
    rig.feed("\x1b\\");
    rig.assert_viewer_is_reference();
}

#[test]
fn a_paint_after_a_canceled_or_ignored_sequence_takes_the_box_away() {
    let screen = size(20, 8);
    for sequence in ["\x1b[1\x18", "\x1b]0;ti\x1a", "\x1b[1;?h"] {
        let mut rig = Rig::new(screen, &TerminalTheme::auto());
        let overlay = box_of(screen, Position::TopRight, "ls");
        rig.feed("ab");
        assert!(rig.paint(Some(&overlay)).is_some());
        rig.feed(sequence);
        assert!(rig.paint(None).is_some(), "{sequence:?}");
        rig.assert_viewer_is_reference();
    }
}
