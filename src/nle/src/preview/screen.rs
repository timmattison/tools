//! The screen of the preview as rows of styled runs of text.
//!
//! The emulator gives each character one cell, also a wide character such as
//! `日` and a zero-width mark. The UI draws a run with `single_cell` set in a
//! box of one cell width, so the columns stay aligned.

use unicode_width::UnicodeWidthChar;

/// A color as red, green, and blue.
pub type Rgb = (u8, u8, u8);

/// The look of one run of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// The color of the text.
    pub foreground: Rgb,
    /// The color behind the text.
    pub background: Rgb,
    /// Whether the text is bold.
    pub bold: bool,
    /// Whether the text is italic.
    pub italic: bool,
    /// Whether the text is underlined.
    pub underline: bool,
}

impl Style {
    /// The CSS declarations of this style.
    pub fn css(&self) -> String {
        String::new()
    }
}

/// Cells of one row next to each other that have one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The characters of the run.
    pub text: String,
    /// The look of the run.
    pub style: Style,
    /// Whether the run is one character that the UI draws in a box of one cell.
    pub single_cell: bool,
}

/// One row of the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The runs of the row, from left to right.
    pub runs: Vec<Run>,
}

/// The screen of a terminal, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// The number of columns.
    pub columns: usize,
    /// The rows, from top to bottom.
    pub rows: Vec<Row>,
    /// The color of a blank cell.
    pub background: Rgb,
}

impl Screen {
    /// The screen of `state`.
    pub fn of(state: &termstate::TerminalState) -> Self {
        let cursor = state
            .is_cursor_visible()
            .then(|| state.get_cursor_position());
        let rows = state
            .get_grid()
            .iter()
            .enumerate()
            .map(|(row_index, cells)| {
                let cursor_column = cursor
                    .filter(|&(_, cursor_row)| cursor_row == row_index)
                    .map(|(column, _)| column);
                Row {
                    runs: runs_of(state, cells, cursor_column),
                }
            })
            .collect();
        let theme = state.get_theme();
        let blank = termstate::Cell {
            ch: ' ',
            fg_color: theme.foreground,
            bg_color: theme.background,
            ..termstate::Cell::default()
        };
        Self {
            columns: state.width,
            rows,
            background: state.resolve_cell_colors(&blank).1,
        }
    }

    /// The text of each row: the texts of its runs, joined.
    pub fn row_texts(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.runs.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }
}

/// The style of one cell of `state`.
fn style_of(state: &termstate::TerminalState, cell: &termstate::Cell) -> Style {
    let (foreground, background) = state.resolve_cell_colors(cell);
    Style {
        foreground,
        background,
        bold: cell.bold,
        italic: cell.italic,
        underline: cell.underline,
    }
}

/// The runs of one row of cells. Cells next to each other with one style merge.
/// The cell at `cursor_column` swaps its foreground and its background.
fn runs_of(
    state: &termstate::TerminalState,
    cells: &[termstate::Cell],
    cursor_column: Option<usize>,
) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (column, cell) in cells.iter().enumerate() {
        let mut style = style_of(state, cell);
        if cursor_column == Some(column) {
            style = Style {
                foreground: style.background,
                background: style.foreground,
                ..style
            };
        }
        let ch = if cell.ch.is_control() { ' ' } else { cell.ch };
        let single_cell = ch.width() != Some(1);
        match runs.last_mut() {
            Some(run) if run.style == style && !run.single_cell && !single_cell => {
                run.text.push(ch);
            }
            _ => runs.push(Run {
                text: ch.to_string(),
                style,
                single_cell,
            }),
        }
    }
    runs
}

/// The CSS hex form of a color: `#rrggbb`, lowercase.
pub fn hex(_color: Rgb) -> String {
    String::new()
}

#[cfg(test)]
mod tests;
