//! The writer of the viewport filter: a model of the output terminal and the
//! escape sequences that bring it to a target screen.

use termstate::ColorSource;
use unicode_width::UnicodeWidthChar;

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
    ///
    /// `target` holds the cells in row order. `cursor` is the place of the
    /// cursor as `(column, row)`, or `None` when the cursor is hidden. The
    /// paint writes only the cells that differ from the model. A cell that
    /// the cursor is not on gets a CUP, and a cell with a new look gets one
    /// SGR. The paint then updates the model.
    pub(super) fn paint(&mut self, target: &[Glyph], cursor: Option<(usize, usize)>) -> String {
        let mut bytes = String::new();
        let width = self.width.max(1);
        for (index, (model, want)) in self.grid.iter_mut().zip(target).enumerate() {
            if model == want {
                continue;
            }
            let (x, y) = (index % width, index / width);
            if self.cursor != Some((x, y)) {
                push_cup(&mut bytes, x, y);
            }
            if self.pen != want.look {
                bytes.push_str(&sgr(&want.look));
                self.pen = want.look;
            }
            bytes.push(want.ch);
            *model = *want;
            let one_wide = UnicodeWidthChar::width(want.ch) == Some(1);
            self.cursor = (one_wide && x + 1 < width).then_some((x + 1, y));
        }
        match cursor {
            Some((x, y)) => {
                if self.cursor != Some((x, y)) {
                    push_cup(&mut bytes, x, y);
                    self.cursor = Some((x, y));
                }
                if !self.cursor_shown {
                    bytes.push_str(SHOW);
                    self.cursor_shown = true;
                }
            }
            None => {
                if self.cursor_shown {
                    bytes.push_str(HIDE);
                    self.cursor_shown = false;
                }
            }
        }
        bytes
    }
}

/// The sequence that shows the cursor.
const SHOW: &str = "\x1b[?25h";
/// The sequence that hides the cursor.
const HIDE: &str = "\x1b[?25l";

/// Write the CUP to the cell at column `x` and row `y`, counted from 0.
fn push_cup(bytes: &mut String, x: usize, y: usize) {
    bytes.push_str(&format!(
        "\x1b[{};{}H",
        y.saturating_add(1),
        x.saturating_add(1)
    ));
}

/// The SGR that sets exactly `look`: it resets the pen first.
fn sgr(look: &Look) -> String {
    let mut bytes = String::from("\x1b[0");
    if look.bold {
        bytes.push_str(";1");
    }
    if look.italic {
        bytes.push_str(";3");
    }
    if look.underline {
        bytes.push_str(";4");
    }
    push_color(&mut bytes, look.fg, Ground::Foreground);
    push_color(&mut bytes, look.bg, Ground::Background);
    bytes.push('m');
    bytes
}

/// Which color of a cell an SGR parameter sets.
#[derive(Debug, Clone, Copy)]
enum Ground {
    Foreground,
    Background,
}

/// Write the SGR parameters that set `source` as the color of `ground`. The
/// default color writes nothing, because the reset of the SGR sets it.
fn push_color(bytes: &mut String, source: ColorSource, ground: Ground) {
    // The first parameter of the base colors, of the bright colors, and of the
    // extended colors, for the foreground and for the background.
    let (base, bright, extended) = match ground {
        Ground::Foreground => (30, 90, 38),
        Ground::Background => (40, 100, 48),
    };
    match source {
        ColorSource::Default => {}
        ColorSource::Base(index) if index < 8 => {
            bytes.push_str(&format!(";{}", base + u16::from(index)));
        }
        ColorSource::Base(index) if index < 16 => {
            bytes.push_str(&format!(";{}", bright + u16::from(index - 8)));
        }
        ColorSource::Base(index) | ColorSource::Indexed(index) => {
            bytes.push_str(&format!(";{extended};5;{index}"));
        }
        ColorSource::Rgb(r, g, b) => {
            bytes.push_str(&format!(";{extended};2;{r};{g};{b}"));
        }
    }
}
