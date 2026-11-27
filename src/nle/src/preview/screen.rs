//! The screen of the preview as rows of styled runs of text.
//!
//! The emulator gives each character one cell, also a wide character such as
//! `日` and a zero-width mark. The UI draws a run with `single_cell` set in a
//! box of one cell width, so the columns stay aligned.

use std::collections::HashMap;

use unicode_width::UnicodeWidthChar;

/// The CSS property of the text color.
const CSS_COLOR: &str = "color";
/// The CSS property of the color behind the text.
const CSS_BACKGROUND_COLOR: &str = "background-color";
/// The CSS declaration of bold text.
const CSS_BOLD: &str = "font-weight:bold";
/// The CSS declaration of italic text.
const CSS_ITALIC: &str = "font-style:italic";
/// The CSS declaration of underlined text.
const CSS_UNDERLINE: &str = "text-decoration:underline";

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
        let mut css = format!(
            "{CSS_COLOR}:{};{CSS_BACKGROUND_COLOR}:{}",
            hex(self.foreground),
            hex(self.background)
        );
        let flags = [
            (self.bold, CSS_BOLD),
            (self.italic, CSS_ITALIC),
            (self.underline, CSS_UNDERLINE),
        ];
        for declaration in flags
            .iter()
            .filter(|(is_set, _)| *is_set)
            .map(|(_, declaration)| declaration)
        {
            css.push(';');
            css.push_str(declaration);
        }
        css
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
        Self::of_with(&mut Colors::new(state))
    }

    /// The screen of the state of `colors`. `colors` resolves the cell colors.
    fn of_with(colors: &mut Colors<'_>) -> Self {
        let state = colors.state;
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
                    runs: runs_of(colors, cells, cursor_column),
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
            background: colors.of(&blank).1,
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

/// The two colors of a cell: the foreground and the background.
type ColorPair = (Rgb, Rgb);

/// The colors that one terminal draws its cells with. `resolve_cell_colors`
/// walks the 256 entries of the palette at each call, but its answer depends
/// only on the two colors of the cell, and a screen holds few pairs. Thus
/// each pair resolves one time.
struct Colors<'a> {
    state: &'a termstate::TerminalState,
    /// The last pair and its answer. Cells next to each other mostly share them.
    last: Option<(ColorPair, ColorPair)>,
    /// Each pair that resolved, and its answer.
    resolved: HashMap<ColorPair, ColorPair>,
    /// The count of calls of `resolve_cell_colors`.
    #[cfg(test)]
    resolutions: usize,
}

impl<'a> Colors<'a> {
    /// The colors of the cells of `state`.
    fn new(state: &'a termstate::TerminalState) -> Self {
        Self {
            state,
            last: None,
            resolved: HashMap::new(),
            #[cfg(test)]
            resolutions: 0,
        }
    }

    /// The foreground and the background that `cell` draws with.
    fn of(&mut self, cell: &termstate::Cell) -> ColorPair {
        let pair = (cell.fg_color, cell.bg_color);
        if let Some((last_pair, answer)) = self.last {
            if last_pair == pair {
                return answer;
            }
        }
        let answer = match self.resolved.get(&pair) {
            Some(&answer) => answer,
            None => {
                #[cfg(test)]
                {
                    self.resolutions += 1;
                }
                let answer = self.state.resolve_cell_colors(cell);
                self.resolved.insert(pair, answer);
                answer
            }
        };
        self.last = Some((pair, answer));
        answer
    }

    /// The count of calls of `resolve_cell_colors` so far.
    #[cfg(test)]
    fn resolutions(&self) -> usize {
        self.resolutions
    }
}

/// The style of one cell.
fn style_of(colors: &mut Colors<'_>, cell: &termstate::Cell) -> Style {
    let (foreground, background) = colors.of(cell);
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
    colors: &mut Colors<'_>,
    cells: &[termstate::Cell],
    cursor_column: Option<usize>,
) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (column, cell) in cells.iter().enumerate() {
        let mut style = style_of(colors, cell);
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
pub fn hex((red, green, blue): Rgb) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

#[cfg(test)]
mod tests;
