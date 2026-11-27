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
    /// Whether the character is dim.
    pub(super) dim: bool,
    /// Whether the character is blinking.
    pub(super) blink: bool,
    /// Whether the character is in reverse video.
    pub(super) reverse: bool,
    /// Whether the character is hidden.
    pub(super) hidden: bool,
    /// Whether the character is struck through.
    pub(super) strikethrough: bool,
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
            dim: false,
            blink: false,
            reverse: false,
            hidden: false,
            strikethrough: false,
        },
    };
}

/// The model of the output terminal.
#[derive(Debug)]
pub(super) struct Writer {
    width: usize,
    grid: Vec<Glyph>,
    /// The pen of the output terminal, or `None` when it is not known.
    pen: Option<Look>,
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
            pen: Some(Look::default()),
            cursor: Some((0, 0)),
            cursor_shown: true,
        }
    }

    /// A terminal that shows `cells`, in row order, `width` cells to a row.
    ///
    /// The cursor is shown or hidden as `cursor_shown` says. The pen and the
    /// place of the cursor are unknown, thus the first write gets a full SGR
    /// and a CUP. The output terminal can hold an attribute that the emulator
    /// does not model. The full SGR starts with `CSI 0`, which removes it.
    pub(super) fn from_screen(width: usize, cells: Vec<Glyph>, cursor_shown: bool) -> Self {
        Self {
            width,
            grid: cells,
            pen: None,
            cursor: None,
            cursor_shown,
        }
    }

    /// The bytes that bring the model to `target` and `cursor`.
    ///
    /// `target` holds the cells in row order. `cursor` is the place of the
    /// cursor as `(column, row)`, or `None` when the cursor is hidden. The
    /// paint writes only the cells that differ from the model. A cell that
    /// the cursor is not on gets a CUP, and a cell with a new look gets one
    /// SGR. The paint then updates the model. It is `paint_cells`, then the
    /// cursor step.
    pub(super) fn paint(&mut self, target: &[Glyph], cursor: Option<(usize, usize)>) -> String {
        let mut bytes = String::new();
        self.paint_cells(target, &mut bytes);
        self.paint_cursor(cursor, &mut bytes);
        bytes
    }

    /// Write to `bytes` the cells that differ between the model and `target`,
    /// and update the model. `target` holds the cells in row order.
    ///
    /// It calls `paint_row` for each row of `target`.
    pub(super) fn paint_cells(&mut self, target: &[Glyph], bytes: &mut String) {
        let width = self.width.max(1);
        for (y, row) in target.chunks(width).enumerate() {
            self.paint_row(y, row, bytes);
        }
    }

    /// Write to `bytes` the cells of row `y` that differ between the model and
    /// `row`, and update the model. `row` holds the cells of that one row.
    ///
    /// A wide character covers two cells: the glyph of the character and a
    /// continuation glyph after it. The paint first turns each half that has
    /// no other half into a space, because a real terminal shows no such half.
    /// It writes a wide character with its two cells together, and it writes
    /// nothing for the continuation glyph.
    pub(super) fn paint_row(&mut self, y: usize, row: &[Glyph], bytes: &mut String) {
        let width = self.width.max(1);
        let base = y.saturating_mul(width);
        let want = normalized(row, width);
        let mut index = 0;
        while index < want.len() {
            let Some(glyph) = want.get(index).copied() else {
                break;
            };
            let is_pair = is_wide(&glyph)
                && want
                    .get(index + 1)
                    .is_some_and(|next| next.ch == termstate::WIDE_CONTINUATION);
            let cells = if is_pair { 2 } else { 1 };
            let differs = (0..cells)
                .any(|offset| self.grid.get(base + index + offset) != want.get(index + offset));
            if differs {
                let x = index;
                if self.cursor != Some((x, y)) {
                    push_cup(bytes, x, y);
                }
                if self.pen != Some(glyph.look) {
                    bytes.push_str(&sgr(&glyph.look));
                    self.pen = Some(glyph.look);
                }
                bytes.push(glyph.ch);
                for offset in 0..cells {
                    if let (Some(slot), Some(value)) = (
                        self.grid.get_mut(base + index + offset),
                        want.get(index + offset),
                    ) {
                        *slot = *value;
                    }
                }
                let one_wide = UnicodeWidthChar::width(glyph.ch) == Some(1);
                self.cursor = (one_wide && x + 1 < width).then_some((x + 1, y));
            }
            index += cells;
        }
    }

    /// Write to `bytes` the cell at column `x` and row `y` again, with its look.
    ///
    /// The write gives the real terminal a pending wrap when the cell is in
    /// the last column. The pen goes back to the pen before the write. The
    /// model does not know the place of the cursor after it.
    pub(super) fn rewrite(&mut self, x: usize, y: usize, glyph: Glyph, bytes: &mut String) {
        push_cup(bytes, x, y);
        let pen = self.pen;
        if pen != Some(glyph.look) {
            bytes.push_str(&sgr(&glyph.look));
        }
        bytes.push(glyph.ch);
        match pen {
            Some(pen) if pen != glyph.look => bytes.push_str(&sgr(&pen)),
            Some(_) => {}
            None => self.pen = Some(glyph.look),
        }
        self.cursor = None;
    }

    /// Write to `bytes` the sequences that bring the cursor to `cursor`, a
    /// `(column, row)` place or `None` for a hidden cursor, and update the
    /// model.
    pub(super) fn paint_cursor(&mut self, cursor: Option<(usize, usize)>, bytes: &mut String) {
        match cursor {
            Some((x, y)) => {
                if self.cursor != Some((x, y)) {
                    push_cup(bytes, x, y);
                    self.cursor = Some((x, y));
                }
                if !self.cursor_shown {
                    bytes.push_str(SHOW_CURSOR);
                    self.cursor_shown = true;
                }
            }
            None => {
                if self.cursor_shown {
                    bytes.push_str(HIDE_CURSOR);
                    self.cursor_shown = false;
                }
            }
        }
    }

    /// Set the pen of the model to `look`, for a caller that wrote its SGR.
    pub(super) fn set_pen(&mut self, look: Look) {
        self.pen = Some(look);
    }
}

/// The sequence that shows the cursor.
pub(super) const SHOW_CURSOR: &str = "\x1b[?25h";
/// The sequence that hides the cursor.
pub(super) const HIDE_CURSOR: &str = "\x1b[?25l";

/// Write the CUP to the cell at column `x` and row `y`, counted from 0.
fn push_cup(bytes: &mut String, x: usize, y: usize) {
    bytes.push_str(&cup(x, y));
}

/// The CUP to the cell at column `x` and row `y`, counted from 0.
pub(super) fn cup(x: usize, y: usize) -> String {
    format!("\x1b[{};{}H", y.saturating_add(1), x.saturating_add(1))
}

/// The SGR that sets exactly `look`: it resets the pen first.
pub(super) fn sgr(look: &Look) -> String {
    let mut bytes = String::from("\x1b[0");
    if look.bold {
        bytes.push_str(";1");
    }
    if look.dim {
        bytes.push_str(";2");
    }
    if look.italic {
        bytes.push_str(";3");
    }
    if look.underline {
        bytes.push_str(";4");
    }
    if look.blink {
        bytes.push_str(";5");
    }
    if look.reverse {
        bytes.push_str(";7");
    }
    if look.hidden {
        bytes.push_str(";8");
    }
    if look.strikethrough {
        bytes.push_str(";9");
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

/// Tells if `glyph` is a character of display width 2.
fn is_wide(glyph: &Glyph) -> bool {
    UnicodeWidthChar::width(glyph.ch) == Some(2)
}

/// The glyphs of `target` as a real terminal shows them, `width` glyphs to a
/// row. A wide character that no continuation glyph follows in its row becomes
/// a space with its look. A continuation glyph that does not follow a wide
/// character becomes a space with its look.
fn normalized(target: &[Glyph], width: usize) -> Vec<Glyph> {
    let space = |glyph: &Glyph| Glyph {
        ch: ' ',
        look: glyph.look,
    };
    target
        .iter()
        .enumerate()
        .map(|(index, glyph)| {
            let in_row = |other: usize| other / width == index / width;
            let is_continuation = |other: Option<&Glyph>| {
                other.is_some_and(|other| other.ch == termstate::WIDE_CONTINUATION)
            };
            if is_wide(glyph) {
                let next = index.checked_add(1).filter(|next| in_row(*next));
                if is_continuation(next.and_then(|next| target.get(next))) {
                    *glyph
                } else {
                    space(glyph)
                }
            } else if glyph.ch == termstate::WIDE_CONTINUATION {
                let previous = index.checked_sub(1).filter(|previous| in_row(*previous));
                if previous
                    .and_then(|previous| target.get(previous))
                    .is_some_and(is_wide)
                {
                    *glyph
                } else {
                    space(glyph)
                }
            } else {
                *glyph
            }
        })
        .collect()
}
