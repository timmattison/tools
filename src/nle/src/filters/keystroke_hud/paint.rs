//! The painter of the keystroke HUD. It keeps two replays of the recording:
//! the screen as the recording draws it, and the screen as the viewer sees it
//! with the HUD. The difference between the two replays is the output that the
//! filter must write.
//!
//! The painter writes each color of a cell in the form of its source in the
//! replay: the default color, one of the 16 base colors, an entry of the
//! 256-color table, or a 24-bit color. Thus a repaint of a cell is correct in
//! each theme of the export.
//!
//! A wide character takes two cells, as in `termstate` and in a terminal: the
//! character and a continuation cell. The painter writes no byte for a
//! continuation cell, because the write of the character covers both cells.
//! A terminal cannot show one half of a wide character. Thus, while the edge
//! of the box covers one half of a wide character of the program, the other
//! half shows blank.

use super::options::{HudOptions, Position, Rgb};
use super::timeline::{Piece, Shown};
use crate::pipeline::TerminalSize;
use termstate::{Cell, ColorSource, TerminalState, TerminalTheme, WIDE_CONTINUATION};
use unicode_width::UnicodeWidthChar;

/// The cells between the box and the edge of the screen.
const MARGIN: usize = 1;
/// The blank cells between the border and the label, on each side.
const PADDING: usize = 1;
/// The rows of the box: the top border, the label, the bottom border.
const BOX_ROWS: usize = 3;

/// A color as three channels.
type Color = (u8, u8, u8);

/// The sequence that selects the DEC line drawing character set.
const SELECT_LINE_DRAWING: &str = "\x1b(0";
/// The sequence that selects the ASCII character set.
const SELECT_ASCII: &str = "\x1b(B";

/// The SGR code that sets the foreground to `source`.
fn fg_code(source: ColorSource) -> String {
    match source {
        ColorSource::Default => "39".to_string(),
        ColorSource::Base(index) if index < 8 => format!("{}", 30 + u32::from(index)),
        ColorSource::Base(index) => format!("{}", 90 + u32::from(index) - 8),
        ColorSource::Indexed(index) => format!("38;5;{index}"),
        ColorSource::Rgb(red, green, blue) => format!("38;2;{red};{green};{blue}"),
    }
}

/// The SGR code that sets the background to `source`.
fn bg_code(source: ColorSource) -> String {
    match source {
        ColorSource::Default => "49".to_string(),
        ColorSource::Base(index) if index < 8 => format!("{}", 40 + u32::from(index)),
        ColorSource::Base(index) => format!("{}", 100 + u32::from(index) - 8),
        ColorSource::Indexed(index) => format!("48;5;{index}"),
        ColorSource::Rgb(red, green, blue) => format!("48;2;{red};{green};{blue}"),
    }
}

/// The source of the 24-bit color `color`.
fn rgb_source((red, green, blue): Color) -> ColorSource {
    ColorSource::Rgb(red, green, blue)
}

/// Whether the screen of `size` has room for a box with a label of one cell.
pub(crate) fn fits(size: TerminalSize) -> bool {
    usize::from(size.width) >= 2 * MARGIN + 2 * PADDING + 3
        && usize::from(size.height) >= 2 * MARGIN + BOX_ROWS
}

/// One cell of a box. `bg` is `None` for a border cell, which keeps the
/// background of the cell under it.
struct OverlayCell {
    /// The character of the cell.
    ch: char,
    /// The foreground color.
    fg: Color,
    /// The background color, or `None` to keep the one under the cell.
    bg: Option<Color>,
    /// Whether the character is bold.
    bold: bool,
}

/// The cells of one box, and where it stands on the screen.
pub(crate) struct Overlay {
    /// The row of the top border.
    top: usize,
    /// The column of the left border.
    left: usize,
    /// The rows of the box.
    rows: Vec<Vec<OverlayCell>>,
}

impl Overlay {
    /// The cell of the box at the screen position (`row`, `column`), or `None`
    /// when the position is outside the box.
    fn cell_at(&self, row: usize, column: usize) -> Option<&OverlayCell> {
        self.rows
            .get(row.checked_sub(self.top)?)?
            .get(column.checked_sub(self.left)?)
    }
}

/// The style of a cell of the label.
#[derive(Clone, Copy)]
enum Style {
    /// Typed text.
    Text,
    /// The label of a special key.
    Accent,
}

/// The color that is `alpha` of the way from `from` to `to`.
fn blend(from: Rgb, to: Rgb, alpha: f64) -> Color {
    let channel = |from: u8, to: u8| {
        let value = f64::from(from) + (f64::from(to) - f64::from(from)) * alpha;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the value is rounded and clamped to 0..=255"
        )]
        let byte = value.round().clamp(0.0, 255.0) as u8;
        byte
    };
    (
        channel(from.red, to.red),
        channel(from.green, to.green),
        channel(from.blue, to.blue),
    )
}

/// Add the cells of `ch` with `style` to `cells`, as `termstate` gives them: a
/// wide character takes the character and a continuation cell, and a
/// character of no width takes none.
fn push_cells(cells: &mut Vec<(char, Style)>, ch: char, style: Style) {
    match UnicodeWidthChar::width(ch).unwrap_or(1) {
        0 => {}
        1 => cells.push((ch, style)),
        _ => {
            cells.push((ch, style));
            cells.push((WIDE_CONTINUATION, style));
        }
    }
}

/// The cells of the label of `shown`, each with its style.
fn label_cells(shown: &Shown) -> Vec<(char, Style)> {
    let mut cells = Vec::new();
    let mut before_is_special = None;
    for piece in &shown.pieces {
        let is_special = matches!(piece, Piece::Special { .. });
        if before_is_special.is_some_and(|before| before || is_special) {
            cells.push((' ', Style::Text));
        }
        match piece {
            Piece::Text(text) => {
                for ch in text.chars() {
                    push_cells(&mut cells, ch, Style::Text);
                }
            }
            Piece::Special { label, count } => {
                let counted = if *count > 1 {
                    format!(" \u{d7}{count}")
                } else {
                    String::new()
                };
                for ch in label.chars().chain(counted.chars()) {
                    push_cells(&mut cells, ch, Style::Accent);
                }
            }
        }
        before_is_special = Some(is_special);
    }
    cells
}

/// The box that shows `shown`, or `None` when the screen of `size` has no room.
pub(crate) fn layout(size: TerminalSize, options: &HudOptions, shown: &Shown) -> Option<Overlay> {
    if !fits(size) {
        return None;
    }
    let columns = usize::from(size.width);
    let rows = usize::from(size.height);
    let mut label = label_cells(shown);
    let most = columns.saturating_sub(2 * MARGIN + 2 * PADDING + 2);
    if label.len() > most {
        let keep = most.saturating_sub(1);
        let mut kept = label.split_off(label.len().saturating_sub(keep));
        // The cut leaves no second half of a wide character.
        if kept.first().is_some_and(|(ch, _)| *ch == WIDE_CONTINUATION) {
            kept.remove(0);
        }
        kept.insert(0, ('\u{2026}', Style::Text));
        label = kept;
    }
    let width = label.len() + 2 * PADDING + 2;
    let top = match options.position {
        Position::TopLeft | Position::TopCenter | Position::TopRight => MARGIN,
        Position::BottomLeft | Position::BottomCenter | Position::BottomRight => {
            rows.saturating_sub(MARGIN + BOX_ROWS)
        }
    };
    let left = match options.position {
        Position::TopLeft | Position::BottomLeft => MARGIN,
        Position::TopCenter | Position::BottomCenter => columns.saturating_sub(width) / 2,
        Position::TopRight | Position::BottomRight => columns.saturating_sub(MARGIN + width),
    };

    let alpha = shown.alpha.clamp(0.0, 1.0);
    let from = options.fade_from();
    let border = blend(from, options.border, alpha);
    let text = blend(from, options.text, alpha);
    let accent = blend(from, options.accent, alpha);
    let background = blend(from, options.background, alpha);
    let border_cell = |ch: char| OverlayCell {
        ch,
        fg: border,
        bg: None,
        bold: false,
    };
    let inside = width.saturating_sub(2);
    let edge = |left: char, right: char| {
        let mut row = vec![border_cell(left)];
        row.extend((0..inside).map(|_| border_cell('\u{2500}')));
        row.push(border_cell(right));
        row
    };
    let blank = || OverlayCell {
        ch: ' ',
        fg: text,
        bg: Some(background),
        bold: false,
    };
    let mut middle = vec![border_cell('\u{2502}')];
    middle.extend((0..PADDING).map(|_| blank()));
    middle.extend(label.iter().map(|(ch, style)| match style {
        Style::Text => OverlayCell {
            ch: *ch,
            fg: text,
            bg: Some(background),
            bold: false,
        },
        Style::Accent => OverlayCell {
            ch: *ch,
            fg: accent,
            bg: Some(background),
            bold: true,
        },
    }));
    middle.extend((0..PADDING).map(|_| blank()));
    middle.push(border_cell('\u{2502}'));
    Some(Overlay {
        top,
        left,
        rows: vec![
            edge('\u{256d}', '\u{256e}'),
            middle,
            edge('\u{2570}', '\u{256f}'),
        ],
    })
}

/// The SGR sequence that sets the style of a cell: it resets the attributes,
/// then sets each text flag of `cell` and the sources of its two colors.
fn sgr(cell: &Cell) -> String {
    let flags: String = [
        (cell.bold, "1;"),
        (cell.dim, "2;"),
        (cell.italic, "3;"),
        (cell.underline, "4;"),
        (cell.blink, "5;"),
        (cell.reverse, "7;"),
        (cell.hidden, "8;"),
        (cell.strikethrough, "9;"),
    ]
    .into_iter()
    .filter_map(|(on, code)| on.then_some(code))
    .collect();
    format!(
        "\x1b[0;{flags}{};{}m",
        fg_code(cell.fg_source),
        bg_code(cell.bg_source)
    )
}

/// Whether the viewer shows the cells `a` and `b` the same, for each part of a
/// cell that `sgr` writes: the character, the source of each color, and each
/// text flag. The viewer draws a color from its source in its own theme, thus
/// the RGB value of a replay does not count. That value can also be stale:
/// SGR 39 and SGR 49 change only the source.
fn same_look(a: &Cell, b: &Cell) -> bool {
    // A destructure, so that a new field of `Cell` fails to compile until this
    // comparison names it.
    let Cell {
        ch,
        fg_color: _,
        bg_color: _,
        fg_source,
        bg_source,
        bold,
        italic,
        underline,
        dim,
        blink,
        reverse,
        hidden,
        strikethrough,
    } = a;
    *ch == b.ch
        && *fg_source == b.fg_source
        && *bg_source == b.bg_source
        && *bold == b.bold
        && *italic == b.italic
        && *underline == b.underline
        && *dim == b.dim
        && *blink == b.blink
        && *reverse == b.reverse
        && *hidden == b.hidden
        && *strikethrough == b.strikethrough
}

/// The sequence that moves the cursor to `row` and `column`, counted from zero.
fn move_to(row: usize, column: usize) -> String {
    format!("\x1b[{};{}H", row + 1, column + 1)
}

/// The state of a screen that the HUD can disturb: the cursor, the current
/// attributes and the character set.
#[derive(PartialEq)]
struct Pen {
    /// The cursor, as column and row.
    cursor: (usize, usize),
    /// The source of the current foreground.
    fg: ColorSource,
    /// The source of the current background.
    bg: ColorSource,
    /// Whether the text is bold.
    bold: bool,
    /// Whether the text is italic.
    italic: bool,
    /// Whether the text is underlined.
    underline: bool,
    /// Whether the text is dim.
    dim: bool,
    /// Whether the text blinks.
    blink: bool,
    /// Whether the text is in reverse video.
    reverse: bool,
    /// Whether the text is hidden.
    hidden: bool,
    /// Whether the text is struck through.
    strikethrough: bool,
    /// Whether the DEC line drawing set is active.
    use_acs: bool,
}

impl Pen {
    /// The pen of `screen`.
    fn of(screen: &TerminalState) -> Self {
        Self {
            cursor: screen.get_cursor_position(),
            fg: screen.current_fg_source,
            bg: screen.current_bg_source,
            bold: screen.bold,
            italic: screen.italic,
            underline: screen.underline,
            dim: screen.dim,
            blink: screen.blink,
            reverse: screen.reverse,
            hidden: screen.hidden,
            strikethrough: screen.strikethrough,
            use_acs: screen.uses_line_drawing(),
        }
    }

    /// The sequences that put a screen in the pen `from` into this state. The
    /// character set is selected only when it differs from the one of `from`,
    /// so that a repair of the cursor writes no set of the replay.
    fn restore(&self, from: &Self) -> String {
        let cell = Cell {
            fg_source: self.fg,
            bg_source: self.bg,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            dim: self.dim,
            blink: self.blink,
            reverse: self.reverse,
            hidden: self.hidden,
            strikethrough: self.strikethrough,
            ..Cell::default()
        };
        let charset = match (self.use_acs, from.use_acs) {
            (true, false) => SELECT_LINE_DRAWING,
            (false, true) => SELECT_ASCII,
            _ => "",
        };
        format!(
            "{}{}{charset}",
            move_to(self.cursor.1, self.cursor.0),
            sgr(&cell)
        )
    }
}

/// Whether the cell `clean` at (`row`, `column`) is one half of a wide
/// character, and the box of `overlay` covers its other half.
fn cuts_wide_character(overlay: Option<&Overlay>, clean: &Cell, row: usize, column: usize) -> bool {
    let covered = |column: usize| overlay.is_some_and(|o| o.cell_at(row, column).is_some());
    if clean.is_wide_continuation() {
        column.checked_sub(1).is_some_and(covered)
    } else {
        covered(column + 1) && UnicodeWidthChar::width(clean.ch) == Some(2)
    }
}

/// Two replays of the recording: the screen as the recording draws it, and
/// the screen as the viewer sees it with the HUD.
pub(crate) struct Painter {
    /// The screen as the recording draws it.
    clean: TerminalState,
    /// The screen as the viewer sees it.
    shown: TerminalState,
    /// The restore count of `clean` at the last paint that found the two
    /// replays the same, or `None`. A paint writes only while both parsers are
    /// settled. While no restore runs, the two replays stay the same until the
    /// next box.
    synced: Option<u64>,
    /// The number of times that `paint` scanned the grid.
    #[cfg(test)]
    scans: usize,
}

impl Painter {
    /// A painter for a screen of `size`, or `None` when `fits(size)` is false.
    pub(crate) fn new(size: TerminalSize) -> Option<Self> {
        if !fits(size) {
            return None;
        }
        let width = usize::from(size.width);
        let height = usize::from(size.height);
        Some(Self {
            clean: TerminalState::new(width, height, TerminalTheme::auto()),
            shown: TerminalState::new(width, height, TerminalTheme::auto()),
            synced: Some(0),
            #[cfg(test)]
            scans: 0,
        })
    }

    /// Feed output of the recording to both screens.
    pub(crate) fn feed(&mut self, data: &str) {
        self.clean.process_output(data).ok();
        self.shown.process_output(data).ok();
    }

    /// The output that makes the viewer see the screen of the recording with
    /// `overlay` on top, or `None` when the viewer sees that already. The
    /// painter feeds this output to its own copy of the screen of the viewer.
    /// A paint with no box writes nothing and scans nothing while the replays
    /// agree: no restore of a save slot has run since they last agreed.
    ///
    /// A paint writes nothing while a replay holds an unfinished sequence of
    /// the program. The first `ESC` of the paint would end that sequence in the
    /// viewer, and the rest of the sequence would print as text. The next paint
    /// after the end of the sequence writes what is owed.
    pub(crate) fn paint(&mut self, overlay: Option<&Overlay>) -> Option<String> {
        if !(self.clean.is_settled() && self.shown.is_settled()) {
            return None;
        }
        self.write(overlay, "")
    }

    /// The last output of the painter, for the end of the region. It takes the
    /// box away, also while the program holds an unfinished sequence, because
    /// the filter cannot write after its region. It then writes the start of
    /// that sequence again, so the first output of the program after the
    /// region finishes the sequence in the viewer. It returns `None` when the
    /// viewer needs nothing.
    pub(crate) fn finish(&mut self) -> Option<String> {
        let reopen = self.clean.unfinished_sequence();
        self.write(None, &reopen)
    }

    /// The output that makes the viewer see the screen of the recording with
    /// `overlay` on top, or `None` when the viewer sees that already. When the
    /// output is not empty, `reopen` goes after it. The painter feeds the
    /// output to its own copy of the screen of the viewer.
    fn write(&mut self, overlay: Option<&Overlay>, reopen: &str) -> Option<String> {
        if overlay.is_none() && self.synced == Some(self.clean.restore_count()) {
            return None;
        }
        #[cfg(test)]
        {
            self.scans += 1;
        }
        let mut body = String::new();
        let mut any_cell = false;
        // The position and the style of the cell that was written last.
        let mut last: Option<(usize, usize, String)> = None;
        for (row, (shown_row, clean_row)) in self
            .shown
            .get_grid()
            .iter()
            .zip(self.clean.get_grid())
            .enumerate()
        {
            for (column, (seen, clean)) in shown_row.iter().zip(clean_row).enumerate() {
                let over = overlay.and_then(|o| o.cell_at(row, column));
                let wanted = match over {
                    Some(cell) => Cell {
                        ch: cell.ch,
                        fg_source: rgb_source(cell.fg),
                        bg_source: cell.bg.map_or(clean.bg_source, rgb_source),
                        bold: cell.bold,
                        ..Cell::default()
                    },
                    // A terminal cannot show one half of a wide character, thus
                    // the half that the box does not cover shows blank.
                    None if cuts_wide_character(overlay, clean, row, column) => Cell {
                        ch: ' ',
                        ..clean.clone()
                    },
                    None => clean.clone(),
                };
                // Most cells do not change. Compare before the codes are built,
                // so that a cell that does not change costs no allocation.
                if same_look(seen, &wanted) {
                    continue;
                }
                if wanted.is_wide_continuation() {
                    // The write of the wide character before this cell covers
                    // it, and a terminal writes no byte for it.
                    if let Some(at) = last.as_mut().filter(|(last_row, last_column, _)| {
                        *last_row == row && *last_column + 1 == column
                    }) {
                        at.1 = column;
                    }
                    continue;
                }
                any_cell = true;
                let style = sgr(&wanted);
                let after_last = last.as_ref().is_some_and(|(last_row, last_column, _)| {
                    *last_row == row && *last_column + 1 == column
                });
                if !after_last {
                    body.push_str(&move_to(row, column));
                }
                if !last.as_ref().is_some_and(|(_, _, before)| *before == style) {
                    body.push_str(&style);
                }
                body.push(wanted.ch);
                last = Some((row, column, style));
            }
        }
        let (shown_pen, clean_pen) = (Pen::of(&self.shown), Pen::of(&self.clean));
        let repair = (shown_pen != clean_pen).then(|| clean_pen.restore(&shown_pen));
        self.synced = (overlay.is_none() && !any_cell && repair.is_none())
            .then(|| self.clean.restore_count());
        let mut output = String::new();
        if any_cell {
            output.push_str("\x1b7\x0f\x1b(B\x1b[?6l");
            output.push_str(&body);
            output.push_str("\x1b8");
        }
        if let Some(repair) = repair {
            output.push_str(&repair);
        }
        if output.is_empty() {
            return None;
        }
        output.push_str(reopen);
        self.shown.process_output(&output).ok();
        Some(output)
    }
}

#[cfg(test)]
mod tests;
