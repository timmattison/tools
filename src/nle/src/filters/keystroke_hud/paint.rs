//! The painter of the keystroke HUD. It keeps two replays of the recording:
//! the screen as the recording draws it, and the screen as the viewer sees it
//! with the HUD. The difference between the two replays is the output that the
//! filter must write.
//!
//! Both replays use a private theme. Its 18 colors are unique, and none is in
//! the 256-color table of `termstate`. Thus the painter maps the color of a
//! cell back to the SGR code that set it, and a repaint of a cell is correct
//! in each theme of the export. A program color of exactly one of the 18 values
//! maps to its theme code. This is the cost of the method.

use super::options::{HudOptions, Position, Rgb};
use super::timeline::{Piece, Shown};
use crate::types::TerminalSize;
use termstate::{Cell, TerminalState, TerminalTheme};

/// The cells between the box and the edge of the screen.
const MARGIN: usize = 1;
/// The blank cells between the border and the label, on each side.
const PADDING: usize = 1;
/// The rows of the box: the top border, the label, the bottom border.
const BOX_ROWS: usize = 3;

/// The number of base colors in a theme.
const BASE_COLORS: u8 = 16;

/// A color as three channels.
type Color = (u8, u8, u8);

/// The replay color of the default foreground.
const REPLAY_FOREGROUND: Color = (1, 1, 1);
/// The replay color of the default background.
const REPLAY_BACKGROUND: Color = (1, 1, 2);

/// The sequence that selects the DEC line drawing character set.
const SELECT_LINE_DRAWING: &str = "\x1b(0";
/// The sequence that selects the ASCII character set.
const SELECT_ASCII: &str = "\x1b(B";

/// The replay color of the base color `index`, from 0 to 15.
fn replay_base(index: u8) -> Color {
    (1, 1, index.saturating_add(3))
}

/// The private theme of the two replays.
fn replay_theme() -> TerminalTheme {
    TerminalTheme {
        foreground: REPLAY_FOREGROUND,
        background: REPLAY_BACKGROUND,
        black: replay_base(0),
        red: replay_base(1),
        green: replay_base(2),
        yellow: replay_base(3),
        blue: replay_base(4),
        magenta: replay_base(5),
        cyan: replay_base(6),
        white: replay_base(7),
        bright_black: replay_base(8),
        bright_red: replay_base(9),
        bright_green: replay_base(10),
        bright_yellow: replay_base(11),
        bright_blue: replay_base(12),
        bright_magenta: replay_base(13),
        bright_cyan: replay_base(14),
        bright_white: replay_base(15),
    }
}

/// The index of the base color that has the replay color `color`, if any.
fn base_index(color: Color) -> Option<u8> {
    let (red, green, blue) = color;
    if red != 1 || green != 1 {
        return None;
    }
    blue.checked_sub(3).filter(|index| *index < BASE_COLORS)
}

/// The SGR code that sets the foreground `color` of a replay cell.
fn fg_code(color: Color) -> String {
    if color == REPLAY_FOREGROUND {
        return "39".to_string();
    }
    match base_index(color) {
        Some(index) if index < 8 => format!("{}", 30 + u32::from(index)),
        Some(index) => format!("{}", 90 + u32::from(index) - 8),
        None => format!("38;2;{};{};{}", color.0, color.1, color.2),
    }
}

/// The SGR code that sets the background `color` of a replay cell.
fn bg_code(color: Color) -> String {
    if color == REPLAY_BACKGROUND {
        return "49".to_string();
    }
    match base_index(color) {
        Some(index) if index < 8 => format!("{}", 40 + u32::from(index)),
        Some(index) => format!("{}", 100 + u32::from(index) - 8),
        None => format!("48;2;{};{};{}", color.0, color.1, color.2),
    }
}

/// Whether the screen of `size` has room for a box with a label of one cell.
pub(crate) fn fits(size: TerminalSize) -> bool {
    usize::from(size.columns) >= 2 * MARGIN + 2 * PADDING + 3
        && usize::from(size.rows) >= 2 * MARGIN + BOX_ROWS
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
            Piece::Text(text) => cells.extend(text.chars().map(|ch| (ch, Style::Text))),
            Piece::Special { label, count } => {
                cells.extend(label.chars().map(|ch| (ch, Style::Accent)));
                if *count > 1 {
                    cells.extend(
                        format!(" \u{d7}{count}")
                            .chars()
                            .map(|ch| (ch, Style::Accent)),
                    );
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
    let columns = usize::from(size.columns);
    let rows = usize::from(size.rows);
    let mut label = label_cells(shown);
    let most = columns.saturating_sub(2 * MARGIN + 2 * PADDING + 2);
    if label.len() > most {
        let keep = most.saturating_sub(1);
        let mut kept = label.split_off(label.len().saturating_sub(keep));
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
/// then sets the ones of `cell` and the colors `fg` and `bg`.
fn sgr(cell: &Cell, fg: &str, bg: &str) -> String {
    let bold = if cell.bold { "1;" } else { "" };
    let italic = if cell.italic { "3;" } else { "" };
    let underline = if cell.underline { "4;" } else { "" };
    format!("\x1b[0;{bold}{italic}{underline}{fg};{bg}m")
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
    /// The current foreground.
    fg: Color,
    /// The current background.
    bg: Color,
    /// Whether the text is bold.
    bold: bool,
    /// Whether the text is italic.
    italic: bool,
    /// Whether the text is underlined.
    underline: bool,
    /// Whether the DEC line drawing set is active.
    use_acs: bool,
}

impl Pen {
    /// The pen of `screen`.
    fn of(screen: &TerminalState) -> Self {
        Self {
            cursor: screen.get_cursor_position(),
            fg: screen.current_fg,
            bg: screen.current_bg,
            bold: screen.bold,
            italic: screen.italic,
            underline: screen.underline,
            use_acs: screen.uses_line_drawing(),
        }
    }

    /// The sequences that put a screen in the pen `from` into this state. The
    /// character set is selected only when it differs from the one of `from`,
    /// so that a repair of the cursor writes no set of the replay.
    fn restore(&self, from: &Self) -> String {
        let cell = Cell {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
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
            sgr(&cell, &fg_code(self.fg), &bg_code(self.bg))
        )
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
        let width = usize::from(size.columns);
        let height = usize::from(size.rows);
        Some(Self {
            clean: TerminalState::new(width, height, replay_theme()),
            shown: TerminalState::new(width, height, replay_theme()),
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
                        fg_color: cell.fg,
                        bg_color: cell.bg.unwrap_or(clean.bg_color),
                        bold: cell.bold,
                        italic: false,
                        underline: false,
                    },
                    None => clean.clone(),
                };
                // Most cells do not change. Compare before the codes are built,
                // so that a cell that does not change costs no allocation.
                if *seen == wanted {
                    continue;
                }
                let (fg, bg) = match over {
                    Some(cell) => (
                        format!("38;2;{};{};{}", cell.fg.0, cell.fg.1, cell.fg.2),
                        match cell.bg {
                            Some(color) => format!("48;2;{};{};{}", color.0, color.1, color.2),
                            None => bg_code(clean.bg_color),
                        },
                    ),
                    None => (fg_code(clean.fg_color), bg_code(clean.bg_color)),
                };
                any_cell = true;
                let style = sgr(&wanted, &fg, &bg);
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
