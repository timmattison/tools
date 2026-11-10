//! The preview lines of one session.
//!
//! The module lays the whole session out as one sequence of lines and cuts a
//! window of `height` lines from it.

use unicode_width::UnicodeWidthChar;

use crate::query::{Found, Query};
use crate::session::Session;

/// The kind of a piece of text. The renderer maps each tone to a style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Plain message text.
    Text,
    /// Text that a word of the query matched.
    Highlight,
    /// The `user` or `assistant` line that starts a message.
    Author,
    /// The line that marks a compaction.
    Compaction,
    /// A line that is not message text, such as `(no messages)`.
    Muted,
}

/// A run of text in one tone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The text of the piece.
    pub text: String,
    /// The tone of the piece.
    pub tone: Tone,
}

/// One line of the preview, as a list of pieces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreviewLine(pub Vec<Piece>);

/// The lines to draw and the scroll that produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// At most `height` lines, from the top of the view.
    pub lines: Vec<PreviewLine>,
    /// The final top line minus the default top line, after the clamps.
    pub scroll: i64,
}

impl PreviewLine {
    /// The text of the line: all pieces joined.
    #[must_use]
    pub fn text(&self) -> String {
        self.0.iter().map(|piece| piece.text.as_str()).collect()
    }

    /// The width of the line in display columns.
    #[must_use]
    pub fn width(&self) -> usize {
        self.0
            .iter()
            .flat_map(|piece| piece.text.chars())
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum()
    }
}

/// Group the digits of `n` in threes: `259822` gives `"259,822"`.
#[must_use]
pub fn thousands(n: u64) -> String {
    n.to_string()
}

/// The lines of the view of `session`.
#[must_use]
pub fn preview(
    _session: &Session,
    _query: &Query,
    _found: &Found,
    _width: u16,
    _height: u16,
    _scroll: i64,
) -> Preview {
    Preview {
        lines: Vec::new(),
        scroll: 0,
    }
}
