//! The VT100/ANSI terminal emulator of `beta`.
//!
//! This crate holds the emulator and the five theme palettes that `beta` uses
//! for its video and web exports. It is shared, so that `nle` uses the same
//! copy.
//!
//! A clone of a `TerminalState` is exact. It copies the screen, and it replays the
//! bytes of an unfinished escape sequence into its own parser.
//!
//! A wide character, such as `日`, takes two cells: the character and a
//! continuation cell after it (`WIDE_CONTINUATION`). A character of no width,
//! such as a combining mark, takes no cell.
//!
//! Two terminals are equal when each later byte has the same effect on both. The
//! backspace removal filter of `nle` uses this equality: it compares the screen
//! before a typed key with the screen after the key that erased it.
//!
//! It does not draw pixels. `beta` keeps the drawing, the fonts, the tmux
//! layout, and the ffmpeg pipe.

use anyhow::Result;
use unicode_width::UnicodeWidthChar;
use vte::{Parser, Perform};

/// The character of the cell after a wide character. It covers both cells of
/// the wide character. `vte` runs NUL as a control and never prints it, thus no
/// printed character has this value.
pub const WIDE_CONTINUATION: char = '\0';

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

/// An extended color that SGR 38, 48 or 58 names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtendedColor {
    /// A color of the 256-color palette.
    Indexed(u8),
    /// A 24-bit color.
    Rgb(u8, u8, u8),
}

/// Reads the colon form of an extended color from the subparameters after the
/// code. `5:n` is indexed. `2:r:g:b` and `2:cs:r:g:b` are RGB, and the color
/// space `cs` is ignored. Each other length gives no color.
fn read_colon_color(subparams: &[u16]) -> Option<ExtendedColor> {
    match *subparams {
        [5, n] => Some(ExtendedColor::Indexed(sgr_color_byte(n))),
        [2, r, g, b] | [2, _, r, g, b] => Some(ExtendedColor::Rgb(
            sgr_color_byte(r),
            sgr_color_byte(g),
            sgr_color_byte(b),
        )),
        _ => None,
    }
}

/// Reads the semicolon form of an extended color from the groups that follow
/// its code, starting at index `at`. Gives the color when it is complete, and
/// the count of groups that the arguments take. An incomplete color takes each
/// group that is left. A mode other than 2 or 5, or no mode, takes none.
fn read_semicolon_color(groups: &[&[u16]], at: usize) -> (Option<ExtendedColor>, usize) {
    let value = |offset: usize| {
        groups
            .get(at + offset)
            .and_then(|group| group.first())
            .copied()
    };
    let left = groups.len().saturating_sub(at);
    match value(0) {
        Some(5) => match value(1) {
            Some(n) => (Some(ExtendedColor::Indexed(sgr_color_byte(n))), 2),
            None => (None, left),
        },
        Some(2) => match (value(1), value(2), value(3)) {
            (Some(r), Some(g), Some(b)) => (
                Some(ExtendedColor::Rgb(
                    sgr_color_byte(r),
                    sgr_color_byte(g),
                    sgr_color_byte(b),
                )),
                4,
            ),
            _ => (None, left),
        },
        _ => (None, 0),
    }
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

/// Where a color came from. The emulator keeps it next to each RGB value, so a
/// writer can say the color again in the form that the source used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorSource {
    /// The default color of the theme. SGR 0, RIS, and a cleared cell give it.
    #[default]
    Default,
    /// One of the 16 base colors, from 0 to 15. SGR 30-37 and 40-47 give 0 to
    /// 7. SGR 90-97 and 100-107 give 8 to 15.
    Base(u8),
    /// A color of the 256-color palette, from SGR 38;5;n or 48;5;n.
    Indexed(u8),
    /// A 24-bit color, from SGR 38;2;r;g;b or 48;2;r;g;b.
    Rgb(u8, u8, u8),
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
    /// The source of the foreground color.
    pub fg_source: ColorSource,
    /// The source of the background color.
    pub bg_source: ColorSource,
    /// Whether the character is bold.
    pub bold: bool,
    /// Whether the character is italic. The emulator tracks it, and `beta` does
    /// not draw it yet.
    pub italic: bool,
    /// Whether the character is underlined.
    pub underline: bool,
    /// Whether the character is dim. The emulator tracks it, and `beta` does not
    /// draw it yet.
    pub dim: bool,
    /// Whether the character is blinking. The emulator tracks it, and `beta` does not
    /// draw it yet.
    pub blink: bool,
    /// Whether the character is in reverse video. The emulator tracks it, and `beta` does not
    /// draw it yet.
    pub reverse: bool,
    /// Whether the character is hidden. The emulator tracks it, and `beta` does not
    /// draw it yet.
    pub hidden: bool,
    /// Whether the character is struck through. The emulator tracks it, and `beta` does not
    /// draw it yet.
    pub strikethrough: bool,
}

/// Tells if the parser is settled after one character: in the ground state, with
/// no unfinished sequence. `was_settled` is the answer before the character.
/// `printed` tells that the parser called `print` for the character. `ended`
/// tells that it called `csi_dispatch`, `esc_dispatch`, `osc_dispatch`, or
/// `unhook`, that it ran CAN or SUB, or that the character is the final byte of
/// a CSI sequence.
///
/// The proof, for vte 0.15 and for text that arrives as valid UTF-8, one whole
/// character at a time:
///
/// - Only ESC leaves the ground state. So from a settled parser, each other
///   character leaves it settled.
/// - `print` fires only in the ground state. The character is whole, so the
///   print takes all of it, and no part of a UTF-8 sequence stays behind.
/// - A CSI dispatch or an ESC dispatch returns the parser to the ground state.
///   An OSC string ends in the ground state on BEL. A DCS string ends in the
///   ground state on 0x9C. They end in the Escape state only on ESC, and the
///   rule excludes ESC.
/// - CAN and SUB move the parser from each state to the ground state, and the
///   parser runs them.
/// - A CSI sequence ends in the ground state on its final byte, 0x40 to 0x7E.
///   When vte ignores the sequence, for example `ESC [ 1 ; ? h`, it calls
///   nothing at that byte. [`Track`] tells that the parser is in a CSI sequence.
///
/// These are all the ways back to the ground state, thus the rule misses no
/// settle point.
fn ends_settled(ch: char, was_settled: bool, printed: bool, ended: bool) -> bool {
    ch != '\x1b' && (printed || ended || was_settled)
}

/// Where the parser of vte 0.15 is, for the one end of a sequence that calls
/// back nothing: the final byte of a CSI sequence that vte ignores.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Track {
    /// The parser is in no state below.
    #[default]
    Elsewhere,
    /// The parser is in the escape state. A `[` starts a CSI sequence.
    Escape,
    /// The parser is in a CSI sequence. A final byte ends it.
    Csi,
}

impl Track {
    /// Returns where the parser is after `ch`, from where it was before.
    ///
    /// ESC moves the parser from each state to the escape state, and CAN and
    /// SUB move it to the ground state. In the escape state and in a CSI
    /// sequence, a control, DEL, and each byte of a character that is not ASCII
    /// keep the state. In a CSI sequence, each byte from 0x20 to 0x3F does too.
    fn after(self, ch: char) -> Self {
        match (self, ch) {
            (_, '\x1b') => Self::Escape,
            (_, '\x18' | '\x1a') => Self::Elsewhere,
            (Self::Escape, '[') => Self::Csi,
            (Self::Escape, ch) if ch.is_ascii_control() || !ch.is_ascii() => Self::Escape,
            (Self::Csi, ch) if Self::is_final_byte(ch) => Self::Elsewhere,
            (Self::Csi, _) => Self::Csi,
            _ => Self::Elsewhere,
        }
    }

    /// Tells if `ch` ends the CSI sequence that the parser is in.
    fn ends_csi(self, ch: char) -> bool {
        self == Self::Csi && Self::is_final_byte(ch)
    }

    /// Tells if `ch` is a final byte of a CSI sequence: 0x40 to 0x7E.
    fn is_final_byte(ch: char) -> bool {
        ('\x40'..='\x7e').contains(&ch)
    }
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

/// The byte `ESC`, which starts an escape sequence.
const ESC: u8 = 0x1b;
/// The byte `CAN`, which ends each sequence.
const CAN: u8 = 0x18;
/// The byte `SUB`, which ends each sequence.
const SUB: u8 = 0x1a;
/// The last C0 control byte.
const LAST_C0: u8 = 0x1f;
/// The byte after `ESC` that starts a DCS string.
const DCS_INTRODUCER: u8 = b'P';
/// The bytes that move the parser out of the escape state. Each other byte
/// runs there, or the parser ignores it.
const ESCAPE_EXIT: std::ops::RangeInclusive<u8> = 0x20..=0x7e;

/// The count that parameter `index` of `params` gives: its value, or 1 when
/// the parameter is missing or 0. A terminal reads a count of 0 as 1.
///
/// `vte` 0.15 gives a missing parameter as 0, so `unwrap_or(1)` alone is not
/// enough.
fn count_param(params: &vte::Params, index: usize) -> usize {
    match params.iter().nth(index).and_then(|p| p.first().copied()) {
        None | Some(0) => 1,
        Some(n) => usize::from(n),
    }
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
        if matches!(byte, CAN | SUB) {
            self.ended = true;
        }
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
            changed_rows: self.changed_rows.clone(),
            cursor_x: self.cursor_x,
            cursor_y: self.cursor_y,
            width: self.width,
            height: self.height,
            theme: self.theme.clone(),
            current_fg: self.current_fg,
            current_bg: self.current_bg,
            current_fg_source: self.current_fg_source,
            current_bg_source: self.current_bg_source,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            dim: self.dim,
            blink: self.blink,
            reverse: self.reverse,
            hidden: self.hidden,
            strikethrough: self.strikethrough,
            parser: replayed_parser(&self.pending),
            pending: self.pending.clone(),
            track: self.track,
            saved: self.saved,
            restores: self.restores,
            alt_screen: self.alt_screen,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            use_acs: self.use_acs,
            autowrap: self.autowrap,
            wrap_pending: self.wrap_pending,
            cursor_visible: self.cursor_visible,
            dynamic_palette: self.dynamic_palette.clone(),
            selection_fg: self.selection_fg,
            selection_bg: self.selection_bg,
            override_fg: self.override_fg,
            override_bg: self.override_bg,
        }
    }
}

impl Cell {
    /// Tells if the cell is the continuation of a wide character: the second
    /// cell of the two that the wide character covers.
    pub fn is_wide_continuation(&self) -> bool {
        self.ch == WIDE_CONTINUATION
    }
}

/// Two terminals are equal when each later byte has the same effect on both.
///
/// Thus they hold the same screen, cursor, pen, saved cursor of each screen,
/// alternate screen state, scroll region, character set, autowrap mode,
/// pending wrap, cursor visibility, palette, selection and override colors,
/// theme, and size, and the same unfinished escape sequence. The pen is the
/// colors, the source of each color, and each attribute that the next printed
/// character gets.
///
/// The parser is not compared. By the argument on `Clone`, a parser is in the
/// state that `Parser::new()` reaches after it reads `pending`, so two equal
/// `pending` buffers give two parsers in the same state.
///
/// The count of restores (`TerminalState::restore_count`) is not compared. It
/// counts the restores that came before, and no later byte reads it.
///
/// The marks of `take_changed_rows` are not compared. They record which rows
/// changed, and no later byte reads them.
impl PartialEq for TerminalState {
    fn eq(&self, other: &Self) -> bool {
        // A destructure, so that a new field fails to compile until this
        // comparison names it.
        let Self {
            grid,
            changed_rows: _,
            cursor_x,
            cursor_y,
            width,
            height,
            theme,
            current_fg,
            current_bg,
            current_fg_source,
            current_bg_source,
            bold,
            italic,
            underline,
            dim,
            blink,
            reverse,
            hidden,
            strikethrough,
            parser: _,
            pending,
            track,
            restores: _,
            saved,
            alt_screen,
            scroll_top,
            scroll_bottom,
            use_acs,
            autowrap,
            wrap_pending,
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
            && *current_fg_source == other.current_fg_source
            && *current_bg_source == other.current_bg_source
            && *bold == other.bold
            && *italic == other.italic
            && *underline == other.underline
            && *dim == other.dim
            && *blink == other.blink
            && *reverse == other.reverse
            && *hidden == other.hidden
            && *strikethrough == other.strikethrough
            && *pending == other.pending
            && *track == other.track
            && *saved == other.saved
            && *alt_screen == other.alt_screen
            && *scroll_top == other.scroll_top
            && *scroll_bottom == other.scroll_bottom
            && *use_acs == other.use_acs
            && *autowrap == other.autowrap
            && *wrap_pending == other.wrap_pending
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
            fg_source: ColorSource::Default,
            bg_source: ColorSource::Default,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            blink: false,
            reverse: false,
            hidden: false,
            strikethrough: false,
        }
    }
}

/// The state that DECSC saves in xterm: the cursor position, the pen, and the
/// character set. The pen is the colors, the source of each color, and each
/// text flag. DECRC restores all of them.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SavedCursor {
    x: usize,
    y: usize,
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    fg_source: ColorSource,
    bg_source: ColorSource,
    bold: bool,
    italic: bool,
    underline: bool,
    dim: bool,
    blink: bool,
    reverse: bool,
    hidden: bool,
    strikethrough: bool,
    use_acs: bool,
}

/// The state of a virtual terminal: the screen grid, the cursor, the current
/// text attributes, and the VT100/ANSI parser that feeds them.
pub struct TerminalState {
    /// The screen grid, one `Vec` of cells for each row.
    pub grid: Vec<Vec<Cell>>,
    /// One flag for each row. A flag is `true` when the emulator changed a cell
    /// of that row since the last call of `take_changed_rows`.
    changed_rows: Vec<bool>,
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
    /// The source of `current_fg`.
    pub current_fg_source: ColorSource,
    /// The source of `current_bg`.
    pub current_bg_source: ColorSource,
    /// Whether the next printed character is bold.
    pub bold: bool,
    /// Whether the next printed character is italic.
    pub italic: bool,
    /// Whether the next printed character is underlined.
    pub underline: bool,
    /// Whether the next printed character is dim.
    pub dim: bool,
    /// Whether the next printed character is blinking.
    pub blink: bool,
    /// Whether the next printed character is in reverse video.
    pub reverse: bool,
    /// Whether the next printed character is hidden.
    pub hidden: bool,
    /// Whether the next printed character is struck through.
    pub strikethrough: bool,
    parser: Parser,
    // The bytes fed since the parser was last settled: in the ground state with
    // no unfinished sequence. Empty means the parser equals `Parser::new()`.
    pending: Vec<u8>,
    // Where the parser is, for the end of a CSI sequence that calls nothing.
    track: Track,
    // The count of restores of a save slot.
    restores: u64,
    // The saved cursor states. Slot 0 is for the normal screen.
    // Slot 1 is for the alternate screen.
    saved: [SavedCursor; 2],
    // Whether the alternate screen is on. The terminal keeps one grid.
    alt_screen: bool,
    // Scrolling region
    scroll_top: usize,
    scroll_bottom: usize,
    // Character set state
    use_acs: bool,
    // Autowrap mode (DECAWM, mode 7)
    autowrap: bool,
    // A character went into the last column with autowrap on
    wrap_pending: bool,
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
    ///
    /// A dimension of 0 becomes 1. The size often comes from outside the
    /// program, for example from the header of a recording, and the cursor
    /// needs one cell. Thus the last column and the last row always exist.
    pub fn new(width: usize, height: usize, theme: TerminalTheme) -> Self {
        let width = width.max(1);
        let height = height.max(1);
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

        let initial_cursor = SavedCursor {
            x: 0,
            y: 0,
            fg: theme.foreground,
            bg: theme.background,
            fg_source: ColorSource::Default,
            bg_source: ColorSource::Default,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            blink: false,
            reverse: false,
            hidden: false,
            strikethrough: false,
            use_acs: false,
        };

        Self {
            grid,
            changed_rows: vec![false; height],
            cursor_x: 0,
            cursor_y: 0,
            width,
            height,
            current_fg: theme.foreground,
            current_bg: theme.background,
            current_fg_source: ColorSource::Default,
            current_bg_source: ColorSource::Default,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            blink: false,
            reverse: false,
            hidden: false,
            strikethrough: false,
            theme,
            parser: Parser::new(),
            pending: Vec::new(),
            track: Track::default(),
            saved: [initial_cursor; 2],
            restores: 0,
            alt_screen: false,
            scroll_top: 0,
            scroll_bottom: height - 1,
            use_acs: false,
            autowrap: true,
            wrap_pending: false,
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
            let (printed, ended) = (feed.printed, feed.ended || self.track.ends_csi(ch));
            self.track = self.track.after(ch);
            if ends_settled(ch, was_settled, printed, ended) {
                self.pending.clear();
            } else {
                self.pending.extend_from_slice(bytes);
            }
        }
        self.parser = parser;
        Ok(())
    }

    /// Returns the rows whose cells changed since the last call, in order,
    /// counted from zero. The call clears the marks. A write to `grid` from
    /// outside the emulator does not mark its row.
    pub fn take_changed_rows(&mut self) -> Vec<usize> {
        let rows = self
            .changed_rows
            .iter()
            .enumerate()
            .filter_map(|(row, changed)| changed.then_some(row))
            .collect();
        self.changed_rows.fill(false);
        rows
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

    /// Returns the scrolling region as `(top, bottom)`, counted from zero,
    /// with both rows included.
    pub fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_top, self.scroll_bottom)
    }

    /// Returns the saved cursor position of the current screen as
    /// `(column, row)`, counted from zero. A restore of the cursor goes there.
    /// `ESC 7`, `CSI s`, and `CSI ? 1048 h` save it. `CSI ? 1049 h` saves it
    /// for the normal screen, then switches to the alternate screen, which has
    /// a save slot of its own.
    pub fn saved_cursor_position(&self) -> (usize, usize) {
        let saved = self.saved_slot();
        (saved.x, saved.y)
    }

    /// Returns whether the line-drawing character set is on. `ESC ( 0` turns it
    /// on, and `ESC ( B` turns it off.
    pub fn uses_line_drawing(&self) -> bool {
        self.use_acs
    }

    /// Returns whether autowrap is on (mode 7, DECAWM). `CSI ? 7 h` turns it
    /// on, and `CSI ? 7 l` turns it off. It is on after `new` and after RIS.
    pub fn uses_autowrap(&self) -> bool {
        self.autowrap
    }

    /// Returns whether a wrap is pending. A character went into the last
    /// column with autowrap on, and the next printed character goes to the
    /// start of the next row.
    pub fn is_wrap_pending(&self) -> bool {
        self.wrap_pending
    }

    /// Returns whether the application has the cursor visible.
    pub fn is_cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Returns how many times a restore of the cursor state has run: ESC 8,
    /// `CSI u`, `?1048l` and `?1049l`. A restore is the one event that lets a
    /// save slot change what the terminal shows later. The count wraps.
    pub fn restore_count(&self) -> u64 {
        self.restores
    }

    /// Returns whether the parser holds no unfinished escape sequence.
    pub fn is_settled(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns the text that puts a parser in the ground state into the state
    /// of this parser. The text is empty when this parser is settled.
    ///
    /// The text starts at the last `ESC` of the unfinished sequence. An `ESC`
    /// moves the parser from each state to the escape state and clears its
    /// parameters, thus the bytes before the last `ESC` do not matter.
    ///
    /// Before the `P` of a DCS, and in each other sequence, the text holds no
    /// C0 control. There a control runs, or the parser ignores it, and it does
    /// not change the state. A terminal that reads the text then runs no
    /// control twice. After the `P` of a DCS, the text keeps each byte. vte 0.15
    /// runs no control in a DCS, and it gives each control of the payload to
    /// `put`.
    pub fn unfinished_sequence(&self) -> String {
        let Some(start) = self.pending.iter().rposition(|byte| *byte == ESC) else {
            return String::new();
        };
        let tail = self.pending.get(start..).unwrap_or_default();
        let dcs_at = tail
            .iter()
            .position(|byte| ESCAPE_EXIT.contains(byte))
            .filter(|&index| tail.get(index) == Some(&DCS_INTRODUCER));
        let kept: Vec<u8> = tail
            .iter()
            .enumerate()
            .filter(|&(index, &byte)| {
                byte == ESC || byte > LAST_C0 || dcs_at.is_some_and(|at| index > at)
            })
            .map(|(_, &byte)| byte)
            .collect();
        String::from_utf8_lossy(&kept).into_owned()
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
    /// Returns the state that DECSC saves: the cursor, the pen, and the
    /// character set.
    fn cursor_state(&self) -> SavedCursor {
        SavedCursor {
            x: self.cursor_x,
            y: self.cursor_y,
            fg: self.current_fg,
            bg: self.current_bg,
            fg_source: self.current_fg_source,
            bg_source: self.current_bg_source,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            dim: self.dim,
            blink: self.blink,
            reverse: self.reverse,
            hidden: self.hidden,
            strikethrough: self.strikethrough,
            use_acs: self.use_acs,
        }
    }

    /// The save slot of the current screen.
    fn saved_slot(&self) -> &SavedCursor {
        &self.saved[usize::from(self.alt_screen)]
    }

    /// Saves the cursor state into the slot of the current screen.
    fn save_cursor(&mut self) {
        self.saved[usize::from(self.alt_screen)] = self.cursor_state();
    }

    /// Restores the cursor state from the slot of the current screen. A
    /// restore moves the cursor, thus it ends a pending wrap.
    fn restore_cursor(&mut self) {
        self.restores = self.restores.wrapping_add(1);
        self.wrap_pending = false;
        // A destructure, so that a new field fails to compile until the
        // restore names it.
        let SavedCursor {
            x,
            y,
            fg,
            bg,
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
            use_acs,
        } = *self.saved_slot();
        self.cursor_x = x;
        self.cursor_y = y;
        self.current_fg = fg;
        self.current_bg = bg;
        self.current_fg_source = fg_source;
        self.current_bg_source = bg_source;
        self.bold = bold;
        self.italic = italic;
        self.underline = underline;
        self.dim = dim;
        self.blink = blink;
        self.reverse = reverse;
        self.hidden = hidden;
        self.strikethrough = strikethrough;
        self.use_acs = use_acs;
    }

    /// Moves the cursor down one row. On the bottom row of the scrolling
    /// region, the region scrolls up and the cursor stays. On the last row of
    /// the screen below the region, the cursor stays and nothing scrolls, as
    /// in xterm.
    fn line_feed(&mut self) {
        if self.cursor_y == self.scroll_bottom {
            self.scroll_up();
        } else if self.cursor_y + 1 < self.height {
            self.cursor_y += 1;
        }
    }

    /// Moves the cursor down `count` rows, for CUD and CNL. As CursorDown in
    /// xterm's cursor.c, the move stops on the bottom margin of the
    /// scrolling region when the cursor starts on it or above it. From below
    /// the region, it stops on the last row of the screen. Nothing scrolls.
    fn cursor_down(&mut self, count: usize) {
        let limit = if self.cursor_y <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.height - 1
        };
        self.cursor_y = self.cursor_y.saturating_add(count).min(limit);
    }

    /// Moves the cursor up `count` rows, for CUU and CPL. As CursorUp in
    /// xterm's cursor.c, the move stops on the top margin of the scrolling
    /// region when the cursor starts on it or below it. From above the
    /// region, it stops on the first row of the screen. Nothing scrolls.
    fn cursor_up(&mut self, count: usize) {
        let limit = if self.cursor_y >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        self.cursor_y = self.cursor_y.saturating_sub(count).max(limit);
    }

    /// Marks one row as changed. A row outside the screen is ignored.
    fn mark_row(&mut self, row: usize) {
        if let Some(flag) = self.changed_rows.get_mut(row) {
            *flag = true;
        }
    }

    /// Marks the rows from `first` to `last`, both included, as changed.
    fn mark_rows(&mut self, first: usize, last: usize) {
        for row in first..=last {
            self.mark_row(row);
        }
    }

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

        // A character of no width, such as a combining mark, takes no cell,
        // and thus it keeps a pending wrap.
        let width = UnicodeWidthChar::width(display_char).unwrap_or(1);
        if width == 0 {
            return;
        }

        // A character after the last column goes to the start of the next row.
        if self.wrap_pending {
            self.wrap_pending = false;
            self.cursor_x = 0;
            self.line_feed();
        }

        if width >= 2 && self.cursor_x + 2 > self.width {
            // A wide character does not fit in the last column.
            if self.autowrap && self.width >= 2 {
                self.cursor_x = 0;
                self.line_feed();
            } else {
                return;
            }
        }

        if self.cursor_y < self.height && self.cursor_x < self.width {
            let x = self.cursor_x;
            let cells = if width >= 2 { 2 } else { 1 };
            self.mark_row(self.cursor_y);
            self.split_wide_pairs_around(x, cells);
            let cell = Cell {
                ch: display_char,
                fg_color: self.current_fg,
                bg_color: self.current_bg,
                fg_source: self.current_fg_source,
                bg_source: self.current_bg_source,
                bold: self.bold,
                italic: self.italic,
                underline: self.underline,
                dim: self.dim,
                blink: self.blink,
                reverse: self.reverse,
                hidden: self.hidden,
                strikethrough: self.strikethrough,
            };
            let row = &mut self.grid[self.cursor_y];
            if cells == 2 {
                if let Some(next) = row.get_mut(x + 1) {
                    *next = Cell {
                        ch: WIDE_CONTINUATION,
                        ..cell
                    };
                }
            }
            row[x] = cell;
            if x + cells >= self.width {
                // Stay in the last column. The wrap happens at the next character.
                self.cursor_x = self.width - 1;
                self.wrap_pending = self.autowrap;
            } else {
                self.cursor_x += cells;
            }
        }
    }

    /// A blank cell: a space with the default colors and no attribute.
    fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg_color: self.theme.foreground,
            bg_color: self.theme.background,
            ..Default::default()
        }
    }

    /// Keeps the rule of the wide characters before a write of `cells` cells at
    /// column `x` of the cursor row. A write that starts on a continuation cell
    /// blanks the wide character before it. A write that ends before a
    /// continuation cell blanks that cell.
    fn split_wide_pairs_around(&mut self, x: usize, cells: usize) {
        let blank = self.blank_cell();
        let Some(row) = self.grid.get_mut(self.cursor_y) else {
            return;
        };
        if x > 0 && row.get(x).is_some_and(Cell::is_wide_continuation) {
            if let Some(previous) = row.get_mut(x - 1) {
                *previous = blank.clone();
            }
        }
        if let Some(after) = row.get_mut(x + cells) {
            if after.is_wide_continuation() {
                *after = blank;
            }
        }
    }

    /// Repairs the cursor row after an edit that can cut a wide character. A
    /// continuation cell that does not follow a wide character becomes blank,
    /// and a wide character that no continuation cell follows becomes blank.
    fn repair_wide_pairs(&mut self) {
        self.mark_row(self.cursor_y);
        let blank = self.blank_cell();
        let Some(row) = self.grid.get_mut(self.cursor_y) else {
            return;
        };
        for x in 0..row.len() {
            let is_head = row
                .get(x)
                .is_some_and(|cell| UnicodeWidthChar::width(cell.ch) == Some(2));
            let followed = row.get(x + 1).is_some_and(Cell::is_wide_continuation);
            let is_continuation = row.get(x).is_some_and(Cell::is_wide_continuation);
            let after_head = x > 0
                && row
                    .get(x - 1)
                    .is_some_and(|cell| UnicodeWidthChar::width(cell.ch) == Some(2));
            let broken = (is_head && !followed) || (is_continuation && !after_head);
            if broken {
                if let Some(cell) = row.get_mut(x) {
                    *cell = blank.clone();
                }
            }
        }
    }

    fn scroll_up(&mut self) {
        self.mark_rows(self.scroll_top, self.scroll_bottom);
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
            self.mark_row(line);
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
        self.mark_rows(0, self.height.saturating_sub(1));
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

    /// Turns an extended color into its RGB value and its source. An indexed
    /// color reads the dynamic palette when the index is inside it.
    fn resolve_extended_color(&self, color: ExtendedColor) -> ((u8, u8, u8), ColorSource) {
        match color {
            ExtendedColor::Indexed(index) => {
                let rgb = self
                    .dynamic_palette
                    .get(usize::from(index))
                    .copied()
                    .unwrap_or_else(|| TerminalTheme::get_256_color(index));
                (rgb, ColorSource::Indexed(index))
            }
            ExtendedColor::Rgb(r, g, b) => ((r, g, b), ColorSource::Rgb(r, g, b)),
        }
    }

    /// Applies the groups of one SGR sequence to the pen. A group with colon
    /// subparameters takes no other group. A group with none can take the
    /// groups after it as the arguments of an extended color (38, 48 or 58).
    fn process_sgr_params(&mut self, params: &vte::Params) {
        let groups: Vec<&[u16]> = params.iter().collect();
        let mut i = 0;

        while let Some(group) = groups.get(i) {
            let Some((&code, subparams)) = group.split_first() else {
                i += 1;
                continue;
            };
            i += 1;

            if !subparams.is_empty() {
                // The colon form: this group holds its own arguments.
                match code {
                    4 => self.underline = subparams.first().copied() != Some(0),
                    38 | 48 => {
                        if let Some(color) = read_colon_color(subparams) {
                            self.set_extended_color(code, color);
                        }
                    }
                    // 58 is the underline color, which the model lacks.
                    _ => {}
                }
                continue;
            }

            match code {
                0 => {
                    // Reset
                    self.current_fg = self.theme.foreground;
                    self.current_bg = self.theme.background;
                    self.current_fg_source = ColorSource::Default;
                    self.current_bg_source = ColorSource::Default;
                    self.bold = false;
                    self.italic = false;
                    self.underline = false;
                    self.dim = false;
                    self.blink = false;
                    self.reverse = false;
                    self.hidden = false;
                    self.strikethrough = false;
                }
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.reverse = true,
                8 => self.hidden = true,
                9 => self.strikethrough = true,
                22 => {
                    // Normal intensity: neither bold nor dim.
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.reverse = false,
                28 => self.hidden = false,
                29 => self.strikethrough = false,
                39 => {
                    // Only the source changes. Decision H-165-2 keeps the RGB
                    // value for each existing caller.
                    self.current_fg_source = ColorSource::Default;
                }
                49 => {
                    // Only the source changes. Decision H-165-2 keeps the RGB
                    // value for each existing caller.
                    self.current_bg_source = ColorSource::Default;
                }
                30..=37 => {
                    // Foreground colors
                    let color_index = sgr_palette_index(code - 30);
                    self.current_fg = self.theme.get_color(color_index);
                    self.current_fg_source = ColorSource::Base(color_index);
                }
                40..=47 => {
                    // Background colors
                    let color_index = sgr_palette_index(code - 40);
                    self.current_bg = self.theme.get_color(color_index);
                    self.current_bg_source = ColorSource::Base(color_index);
                }
                38 | 48 | 58 => {
                    // Extended color, semicolon form: the next groups are its
                    // arguments. 58 is the underline color, which the model
                    // lacks, so it reads its arguments and changes nothing.
                    let (color, taken) = read_semicolon_color(&groups, i);
                    i += taken;
                    if code != 58 {
                        if let Some(color) = color {
                            self.set_extended_color(code, color);
                        }
                    }
                }
                // Default underline color, which the model lacks.
                59 => {}
                90..=97 => {
                    // Bright foreground colors
                    let color_index = sgr_palette_index(code - 90 + 8);
                    self.current_fg = self.theme.get_color(color_index);
                    self.current_fg_source = ColorSource::Base(color_index);
                }
                100..=107 => {
                    // Bright background colors
                    let color_index = sgr_palette_index(code - 100 + 8);
                    self.current_bg = self.theme.get_color(color_index);
                    self.current_bg_source = ColorSource::Base(color_index);
                }
                _ => {}
            }
        }
    }

    /// Sets the foreground (`code` 38) or the background (`code` 48) pen color.
    fn set_extended_color(&mut self, code: u16, color: ExtendedColor) {
        let (rgb, source) = self.resolve_extended_color(color);
        if code == 38 {
            self.current_fg = rgb;
            self.current_fg_source = source;
        } else {
            self.current_bg = rgb;
            self.current_bg_source = source;
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
        // LF, CR, BS and HT move the cursor, so they end a pending wrap.
        if matches!(byte, b'\n' | b'\r' | b'\x08' | b'\t') {
            self.wrap_pending = false;
        }
        match byte {
            b'\n' => self.line_feed(),
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\t' => {
                // A tab moves the cursor and writes no cell.
                self.cursor_x = ((self.cursor_x / 8 + 1) * 8).min(self.width - 1);
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
                            7 => {
                                self.autowrap = true;
                                self.wrap_pending = false;
                            }
                            25 => self.cursor_visible = true,
                            1049 => {
                                // Save into the slot of the current screen, then switch
                                self.save_cursor();
                                self.alt_screen = true;
                            }
                            // The one grid stays. These modes only switch the save slot.
                            47 | 1047 => self.alt_screen = true,
                            1048 => self.save_cursor(),
                            // 1000, 1002, 1006, 2004: acknowledged but not rendered
                            _ => {}
                        }
                    }
                }
                'l' => {
                    // DECRST - Reset Mode
                    for param in params.iter().flatten() {
                        match *param {
                            7 => {
                                self.autowrap = false;
                                self.wrap_pending = false;
                            }
                            25 => self.cursor_visible = false,
                            1049 => {
                                // Switch to the normal screen, then restore its slot
                                self.alt_screen = false;
                                self.restore_cursor();
                            }
                            47 | 1047 => self.alt_screen = false,
                            1048 => self.restore_cursor(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Each cursor move and each edit ends a pending wrap. SGR, save
        // cursor, scroll, and the device attributes request do not. DECSTBM
        // ends it only when it moves the cursor home, in its own arm.
        if matches!(
            action,
            'H' | 'f'
                | 'A'
                | 'B'
                | 'C'
                | 'D'
                | 'E'
                | 'F'
                | 'G'
                | '`'
                | 'd'
                | 'J'
                | 'K'
                | 'X'
                | 'P'
                | '@'
                | 'L'
                | 'M'
                | 'u'
        ) {
            self.wrap_pending = false;
        }

        match action {
            'H' | 'f' => {
                // Cursor position
                let row = count_param(params, 0);
                let col = count_param(params, 1);
                self.cursor_y = row.saturating_sub(1).min(self.height - 1);
                self.cursor_x = col.saturating_sub(1).min(self.width - 1);
            }
            'A' => {
                // Cursor up
                self.cursor_up(count_param(params, 0));
            }
            'B' => {
                // Cursor down
                self.cursor_down(count_param(params, 0));
            }
            'G' | '`' => {
                // CHA and HPA: cursor to a column
                let col = count_param(params, 0);
                self.cursor_x = col.saturating_sub(1).min(self.width - 1);
            }
            'd' => {
                // VPA: cursor to a row
                let row = count_param(params, 0);
                self.cursor_y = row.saturating_sub(1).min(self.height - 1);
            }
            'E' => {
                // CNL: cursor down, to the first column
                self.cursor_down(count_param(params, 0));
                self.cursor_x = 0;
            }
            'F' => {
                // CPL: cursor up, to the first column
                self.cursor_up(count_param(params, 0));
                self.cursor_x = 0;
            }
            'C' => {
                // Cursor right
                let count = count_param(params, 0);
                self.cursor_x = (self.cursor_x + count).min(self.width - 1);
            }
            'D' => {
                // Cursor left
                let count = count_param(params, 0);
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
                        self.mark_row(self.cursor_y);
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
                        self.mark_row(self.cursor_y);
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
                if matches!(mode, 0 | 1) {
                    self.repair_wide_pairs();
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
                        self.mark_row(self.cursor_y);
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
                        self.mark_row(self.cursor_y);
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
                self.repair_wide_pairs();
            }
            'm' => {
                // Set graphics rendition (colors, bold, etc.)
                self.process_sgr_params(params);
            }
            'r' => {
                // Set scrolling region (DECSTBM). As CASE_DECSTBM in xterm's
                // charproc.c, read a bottom that is missing, 0, or below the
                // screen as the last row.
                let top = count_param(params, 0);
                let bottom = match params.iter().nth(1).and_then(|p| p.first().copied()) {
                    None | Some(0) => self.height,
                    Some(n) => usize::from(n).min(self.height),
                };

                // xterm ignores a DECSTBM whose top is not above its bottom.
                if top > 0 && top < bottom {
                    self.scroll_top = top - 1; // Convert to 0-based
                    self.scroll_bottom = bottom - 1;

                    // DECSTBM also moves cursor to home position, thus it ends
                    // a pending wrap. An ignored DECSTBM keeps it, as in xterm.
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                    self.wrap_pending = false;
                }
            }
            'c' => {
                // Device Attributes (DA) request - ignore for now
                // tmux sends this to query terminal capabilities
                // We're not responding, just consuming it
            }
            's' => {
                // Save cursor (ANSI.SYS style) - the same as ESC 7
                self.save_cursor();
            }
            'u' => {
                // Restore cursor (ANSI.SYS style) - the same as ESC 8
                self.restore_cursor();
            }
            'S' => {
                // Scroll up
                let count = count_param(params, 0);
                for _ in 0..count {
                    self.scroll_up();
                }
            }
            'T' => {
                // Scroll down: shift content DOWN within scroll region, clear top
                let count = count_param(params, 0);
                self.mark_rows(self.scroll_top, self.scroll_bottom);
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
                let count = count_param(params, 0);
                self.mark_rows(self.cursor_y, self.height.saturating_sub(1));
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
                let count = count_param(params, 0);
                self.mark_rows(self.cursor_y, self.height.saturating_sub(1));
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
                let raw_count = count_param(params, 0);
                let count = raw_count.min(self.width.saturating_sub(self.cursor_x));
                let start_x = self.cursor_x;
                self.mark_row(self.cursor_y);
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
                self.repair_wide_pairs();
            }
            '@' => {
                // Insert characters (clamped to avoid usize underflow)
                let raw_count = count_param(params, 0);
                let count = raw_count.min(self.width.saturating_sub(self.cursor_x));
                let start_x = self.cursor_x;
                self.mark_row(self.cursor_y);
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
                self.repair_wide_pairs();
            }
            'X' => {
                // Erase characters
                let count = count_param(params, 0);
                self.mark_row(self.cursor_y);
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
                self.repair_wide_pairs();
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        // DECRC, IND, NEL, RI and RIS move the cursor, so they end a pending wrap.
        if matches!(byte, b'8' | b'D' | b'E' | b'M' | b'c') {
            self.wrap_pending = false;
        }
        match byte {
            b'7' => {
                // Save the cursor, the attributes, and the character set (DECSC)
                self.save_cursor();
            }
            b'8' => {
                // Restore the cursor, the attributes, and the character set (DECRC)
                self.restore_cursor();
            }
            b'D' => {
                // Index (IND) - a line feed
                self.line_feed();
            }
            b'E' => {
                // Next Line (NEL) - the first column, then a line feed
                self.cursor_x = 0;
                self.line_feed();
            }
            b'M' => {
                // Reverse Index (RI) - move cursor up, scroll if at top
                if self.cursor_y == self.scroll_top {
                    self.mark_rows(self.scroll_top, self.scroll_bottom);
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
                self.current_fg_source = ColorSource::Default;
                self.current_bg_source = ColorSource::Default;
                self.bold = false;
                self.italic = false;
                self.underline = false;
                self.dim = false;
                self.blink = false;
                self.reverse = false;
                self.hidden = false;
                self.strikethrough = false;
                self.cursor_visible = true;
                self.use_acs = false;
                self.autowrap = true;
                self.wrap_pending = false;
                self.alt_screen = false;
                self.saved = [self.cursor_state(); 2];
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
