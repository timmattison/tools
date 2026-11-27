//! The VT100/ANSI terminal emulator of `beta`.
//!
//! This crate holds the emulator and the five theme palettes that `beta` uses
//! for its video and web exports. It is shared, so that `nle` uses the same
//! copy.
//!
//! A clone of a `TerminalState` is exact. It copies the screen, and it replays the
//! bytes of an unfinished escape sequence into its own parser.
//!
//! Two terminals are equal when each later byte has the same effect on both. The
//! backspace removal filter of `nle` uses this equality: it compares the screen
//! before a typed key with the screen after the key that erased it.
//!
//! It does not draw pixels. `beta` keeps the drawing, the fonts, the tmux
//! layout, and the ffmpeg pipe.

use anyhow::Result;
use vte::{Parser, Perform};

/// Narrows an SGR colour offset to the palette index it names.
///
/// Callers reach this only from a match arm that has already pinned the SGR
/// parameter to one of the eight-value ranges 30-37, 40-47, 90-97 or 100-107, so
/// the offset handed in is always 0..=15 and the narrowing is exact.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the caller's match arm pins the offset to 0..=15, so this narrowing is exact"
)]
const fn sgr_palette_index(offset: u16) -> u8 {
    offset as u8
}

/// Narrows a raw VT parameter to the byte an SGR colour operand needs.
///
/// VT parameters are unbounded 16-bit values, so a malformed sequence such as
/// `ESC[38;2;300;0;0m` can carry an operand wider than a byte. Wrapping it is
/// deliberate and long-standing: the renderer keeps painting with the wrapped
/// value rather than discarding the whole sequence.
#[expect(
    clippy::cast_possible_truncation,
    reason = "an out-of-range VT colour operand is deliberately wrapped into a byte, preserving the renderer's existing handling of malformed input"
)]
const fn sgr_color_byte(param: u16) -> u8 {
    param as u8
}

/// The colors of one terminal theme: the default background and foreground,
/// and the 16 base ANSI colors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTheme {
    /// The default background color.
    pub background: (u8, u8, u8),
    /// The default foreground color.
    pub foreground: (u8, u8, u8),
    // The 16 base ANSI colors, normal and bright.
    pub black: (u8, u8, u8),
    pub red: (u8, u8, u8),
    pub green: (u8, u8, u8),
    pub yellow: (u8, u8, u8),
    pub blue: (u8, u8, u8),
    pub magenta: (u8, u8, u8),
    pub cyan: (u8, u8, u8),
    pub white: (u8, u8, u8),
    pub bright_black: (u8, u8, u8),
    pub bright_red: (u8, u8, u8),
    pub bright_green: (u8, u8, u8),
    pub bright_yellow: (u8, u8, u8),
    pub bright_blue: (u8, u8, u8),
    pub bright_magenta: (u8, u8, u8),
    pub bright_cyan: (u8, u8, u8),
    pub bright_white: (u8, u8, u8),
}

impl TerminalTheme {
    /// The theme that `from_name` gives for an unknown name. It is `black`.
    pub fn auto() -> Self {
        Self::black()
    }

    /// A black background with white text.
    pub fn black() -> Self {
        Self {
            background: (0, 0, 0),
            foreground: (255, 255, 255),
            black: (0, 0, 0),
            red: (255, 85, 85),
            green: (85, 255, 85),
            yellow: (255, 255, 85),
            blue: (85, 85, 255),
            magenta: (255, 85, 255),
            cyan: (85, 255, 255),
            white: (255, 255, 255),
            bright_black: (85, 85, 85),
            bright_red: (255, 85, 85),
            bright_green: (85, 255, 85),
            bright_yellow: (255, 255, 85),
            bright_blue: (85, 85, 255),
            bright_magenta: (255, 85, 255),
            bright_cyan: (85, 255, 255),
            bright_white: (255, 255, 255),
        }
    }

    /// The Dracula palette.
    pub fn dracula() -> Self {
        Self {
            background: (40, 42, 54),
            foreground: (248, 248, 242),
            black: (40, 42, 54),
            red: (255, 85, 85),
            green: (80, 250, 123),
            yellow: (241, 250, 140),
            blue: (98, 114, 164),
            magenta: (255, 121, 198),
            cyan: (139, 233, 253),
            white: (248, 248, 242),
            bright_black: (98, 114, 164),
            bright_red: (255, 85, 85),
            bright_green: (80, 250, 123),
            bright_yellow: (241, 250, 140),
            bright_blue: (98, 114, 164),
            bright_magenta: (255, 121, 198),
            bright_cyan: (139, 233, 253),
            bright_white: (255, 255, 255),
        }
    }

    /// The Monokai palette.
    pub fn monokai() -> Self {
        Self {
            background: (39, 40, 34),
            foreground: (248, 248, 242),
            black: (39, 40, 34),
            red: (249, 38, 114),
            green: (166, 226, 46),
            yellow: (244, 191, 117),
            blue: (102, 217, 239),
            magenta: (174, 129, 255),
            cyan: (161, 239, 228),
            white: (248, 248, 242),
            bright_black: (117, 113, 94),
            bright_red: (249, 38, 114),
            bright_green: (166, 226, 46),
            bright_yellow: (244, 191, 117),
            bright_blue: (102, 217, 239),
            bright_magenta: (174, 129, 255),
            bright_cyan: (161, 239, 228),
            bright_white: (248, 248, 242),
        }
    }

    /// The Solarized Dark palette.
    pub fn solarized_dark() -> Self {
        Self {
            background: (0, 43, 54),
            foreground: (131, 148, 150),
            black: (7, 54, 66),
            red: (220, 50, 47),
            green: (133, 153, 0),
            yellow: (181, 137, 0),
            blue: (38, 139, 210),
            magenta: (211, 54, 130),
            cyan: (42, 161, 152),
            white: (238, 232, 213),
            bright_black: (0, 43, 54),
            bright_red: (203, 75, 22),
            bright_green: (88, 110, 117),
            bright_yellow: (101, 123, 131),
            bright_blue: (131, 148, 150),
            bright_magenta: (108, 113, 196),
            bright_cyan: (147, 161, 161),
            bright_white: (253, 246, 227),
        }
    }

    /// The Solarized Light palette.
    pub fn solarized_light() -> Self {
        Self {
            background: (253, 246, 227),
            foreground: (101, 123, 131),
            black: (7, 54, 66),
            red: (220, 50, 47),
            green: (133, 153, 0),
            yellow: (181, 137, 0),
            blue: (38, 139, 210),
            magenta: (211, 54, 130),
            cyan: (42, 161, 152),
            white: (238, 232, 213),
            bright_black: (0, 43, 54),
            bright_red: (203, 75, 22),
            bright_green: (88, 110, 117),
            bright_yellow: (101, 123, 131),
            bright_blue: (131, 148, 150),
            bright_magenta: (108, 113, 196),
            bright_cyan: (147, 161, 161),
            bright_white: (253, 246, 227),
        }
    }

    /// Returns the theme that has this name, or `auto` for an unknown name.
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "black" => Self::black(),
            "dracula" => Self::dracula(),
            "monokai" => Self::monokai(),
            "solarized-dark" => Self::solarized_dark(),
            "solarized-light" => Self::solarized_light(),
            _ => Self::auto(),
        }
    }

    /// Returns the RGB color of a palette index: the 16 base colors come from the
    /// theme, and the other 240 from the standard 256-color table.
    pub fn get_color(&self, index: u8) -> (u8, u8, u8) {
        match index {
            0 => self.black,
            1 => self.red,
            2 => self.green,
            3 => self.yellow,
            4 => self.blue,
            5 => self.magenta,
            6 => self.cyan,
            7 => self.white,
            8 => self.bright_black,
            9 => self.bright_red,
            10 => self.bright_green,
            11 => self.bright_yellow,
            12 => self.bright_blue,
            13 => self.bright_magenta,
            14 => self.bright_cyan,
            15 => self.bright_white,
            _ => self.foreground,
        }
    }

    /// Returns the RGB color of an index in the standard xterm 256-color table.
    pub fn get_256_color(index: u8) -> (u8, u8, u8) {
        match index {
            // Standard 16 colors (0-15)
            0..=15 => {
                // Use standard ANSI colors
                match index {
                    0 => (0, 0, 0),        // black
                    1 => (128, 0, 0),      // red
                    2 => (0, 128, 0),      // green
                    3 => (128, 128, 0),    // yellow
                    4 => (0, 0, 128),      // blue
                    5 => (128, 0, 128),    // magenta
                    6 => (0, 128, 128),    // cyan
                    7 => (192, 192, 192),  // white
                    8 => (128, 128, 128),  // bright black
                    9 => (255, 0, 0),      // bright red
                    10 => (0, 255, 0),     // bright green
                    11 => (255, 255, 0),   // bright yellow
                    12 => (0, 0, 255),     // bright blue
                    13 => (255, 0, 255),   // bright magenta
                    14 => (0, 255, 255),   // bright cyan
                    15 => (255, 255, 255), // bright white
                    _ => (255, 255, 255),
                }
            }
            // 6x6x6 RGB cube (16-231)
            16..=231 => {
                let index = index - 16;
                let r = index / 36;
                let g = (index % 36) / 6;
                let b = index % 6;

                // Convert 0-5 range to 0-255 range
                let r = if r == 0 { 0 } else { 55 + r * 40 };
                let g = if g == 0 { 0 } else { 55 + g * 40 };
                let b = if b == 0 { 0 } else { 55 + b * 40 };

                (r, g, b)
            }
            // Grayscale (232-255)
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                (gray, gray, gray)
            }
        }
    }
}

/// One character cell of the screen grid, with its colors and attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The character of the cell.
    pub ch: char,
    /// The foreground color.
    pub fg_color: (u8, u8, u8),
    /// The background color.
    pub bg_color: (u8, u8, u8),
    /// Whether the character is bold.
    pub bold: bool,
    /// Whether the character is italic. The emulator tracks it, and `beta` does
    /// not draw it yet.
    pub italic: bool,
    /// Whether the character is underlined.
    pub underline: bool,
}

/// Tells if the parser is settled after one character: in the ground state, with
/// no unfinished sequence. `was_settled` is the answer before the character.
/// `printed` tells that the parser called `print` for the character, and `ended`
/// that it called `csi_dispatch`, `esc_dispatch`, `osc_dispatch`, or `unhook`.
///
/// The proof, for vte 0.15 and for text that arrives as valid UTF-8, one whole
/// character at a time:
///
/// - Only ESC leaves the ground state. So from a settled parser, each other
///   character leaves it settled.
/// - `print` fires only in the ground state. The character is whole, so the
///   print takes all of it, and no part of a UTF-8 sequence stays behind.
/// - A CSI dispatch or an ESC dispatch returns the parser to the ground state.
///   An OSC string ends in the ground state on BEL, CAN, or SUB. A DCS string
///   ends in the ground state on CAN, SUB, or 0x9C. They end in the Escape state
///   only on ESC, and the rule excludes ESC.
/// - A settle point that the rule misses, for example the end of an ignored CSI
///   sequence, which calls nothing, only keeps bytes in the buffer until the next
///   print or dispatch. That makes the replay longer. It never makes it wrong.
fn ends_settled(ch: char, was_settled: bool, printed: bool, ended: bool) -> bool {
    ch != '\x1b' && (printed || ended || was_settled)
}

/// Makes a parser in the state that `Parser::new()` reaches after it has read
/// `pending`, by feeding it the bytes and discarding what it reports.
fn replayed_parser(pending: &[u8]) -> Parser {
    let mut parser = Parser::new();
    for byte in pending {
        parser.advance(&mut Discard, &[*byte]);
    }
    parser
}

/// A performer that ignores each callback. It serves the replay of `pending`.
struct Discard;

impl Perform for Discard {}

/// The performer that the parser calls during `process_output`. It forwards each
/// callback to the terminal, and records what kind of callback came.
struct Feed<'a> {
    state: &'a mut TerminalState,
    printed: bool,
    ended: bool,
}

impl Perform for Feed<'_> {
    fn print(&mut self, ch: char) {
        self.printed = true;
        self.state.print(ch);
    }

    fn execute(&mut self, byte: u8) {
        self.state.execute(byte);
    }

    fn unhook(&mut self) {
        self.ended = true;
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.ended = true;
        self.state.osc_dispatch(params, bell_terminated);
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        self.ended = true;
        self.state
            .csi_dispatch(params, intermediates, ignore, action);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.ended = true;
        self.state.esc_dispatch(intermediates, ignore, byte);
    }
}

/// A clone is exact: it holds the same screen, and it finishes an unfinished
/// escape sequence in the same way as the original.
///
/// `vte::Parser` does not implement `Clone`. So the terminal keeps the bytes of
/// the sequence that is not finished, and the clone feeds them to a new parser.
/// That parser reaches the state of the original parser.
impl Clone for TerminalState {
    fn clone(&self) -> Self {
        Self {
            grid: self.grid.clone(),
            cursor_x: self.cursor_x,
            cursor_y: self.cursor_y,
            width: self.width,
            height: self.height,
            theme: self.theme.clone(),
            current_fg: self.current_fg,
            current_bg: self.current_bg,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            parser: replayed_parser(&self.pending),
            pending: self.pending.clone(),
            saved_cursor_x: self.saved_cursor_x,
            saved_cursor_y: self.saved_cursor_y,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            use_acs: self.use_acs,
            cursor_visible: self.cursor_visible,
            dynamic_palette: self.dynamic_palette.clone(),
            selection_fg: self.selection_fg,
            selection_bg: self.selection_bg,
            override_fg: self.override_fg,
            override_bg: self.override_bg,
        }
    }
}

/// Two terminals are equal when each later byte has the same effect on both.
///
/// Thus they hold the same screen, cursor, pen, saved cursor, scroll region,
/// character set, cursor visibility, palette, selection and override colors,
/// theme, and size, and the same unfinished escape sequence.
///
/// The parser is not compared. By the argument on `Clone`, a parser is in the
/// state that `Parser::new()` reaches after it reads `pending`, so two equal
/// `pending` buffers give two parsers in the same state.
impl PartialEq for TerminalState {
    fn eq(&self, other: &Self) -> bool {
        // A destructure, so that a new field fails to compile until this
        // comparison names it.
        let Self {
            grid,
            cursor_x,
            cursor_y,
            width,
            height,
            theme,
            current_fg,
            current_bg,
            bold,
            italic,
            underline,
            parser: _,
            pending,
            saved_cursor_x,
            saved_cursor_y,
            scroll_top,
            scroll_bottom,
            use_acs,
            cursor_visible,
            dynamic_palette,
            selection_fg,
            selection_bg,
            override_fg,
            override_bg,
        } = self;
        *cursor_x == other.cursor_x
            && *cursor_y == other.cursor_y
            && *width == other.width
            && *height == other.height
            && *current_fg == other.current_fg
            && *current_bg == other.current_bg
            && *bold == other.bold
            && *italic == other.italic
            && *underline == other.underline
            && *pending == other.pending
            && *saved_cursor_x == other.saved_cursor_x
            && *saved_cursor_y == other.saved_cursor_y
            && *scroll_top == other.scroll_top
            && *scroll_bottom == other.scroll_bottom
            && *use_acs == other.use_acs
            && *cursor_visible == other.cursor_visible
            && *selection_fg == other.selection_fg
            && *selection_bg == other.selection_bg
            && *override_fg == other.override_fg
            && *override_bg == other.override_bg
            && *theme == other.theme
            && *dynamic_palette == other.dynamic_palette
            && *grid == other.grid
    }
}

impl Eq for TerminalState {}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg_color: (255, 255, 255),
            bg_color: (0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// The state of a virtual terminal: the screen grid, the cursor, the current
/// text attributes, and the VT100/ANSI parser that feeds them.
pub struct TerminalState {
    /// The screen grid, one `Vec` of cells for each row.
    pub grid: Vec<Vec<Cell>>,
    /// The cursor column, counted from zero.
    pub cursor_x: usize,
    /// The cursor row, counted from zero.
    pub cursor_y: usize,
    /// The number of columns.
    pub width: usize,
    /// The number of rows.
    pub height: usize,
    /// The theme that gives the default and the base colors.
    pub theme: TerminalTheme,
    /// The foreground color that the next printed character gets.
    pub current_fg: (u8, u8, u8),
    /// The background color that the next printed character gets.
    pub current_bg: (u8, u8, u8),
    /// Whether the next printed character is bold.
    pub bold: bool,
    /// Whether the next printed character is italic.
    pub italic: bool,
    /// Whether the next printed character is underlined.
    pub underline: bool,
    parser: Parser,
    // The bytes fed since the parser was last settled: in the ground state with
    // no unfinished sequence. Empty means the parser equals `Parser::new()`.
    pending: Vec<u8>,
    // Saved cursor position for ESC 7/8
    saved_cursor_x: usize,
    saved_cursor_y: usize,
    // Scrolling region
    scroll_top: usize,
    scroll_bottom: usize,
    // Character set state
    use_acs: bool,
    // Cursor visibility
    cursor_visible: bool,
    // Dynamic color palette for OSC sequences
    dynamic_palette: Vec<(u8, u8, u8)>,
    // Selection colors
    selection_fg: Option<(u8, u8, u8)>,
    selection_bg: Option<(u8, u8, u8)>,
    // Override colors from OSC 10/11
    override_fg: Option<(u8, u8, u8)>,
    override_bg: Option<(u8, u8, u8)>,
    // Terminal modes (tracked for correctness but not used in rendering currently)
}

impl TerminalState {
    /// Creates a blank terminal of `width` columns and `height` rows.
    pub fn new(width: usize, height: usize, theme: TerminalTheme) -> Self {
        let mut grid = Vec::new();
        for _ in 0..height {
            let mut row = Vec::new();
            for _ in 0..width {
                row.push(Cell {
                    ch: ' ',
                    fg_color: theme.foreground,
                    bg_color: theme.background,
                    ..Default::default()
                });
            }
            grid.push(row);
        }

        // Initialize the standard 256-color palette
        let mut dynamic_palette = Vec::with_capacity(256);
        for i in 0..=255 {
            dynamic_palette.push(TerminalTheme::get_256_color(i));
        }

        Self {
            grid,
            cursor_x: 0,
            cursor_y: 0,
            width,
            height,
            current_fg: theme.foreground,
            current_bg: theme.background,
            bold: false,
            italic: false,
            underline: false,
            theme,
            parser: Parser::new(),
            pending: Vec::new(),
            saved_cursor_x: 0,
            saved_cursor_y: 0,
            scroll_top: 0,
            scroll_bottom: height - 1,
            use_acs: false,
            cursor_visible: true,
            dynamic_palette,
            selection_fg: Some((0, 0, 0)), // Default selection: black on yellow
            selection_bg: Some((255, 255, 0)),
            override_fg: None,
            override_bg: None,
        }
    }

    /// Feeds terminal output to the emulator and updates the grid, the cursor, and
    /// the attributes.
    ///
    /// # Errors
    ///
    /// This function never returns an error today. It returns `Result` so that a
    /// future parser failure does not change the signature.
    pub fn process_output(&mut self, data: &str) -> Result<()> {
        // The parser calls back into `self`, so take it out for the whole call.
        let mut parser = std::mem::take(&mut self.parser);
        let mut utf8 = [0_u8; 4];
        for ch in data.chars() {
            let was_settled = self.pending.is_empty();
            let bytes = ch.encode_utf8(&mut utf8).as_bytes();
            let mut feed = Feed {
                state: self,
                printed: false,
                ended: false,
            };
            // One byte for each call, so the parser acts as it always did.
            for byte in bytes {
                parser.advance(&mut feed, &[*byte]);
            }
            let (printed, ended) = (feed.printed, feed.ended);
            if ends_settled(ch, was_settled, printed, ended) {
                self.pending.clear();
            } else {
                self.pending.extend_from_slice(bytes);
            }
        }
        self.parser = parser;
        Ok(())
    }

    /// Returns the screen grid, one `Vec` of cells for each row.
    pub fn get_grid(&self) -> &Vec<Vec<Cell>> {
        &self.grid
    }

    /// Returns the theme of this terminal.
    pub fn get_theme(&self) -> &TerminalTheme {
        &self.theme
    }

    #[cfg(test)]
    pub fn get_width(&self) -> usize {
        self.width
    }

    #[cfg(test)]
    pub fn get_height(&self) -> usize {
        self.height
    }

    /// Returns the cursor position as `(column, row)`, counted from zero.
    pub fn get_cursor_position(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    /// Returns whether the application has the cursor visible.
    pub fn is_cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Returns the `(foreground, background)` colors that a cell is drawn with.
    /// This applies the dynamic palette, the OSC color overrides, and the tmux
    /// color corrections.
    pub fn resolve_cell_colors(&self, cell: &Cell) -> ((u8, u8, u8), (u8, u8, u8)) {
        let mut fg = cell.fg_color;
        let mut bg = cell.bg_color;

        // Only check palette entries that have been modified (not all 256)
        for (index, &dynamic_color) in (0..=u8::MAX).zip(self.dynamic_palette.iter()) {
            let standard_color = TerminalTheme::get_256_color(index);
            if dynamic_color == standard_color {
                continue; // Skip unmodified entries
            }
            if fg == standard_color {
                fg = dynamic_color;
            }
            if bg == standard_color {
                bg = dynamic_color;
            }
        }

        // Apply global foreground/background color overrides from OSC 10/11
        if let Some(override_fg) = self.override_fg {
            // Replace theme foreground or any color that matches the theme foreground
            if fg == self.theme.foreground {
                fg = override_fg;
            }
        }
        if let Some(override_bg) = self.override_bg {
            // Replace theme background or any color that matches the theme background
            if bg == self.theme.background {
                bg = override_bg;
            }
        }

        // Handle common tmux color issues
        // tmux often uses specific color combinations that need adjustment
        self.apply_tmux_color_corrections(fg, bg)
    }

    fn apply_tmux_color_corrections(
        &self,
        fg: (u8, u8, u8),
        bg: (u8, u8, u8),
    ) -> ((u8, u8, u8), (u8, u8, u8)) {
        // Fix common tmux color rendering issues

        // Handle tmux status bar colors - bright green should be normal green
        let corrected_bg = match bg {
            (0, 255, 0) => (0, 128, 0),    // Bright green -> normal green
            (85, 255, 85) => (0, 128, 0),  // Another bright green variant
            (80, 250, 123) => (0, 128, 0), // Dracula bright green -> normal green
            _ => bg,
        };

        // Handle tmux status bar foreground text color
        let corrected_fg = if corrected_bg == (0, 128, 0) {
            // Status bar should have black text on green background for maximum visibility
            (0, 0, 0)
        } else if bg == (0, 255, 0) || bg == (85, 255, 85) || bg == (80, 250, 123) {
            // Even if we didn't correct the background, ensure text is visible on any green
            (0, 0, 0)
        } else if corrected_bg != (0, 0, 0) && fg == corrected_bg {
            // If foreground matches background, force contrasting color
            if is_light_color_terminal(corrected_bg) {
                (0, 0, 0) // Black text on light background
            } else {
                (255, 255, 255) // White text on dark background
            }
        } else {
            fg
        };

        // Handle selection colors - ensure proper contrast for yellow selection background
        let (final_fg, final_bg) =
            if corrected_bg == (255, 255, 0) || corrected_bg == (255, 255, 85) {
                // Yellow background should have black foreground for selection
                ((0, 0, 0), corrected_bg)
            } else {
                (corrected_fg, corrected_bg)
            };

        (final_fg, final_bg)
    }
}
// Helper function for color analysis in terminal renderer
fn is_light_color_terminal(color: (u8, u8, u8)) -> bool {
    let luminance = 0.299 * color.0 as f32 + 0.587 * color.1 as f32 + 0.114 * color.2 as f32;
    luminance > 127.0
}

impl TerminalState {
    fn put_char(&mut self, ch: char) {
        // Convert ACS characters to Unicode box drawing if needed
        let display_char = if self.use_acs {
            match ch {
                'j' => '┘', // Lower right corner
                'k' => '┐', // Upper right corner
                'l' => '┌', // Upper left corner
                'm' => '└', // Lower left corner
                'n' => '┼', // Cross
                'q' => '─', // Horizontal line
                't' => '├', // Left T
                'u' => '┤', // Right T
                'v' => '┴', // Bottom T
                'w' => '┬', // Top T
                'x' => '│', // Vertical line
                _ => ch,
            }
        } else {
            ch
        };

        if self.cursor_y < self.height && self.cursor_x < self.width {
            self.grid[self.cursor_y][self.cursor_x] = Cell {
                ch: display_char,
                fg_color: self.current_fg,
                bg_color: self.current_bg,
                bold: self.bold,
                italic: self.italic,
                underline: self.underline,
            };
            self.cursor_x += 1;

            // Don't auto-wrap at line end - just stop at the edge
            if self.cursor_x >= self.width {
                self.cursor_x = self.width - 1;
            }
        }
    }

    fn scroll_up(&mut self) {
        // Only scroll within the scrolling region
        for y in (self.scroll_top + 1)..=self.scroll_bottom {
            for x in 0..self.width {
                self.grid[y - 1][x] = self.grid[y][x].clone();
            }
        }

        // Clear the bottom line of the scrolling region
        for x in 0..self.width {
            self.grid[self.scroll_bottom][x] = Cell {
                ch: ' ',
                fg_color: self.theme.foreground,
                bg_color: self.theme.background,
                ..Default::default()
            };
        }
    }

    fn clear_line(&mut self, line: usize) {
        if line < self.height {
            for x in 0..self.width {
                self.grid[line][x] = Cell {
                    ch: ' ',
                    fg_color: self.theme.foreground,
                    bg_color: self.theme.background,
                    ..Default::default()
                };
            }
        }
    }

    fn clear_screen(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.grid[y][x] = Cell {
                    ch: ' ',
                    fg_color: self.theme.foreground,
                    bg_color: self.theme.background,
                    ..Default::default()
                };
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn process_sgr_params(&mut self, params: &vte::Params) {
        let param_vec: Vec<u16> = params.iter().flatten().copied().collect();
        let mut i = 0;

        while i < param_vec.len() {
            let param = param_vec[i];

            match param {
                0 => {
                    // Reset
                    self.current_fg = self.theme.foreground;
                    self.current_bg = self.theme.background;
                    self.bold = false;
                    self.italic = false;
                    self.underline = false;
                }
                1 => self.bold = true,
                3 => self.italic = true,
                4 => self.underline = true,
                22 => self.bold = false,
                23 => self.italic = false,
                24 => self.underline = false,
                30..=37 => {
                    // Foreground colors
                    let color_index = sgr_palette_index(param - 30);
                    self.current_fg = self.theme.get_color(color_index);
                }
                38
                    // Extended foreground color
                    if i + 1 < param_vec.len() => {
                        match param_vec[i + 1] {
                            5
                                // 256-color mode: ESC[38;5;n
                                if i + 2 < param_vec.len() => {
                                    let color_index = sgr_color_byte(param_vec[i + 2]);
                                    if (color_index as usize) < self.dynamic_palette.len() {
                                        self.current_fg =
                                            self.dynamic_palette[color_index as usize];
                                    } else {
                                        self.current_fg = TerminalTheme::get_256_color(color_index);
                                    }
                                    i += 2; // Skip the 5 and color index
                                }
                            2
                                // 24-bit RGB mode: ESC[38;2;r;g;b
                                if i + 4 < param_vec.len() => {
                                    let r = sgr_color_byte(param_vec[i + 2]);
                                    let g = sgr_color_byte(param_vec[i + 3]);
                                    let b = sgr_color_byte(param_vec[i + 4]);
                                    self.current_fg = (r, g, b);
                                    i += 4; // Skip the 2, r, g, b
                                }
                            _ => {}
                        }
                    }
                40..=47 => {
                    // Background colors
                    let color_index = sgr_palette_index(param - 40);
                    self.current_bg = self.theme.get_color(color_index);
                }
                48
                    // Extended background color
                    if i + 1 < param_vec.len() => {
                        match param_vec[i + 1] {
                            5
                                // 256-color mode: ESC[48;5;n
                                if i + 2 < param_vec.len() => {
                                    let color_index = sgr_color_byte(param_vec[i + 2]);
                                    if (color_index as usize) < self.dynamic_palette.len() {
                                        self.current_bg =
                                            self.dynamic_palette[color_index as usize];
                                    } else {
                                        self.current_bg = TerminalTheme::get_256_color(color_index);
                                    }
                                    i += 2; // Skip the 5 and color index
                                }
                            2
                                // 24-bit RGB mode: ESC[48;2;r;g;b
                                if i + 4 < param_vec.len() => {
                                    let r = sgr_color_byte(param_vec[i + 2]);
                                    let g = sgr_color_byte(param_vec[i + 3]);
                                    let b = sgr_color_byte(param_vec[i + 4]);
                                    self.current_bg = (r, g, b);
                                    i += 4; // Skip the 2, r, g, b
                                }
                            _ => {}
                        }
                    }
                90..=97 => {
                    // Bright foreground colors
                    let color_index = sgr_palette_index(param - 90 + 8);
                    self.current_fg = self.theme.get_color(color_index);
                }
                100..=107 => {
                    // Bright background colors
                    let color_index = sgr_palette_index(param - 100 + 8);
                    self.current_bg = self.theme.get_color(color_index);
                }
                _ => {}
            }

            i += 1;
        }
    }

    /// Public wrapper for testing parse_color_spec.
    #[cfg(test)]
    pub fn parse_color_spec_pub(&self, color_spec: &str) -> Option<(u8, u8, u8)> {
        self.parse_color_spec(color_spec)
    }

    #[expect(
        clippy::string_slice,
        reason = "every byte-level index below is guarded by an `is_ascii()` check on the whole spec, so no index can land inside a multi-byte character"
    )]
    fn parse_color_spec(&self, color_spec: &str) -> Option<(u8, u8, u8)> {
        let spec = color_spec.trim();
        if spec.starts_with('#') && spec.len() == 7 && spec.is_ascii() {
            // Hex format: #RRGGBB
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&spec[1..3], 16),
                u8::from_str_radix(&spec[3..5], 16),
                u8::from_str_radix(&spec[5..7], 16),
            ) {
                return Some((r, g, b));
            }
        } else if spec.starts_with("rgb:") && spec.is_ascii() {
            // X11 RGB format: rgb:RRRR/GGGG/BBBB or rgb:RR/GG/BB
            let rgb_part = &spec[4..];
            let parts: Vec<&str> = rgb_part.split('/').collect();
            if parts.len() == 3 {
                let parse_component = |s: &str| -> Option<u8> {
                    match s.len() {
                        2 => u8::from_str_radix(s, 16).ok(),
                        4 => u8::from_str_radix(&s[..2], 16).ok(),
                        _ => None,
                    }
                };

                if let (Some(r), Some(g), Some(b)) = (
                    parse_component(parts[0]),
                    parse_component(parts[1]),
                    parse_component(parts[2]),
                ) {
                    return Some((r, g, b));
                }
            }
        } else {
            // Named colors - basic support
            match spec.to_lowercase().as_str() {
                "black" => return Some((0, 0, 0)),
                "red" => return Some((255, 0, 0)),
                "green" => return Some((0, 255, 0)),
                "yellow" => return Some((255, 255, 0)),
                "blue" => return Some((0, 0, 255)),
                "magenta" => return Some((255, 0, 255)),
                "cyan" => return Some((0, 255, 255)),
                "white" => return Some((255, 255, 255)),
                _ => {}
            }
        }

        None
    }
}

// The five callbacks of the parser. They are private, so that the parser of vte
// stays out of the public API: each byte goes through `process_output`, which
// keeps `pending` right.
impl TerminalState {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.cursor_y += 1;
                // Check if we need to scroll within the scrolling region
                if self.cursor_y > self.scroll_bottom {
                    self.scroll_up();
                    self.cursor_y = self.scroll_bottom;
                }
            }
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\t' => {
                let spaces = 8 - (self.cursor_x % 8);
                for _ in 0..spaces {
                    self.put_char(' ');
                }
            }
            b'\x08'
                // Backspace
                if self.cursor_x > 0 => {
                    self.cursor_x -= 1;
                }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // Handle OSC sequences - these are operating system commands
        if params.is_empty() {
            return;
        }

        let first_param = std::str::from_utf8(params[0]).unwrap_or("");

        match first_param {
            "4"
                // OSC 4 ; color_index ; color_spec ST
                // Set/modify color palette entry
                if params.len() >= 3 => {
                    if let Ok(index) = std::str::from_utf8(params[1]).unwrap_or("").parse::<u8>() {
                        let color_spec = std::str::from_utf8(params[2]).unwrap_or("");
                        if let Some(color) = self.parse_color_spec(color_spec) {
                            if (index as usize) < self.dynamic_palette.len() {
                                self.dynamic_palette[index as usize] = color;
                            }
                        }
                    }
                }
            "10"
                // OSC 10 ; color_spec ST - Set text foreground color
                if params.len() >= 2 => {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.override_fg = Some(color);
                    }
                }
            "11"
                // OSC 11 ; color_spec ST - Set text background color
                if params.len() >= 2 => {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.override_bg = Some(color);
                    }
                }
            "17"
                // OSC 17 ; color_spec ST - Set selection background color
                if params.len() >= 2 => {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.selection_bg = Some(color);
                    }
                }
            "19"
                // OSC 19 ; color_spec ST - Set selection foreground color
                if params.len() >= 2 => {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.selection_fg = Some(color);
                    }
                }
            "52" => {
                // OSC 52 - Clipboard operations - ignore for security
            }
            "104"
                // OSC 104 ; index_list ST - Reset color palette entries
                if params.len() >= 2 => {
                    let indices = std::str::from_utf8(params[1]).unwrap_or("");
                    if indices.is_empty() {
                        // Reset all colors
                        for index in 0..=u8::MAX {
                            self.dynamic_palette[usize::from(index)] =
                                TerminalTheme::get_256_color(index);
                        }
                    } else {
                        // Reset specific indices
                        for index_str in indices.split(';') {
                            if let Ok(index) = index_str.parse::<u8>() {
                                if (index as usize) < self.dynamic_palette.len() {
                                    self.dynamic_palette[index as usize] =
                                        TerminalTheme::get_256_color(index);
                                }
                            }
                        }
                    }
                }
            "110" => {
                // OSC 110 ST - Reset text foreground color
                self.override_fg = None;
            }
            "111" => {
                // OSC 111 ST - Reset text background color
                self.override_bg = None;
            }
            "117" => {
                // OSC 117 ST - Reset selection background color
                self.selection_bg = Some((255, 255, 0)); // Reset to default yellow
            }
            "119" => {
                // OSC 119 ST - Reset selection foreground color
                self.selection_fg = Some((0, 0, 0)); // Reset to default black
            }
            _ => {
                // Ignore other OSC sequences
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Check for private mode sequences (CSI ? ...)
        if intermediates.contains(&b'?') {
            match action {
                'h' => {
                    // DECSET - Set Mode
                    for param in params.iter().flatten() {
                        match *param {
                            25 => self.cursor_visible = true,
                            1049 => {
                                self.saved_cursor_x = self.cursor_x;
                                self.saved_cursor_y = self.cursor_y;
                            }
                            // 47, 1047, 1000, 1002, 1006, 2004: acknowledged but not rendered
                            _ => {}
                        }
                    }
                }
                'l' => {
                    // DECRST - Reset Mode
                    for param in params.iter().flatten() {
                        match *param {
                            25 => self.cursor_visible = false,
                            1049 => {
                                self.cursor_x = self.saved_cursor_x;
                                self.cursor_y = self.saved_cursor_y;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match action {
            'H' | 'f' => {
                // Cursor position
                let row = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1);
                let col = params
                    .iter()
                    .nth(1)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1);
                self.cursor_y = ((row as usize).saturating_sub(1)).min(self.height - 1);
                self.cursor_x = ((col as usize).saturating_sub(1)).min(self.width - 1);
            }
            'A' => {
                // Cursor up
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                self.cursor_y = self.cursor_y.saturating_sub(count);
            }
            'B' => {
                // Cursor down
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                self.cursor_y = (self.cursor_y + count).min(self.height - 1);
            }
            'C' => {
                // Cursor right
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                self.cursor_x = (self.cursor_x + count).min(self.width - 1);
            }
            'D' => {
                // Cursor left
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                self.cursor_x = self.cursor_x.saturating_sub(count);
            }
            'J' => {
                // Erase in display
                let mode = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match mode {
                    0 => {
                        // Clear from cursor to end of screen
                        for x in self.cursor_x..self.width {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                        for y in (self.cursor_y + 1)..self.height {
                            self.clear_line(y);
                        }
                    }
                    1 => {
                        // Clear from start of screen to cursor
                        for y in 0..self.cursor_y {
                            self.clear_line(y);
                        }
                        for x in 0..=self.cursor_x {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                    }
                    2 => {
                        // Clear entire screen
                        self.clear_screen();
                    }
                    _ => {}
                }
            }
            'K' => {
                // Erase in line
                let mode = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match mode {
                    0 => {
                        // Clear from cursor to end of line
                        for x in self.cursor_x..self.width {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                    }
                    1 => {
                        // Clear from start of line to cursor
                        for x in 0..=self.cursor_x {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                    }
                    2 => {
                        // Clear entire line
                        self.clear_line(self.cursor_y);
                    }
                    _ => {}
                }
            }
            'm' => {
                // Set graphics rendition (colors, bold, etc.)
                self.process_sgr_params(params);
            }
            'r' => {
                // Set scrolling region (DECSTBM)
                let top = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                let bottom = params
                    .iter()
                    .nth(1)
                    .and_then(|p| p.first().copied())
                    .map_or(self.height, usize::from);

                // Validate and set scrolling region
                if top > 0 && bottom <= self.height && top < bottom {
                    self.scroll_top = top - 1; // Convert to 0-based
                    self.scroll_bottom = bottom - 1;

                    // DECSTBM also moves cursor to home position
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                }
            }
            'c' => {
                // Device Attributes (DA) request - ignore for now
                // tmux sends this to query terminal capabilities
                // We're not responding, just consuming it
            }
            's' => {
                // Save cursor position (ANSI.SYS style) - similar to ESC 7
                self.saved_cursor_x = self.cursor_x;
                self.saved_cursor_y = self.cursor_y;
            }
            'u' => {
                // Restore cursor position (ANSI.SYS style) - similar to ESC 8
                self.cursor_x = self.saved_cursor_x;
                self.cursor_y = self.saved_cursor_y;
            }
            'S' => {
                // Scroll up
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                for _ in 0..count {
                    self.scroll_up();
                }
            }
            'T' => {
                // Scroll down: shift content DOWN within scroll region, clear top
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                for _ in 0..count {
                    for y in (self.scroll_top + 1..=self.scroll_bottom).rev() {
                        for x in 0..self.width {
                            self.grid[y][x] = self.grid[y - 1][x].clone();
                        }
                    }
                    self.clear_line(self.scroll_top);
                }
            }
            'L' => {
                // Insert lines
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                for _ in 0..count {
                    // Move lines down from current cursor position
                    for y in (self.cursor_y + 1..self.height).rev() {
                        for x in 0..self.width {
                            if y > 0 {
                                self.grid[y][x] = self.grid[y - 1][x].clone();
                            }
                        }
                    }
                    self.clear_line(self.cursor_y);
                }
            }
            'M' => {
                // Delete lines
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                for _ in 0..count {
                    // Move lines up from current cursor position
                    for y in self.cursor_y..(self.height - 1) {
                        for x in 0..self.width {
                            self.grid[y][x] = self.grid[y + 1][x].clone();
                        }
                    }
                    self.clear_line(self.height - 1);
                }
            }
            'P' => {
                // Delete characters (clamped to avoid usize underflow)
                let raw_count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                let count = raw_count.min(self.width.saturating_sub(self.cursor_x));
                let start_x = self.cursor_x;
                for x in start_x..self.width.saturating_sub(count) {
                    self.grid[self.cursor_y][x] = self.grid[self.cursor_y][x + count].clone();
                }
                for x in self.width.saturating_sub(count)..self.width {
                    self.grid[self.cursor_y][x] = Cell {
                        ch: ' ',
                        fg_color: self.theme.foreground,
                        bg_color: self.theme.background,
                        ..Default::default()
                    };
                }
            }
            '@' => {
                // Insert characters (clamped to avoid usize underflow)
                let raw_count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                let count = raw_count.min(self.width.saturating_sub(self.cursor_x));
                let start_x = self.cursor_x;
                for x in (start_x..self.width.saturating_sub(count)).rev() {
                    self.grid[self.cursor_y][x + count] = self.grid[self.cursor_y][x].clone();
                }
                for x in start_x..(start_x + count).min(self.width) {
                    self.grid[self.cursor_y][x] = Cell {
                        ch: ' ',
                        fg_color: self.theme.foreground,
                        bg_color: self.theme.background,
                        ..Default::default()
                    };
                }
            }
            'X' => {
                // Erase characters
                let count = params
                    .iter()
                    .nth(0)
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1) as usize;
                for i in 0..count {
                    if self.cursor_x + i < self.width {
                        self.grid[self.cursor_y][self.cursor_x + i] = Cell {
                            ch: ' ',
                            fg_color: self.theme.foreground,
                            bg_color: self.theme.background,
                            ..Default::default()
                        };
                    }
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => {
                // Save cursor position (DECSC)
                self.saved_cursor_x = self.cursor_x;
                self.saved_cursor_y = self.cursor_y;
            }
            b'8' => {
                // Restore cursor position (DECRC)
                self.cursor_x = self.saved_cursor_x;
                self.cursor_y = self.saved_cursor_y;
            }
            b'D' => {
                // Index (IND) - move cursor down, scroll if at bottom
                self.cursor_y += 1;
                if self.cursor_y > self.scroll_bottom {
                    self.scroll_up();
                    self.cursor_y = self.scroll_bottom;
                }
            }
            b'E' => {
                // Next Line (NEL) - move to first column of next line
                self.cursor_x = 0;
                self.cursor_y += 1;
                if self.cursor_y > self.scroll_bottom {
                    self.scroll_up();
                    self.cursor_y = self.scroll_bottom;
                }
            }
            b'M' => {
                // Reverse Index (RI) - move cursor up, scroll if at top
                if self.cursor_y == self.scroll_top {
                    // Scroll down by moving lines down and clearing the top
                    for y in (self.scroll_top + 1..=self.scroll_bottom).rev() {
                        for x in 0..self.width {
                            self.grid[y][x] = self.grid[y - 1][x].clone();
                        }
                    }
                    self.clear_line(self.scroll_top);
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
            }
            b'H' => {
                // Tab Set (HTS) - set tab stop at current column
                // We don't implement tab stops, just consume
            }
            b'=' => {
                // Application Keypad Mode (DECKPAM)
                // Just consume, we don't differentiate keypad modes
            }
            b'>' => {
                // Normal Keypad Mode (DECKPNM)
                // Just consume, we don't differentiate keypad modes
            }
            b'c' => {
                // Reset terminal (RIS) - full reset
                self.clear_screen();
                self.current_fg = self.theme.foreground;
                self.current_bg = self.theme.background;
                self.bold = false;
                self.italic = false;
                self.underline = false;
                self.cursor_visible = true;
                self.use_acs = false;
            }
            _ => {
                // Ignore unhandled escape sequences
            }
        }

        // Handle character set designation sequences
        if !intermediates.is_empty() {
            match intermediates[0] {
                b'(' | b')' | b'*' | b'+' => {
                    // Character set designation
                    match byte {
                        b'0' => self.use_acs = true, // DEC Special Character and Line Drawing Set
                        b'B' => self.use_acs = false, // ASCII character set
                        b'A' => self.use_acs = false, // UK character set
                        _ => {}                      // Other character sets - just use ASCII
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
