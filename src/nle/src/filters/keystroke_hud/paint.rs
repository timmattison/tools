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

use super::options::HudOptions;
use super::timeline::Shown;
use crate::types::TerminalSize;
use termstate::{Cell, TerminalState, TerminalTheme};

/// The cells between the box and the edge of the screen.
const MARGIN: usize = 1;
/// The blank cells between the border and the label, on each side.
const PADDING: usize = 1;
/// The rows of the box: the top border, the label, the bottom border.
const BOX_ROWS: usize = 3;

/// A color as three channels.
type Color = (u8, u8, u8);

/// The replay color of the default foreground.
const REPLAY_FOREGROUND: Color = (1, 1, 1);
/// The replay color of the default background.
const REPLAY_BACKGROUND: Color = (1, 1, 2);

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

/// The SGR code that sets the foreground `color` of a replay cell.
fn fg_code(color: Color) -> String {
    let _ = color;
    String::new()
}

/// The SGR code that sets the background `color` of a replay cell.
fn bg_code(color: Color) -> String {
    let _ = color;
    String::new()
}

/// Whether the screen of `size` has room for a box with a label of one cell.
pub(crate) fn fits(size: TerminalSize) -> bool {
    let _ = size;
    true
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

/// The box that shows `shown`, or `None` when the screen of `size` has no room.
pub(crate) fn layout(size: TerminalSize, options: &HudOptions, shown: &Shown) -> Option<Overlay> {
    let _ = (size, options, shown);
    Some(Overlay {
        top: 0,
        left: 0,
        rows: Vec::new(),
    })
}

/// Two replays of the recording: the screen as the recording draws it, and
/// the screen as the viewer sees it with the HUD.
pub(crate) struct Painter {
    /// The screen as the recording draws it.
    clean: TerminalState,
    /// The screen as the viewer sees it.
    shown: TerminalState,
}

impl Painter {
    /// A painter for a screen of `size`, or `None` when `fits(size)` is false.
    pub(crate) fn new(size: TerminalSize) -> Option<Self> {
        let width = usize::from(size.columns);
        let height = usize::from(size.rows);
        Some(Self {
            clean: TerminalState::new(width, height, replay_theme()),
            shown: TerminalState::new(width, height, replay_theme()),
        })
    }

    /// Feed output of the recording to both screens.
    pub(crate) fn feed(&mut self, data: &str) {
        self.clean.process_output(data).ok();
        self.shown.process_output(data).ok();
    }

    /// The output that makes the viewer see the screen of the recording with
    /// `overlay` on top, or `None` when the viewer sees that already.
    pub(crate) fn paint(&mut self, overlay: Option<&Overlay>) -> Option<String> {
        let _ = (overlay, Cell::default());
        None
    }
}

#[cfg(test)]
mod tests;
