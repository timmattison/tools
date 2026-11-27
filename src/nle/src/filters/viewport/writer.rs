//! The writer of the viewport filter: a model of the output terminal and the
//! escape sequences that bring it to a target screen.

use termstate::ColorSource;

/// How a cell looks: its two color sources and its attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Look {
    /// The source of the foreground color.
    pub(super) fg: ColorSource,
    /// The source of the background color.
    pub(super) bg: ColorSource,
    /// Whether the character is bold.
    pub(super) bold: bool,
    /// Whether the character is italic.
    pub(super) italic: bool,
    /// Whether the character is underlined.
    pub(super) underline: bool,
}

/// One cell of a screen: a character and its look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Glyph {
    /// The character of the cell.
    pub(super) ch: char,
    /// How the cell looks.
    pub(super) look: Look,
}

impl Glyph {
    /// The blank glyph: a space with the default colors and no attribute.
    pub(super) const BLANK: Glyph = Glyph {
        ch: ' ',
        look: Look {
            fg: ColorSource::Default,
            bg: ColorSource::Default,
            bold: false,
            italic: false,
            underline: false,
        },
    };
}

/// The model of the output terminal.
#[derive(Debug)]
pub(super) struct Writer {
    width: usize,
    grid: Vec<Glyph>,
    pen: Look,
    cursor: Option<(usize, usize)>,
    cursor_shown: bool,
}

impl Writer {
    /// A new terminal of `width` by `height` cells: blank, with the default
    /// pen, and the cursor at the top left and shown.
    pub(super) fn blank(width: usize, height: usize) -> Self {
        Self {
            width,
            grid: vec![Glyph::BLANK; width.saturating_mul(height)],
            pen: Look::default(),
            cursor: Some((0, 0)),
            cursor_shown: true,
        }
    }

    /// The bytes that bring the model to `target` and `cursor`.
    pub(super) fn paint(&mut self, target: &[Glyph], cursor: Option<(usize, usize)>) -> String {
        let _ = (target, cursor, self.width);
        String::new()
    }
}
