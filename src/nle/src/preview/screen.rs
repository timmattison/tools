//! The screen of the preview as rows of styled runs of text.
//!
//! The emulator gives each character one cell, also a wide character such as
//! `日` and a zero-width mark. The UI draws a run with `single_cell` set in a
//! box of one cell width, so the columns stay aligned.

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
    pub fn of(_state: &termstate::TerminalState) -> Self {
        Self {
            columns: 0,
            rows: Vec::new(),
            background: (0, 0, 0),
        }
    }

    /// The text of each row: the texts of its runs, joined.
    pub fn row_texts(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The CSS hex form of a color: `#rrggbb`, lowercase.
pub fn hex(_color: Rgb) -> String {
    String::new()
}

#[cfg(test)]
mod tests;
