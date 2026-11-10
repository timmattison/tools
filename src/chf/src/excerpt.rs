//! The preview lines of one session.
//!
//! The module lays the whole session out as one sequence of lines and cuts a
//! window of `height` lines from it. The sequence holds, for each message in
//! order: the compaction lines that sit before it, an author line, the wrapped
//! text of the message, and an empty line (not after the last message).
//!
//! Only the messages that the view needs are wrapped. The module starts at
//! the message of the anchor (the last message for `Found::Any`). It adds
//! messages before and after that message only until the view is full and the
//! clamps are known. A session of 10,000 messages thus costs a few messages
//! for a view of 30 lines.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;

use unicode_width::UnicodeWidthChar;

use crate::query::{Found, Query};
use crate::session::{CompactionPoint, Role, Session};

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
    /// The text of the piece. It is never empty in a returned line.
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
            .map(char_width)
            .sum()
    }
}

/// Group the digits of `n` in threes: `259822` gives `"259,822"`.
///
/// This is the one place that groups digits. The renderer uses it too.
#[must_use]
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Wrap `text` to `width` display columns with the rule of the preview.
///
/// The rule is the one that the preview uses for a message: the text splits
/// at each `\n`, a line breaks after its last space when the next char does
/// not fit, and the sanitizing of the preview applies. An empty part gives one
/// empty line. A caller that must know how many rows a text takes wraps it
/// here and draws the lines as they come. A second wrap of its own, for
/// example the word wrap of a ratatui paragraph, can need more rows than the
/// caller counted. A `width` of 0 gives no lines.
#[must_use]
pub fn wrap_text(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    wrap_message(text, usize::from(width))
        .lines
        .iter()
        .map(|line| line.items.iter().map(|item| item.ch).collect())
        .collect()
}

/// The lines of the view of `session`.
///
/// `found` picks the anchor. `Found::At` puts the line of the match 3 lines
/// below the top. `Found::Any` puts the last line of the session at the bottom.
/// `scroll` moves the top. Both the default top and the final top are clamped
/// to the first and the last line of the session. A `width` or `height` of 0
/// gives an empty preview.
#[must_use]
pub fn preview(
    session: &Session,
    query: &Query,
    found: &Found,
    width: u16,
    height: u16,
    scroll: i64,
) -> Preview {
    if width == 0 || height == 0 {
        return Preview {
            lines: Vec::new(),
            scroll: 0,
        };
    }
    let mut window = Window::new(session, query, found, usize::from(width));
    window.fill(usize::from(height), scroll);
    window.cut(usize::from(height), scroll)
}

/// Lines above the match in the default view.
const LINES_ABOVE_MATCH: usize = 3;

/// The blocks of the session that the view needs, in order.
struct Window<'a> {
    session: &'a Session,
    query: &'a Query,
    width: usize,
    /// Compaction points by the index of the block that holds them.
    points: BTreeMap<usize, Vec<CompactionPoint>>,
    /// The count of blocks of the whole session.
    block_count: usize,
    /// The index of the first block in `blocks`.
    first: usize,
    blocks: VecDeque<Vec<PreviewLine>>,
    /// The count of lines in `blocks`.
    total: usize,
    /// The line of the match inside the window. `None` means the last line.
    anchor: Option<usize>,
    /// The lines between the top of the default view and the anchor.
    above: usize,
}

impl<'a> Window<'a> {
    fn new(session: &'a Session, query: &'a Query, found: &Found, width: usize) -> Self {
        let count = session.messages.len();
        // One block for each message, one block after them for the compaction
        // points at the end. A session with no message has one block.
        let block_count = if count == 0 { 1 } else { count + 1 };
        let mut points: BTreeMap<usize, Vec<CompactionPoint>> = BTreeMap::new();
        for point in &session.compaction_points {
            let index = if count == 0 {
                0
            } else {
                point.before_message.min(count)
            };
            points.entry(index).or_default().push(*point);
        }
        let mut window = Window {
            session,
            query,
            width,
            points,
            block_count,
            first: 0,
            blocks: VecDeque::new(),
            total: 0,
            anchor: None,
            above: 0,
        };
        match found {
            Found::At(hit) if count > 0 => {
                let index = hit.message.min(count - 1);
                let (block, inner) = window.build(index, Some(hit.range.start));
                window.first = index;
                window.total = block.len();
                window.anchor = Some(inner.unwrap_or(0));
                window.above = LINES_ABOVE_MATCH;
                window.blocks.push_back(block);
            }
            _ => {
                let index = block_count - 1;
                let (block, _) = window.build(index, None);
                window.first = index;
                window.total = block.len();
                window.blocks.push_back(block);
            }
        }
        window
    }

    /// The line of the anchor inside the window.
    fn anchor_line(&self) -> usize {
        self.anchor.unwrap_or(self.total.saturating_sub(1))
    }

    /// Add blocks until the clamps and the view are known.
    fn fill(&mut self, height: usize, scroll: i64) {
        if self.anchor.is_none() {
            self.above = height - 1;
        }
        let back = usize::try_from(scroll.unsigned_abs()).unwrap_or(usize::MAX);
        let (back, forth) = if scroll < 0 { (back, 0) } else { (0, back) };
        let need_before = self
            .above
            .saturating_add(back)
            .saturating_add(height)
            .saturating_add(1);
        let need_after = height.saturating_add(forth).saturating_add(1);
        loop {
            let mut moved = false;
            let anchor = self.anchor_line();
            if self.first > 0 && anchor < need_before {
                let (block, _) = self.build(self.first - 1, None);
                self.first -= 1;
                self.total += block.len();
                if let Some(line) = self.anchor.as_mut() {
                    *line += block.len();
                }
                self.blocks.push_front(block);
                moved = true;
            }
            let end = self.first + self.blocks.len();
            if end < self.block_count && self.total - self.anchor_line() < need_after {
                let (block, _) = self.build(end, None);
                self.total += block.len();
                self.blocks.push_back(block);
                moved = true;
            }
            if !moved {
                break;
            }
        }
    }

    /// Cut the view out of the window.
    fn cut(&mut self, height: usize, scroll: i64) -> Preview {
        let last_top = to_i64(self.total.saturating_sub(height));
        let default_top = to_i64(self.anchor_line())
            .saturating_sub(to_i64(self.above))
            .clamp(0, last_top);
        let top = default_top.saturating_add(scroll).clamp(0, last_top);
        let top_index = usize::try_from(top).unwrap_or(0);
        let lines = self
            .blocks
            .iter()
            .flatten()
            .skip(top_index)
            .take(height)
            .cloned()
            .collect();
        Preview {
            lines,
            scroll: top - default_top,
        }
    }

    /// Build the lines of block `index`.
    ///
    /// `target` is a byte offset in the message. The second value is the line
    /// of the block that holds that offset.
    fn build(&self, index: usize, target: Option<usize>) -> (Vec<PreviewLine>, Option<usize>) {
        let mut lines = Vec::new();
        for point in self.points.get(&index).map_or(&[][..], Vec::as_slice) {
            lines.push(compaction_line(point, self.width));
        }
        let count = self.session.messages.len();
        if count == 0 {
            lines.push(single("(no messages)", Tone::Muted, self.width));
            return (lines, None);
        }
        let Some(span) = self.session.messages.get(index) else {
            return (lines, None);
        };
        let author = match span.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        lines.push(single(author, Tone::Author, self.width));
        let text = self.session.message_text(index);
        let ranges = self.query.ranges_in(text);
        let base = lines.len();
        let wrapped = wrap_message(text, self.width);
        let mut found = None;
        for (i, line) in wrapped.lines.iter().enumerate() {
            lines.push(paint(line, &ranges));
            if found.is_none() && target.is_some_and(|start| start < line.end) {
                found = Some(base + i);
            }
        }
        if target.is_some() && found.is_none() {
            found = Some(lines.len() - 1);
        }
        if index + 1 < count {
            lines.push(PreviewLine::default());
        }
        (lines, found)
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The display width of one char. A char with no width counts as 0.
fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// A line of one piece, cut to `width` columns.
fn single(text: &str, tone: Tone, width: usize) -> PreviewLine {
    let mut used = 0;
    let clipped: String = text
        .chars()
        .take_while(|c| {
            used += char_width(*c);
            used <= width
        })
        .collect();
    if clipped.is_empty() {
        return PreviewLine::default();
    }
    PreviewLine(vec![Piece {
        text: clipped,
        tone,
    }])
}

fn compaction_line(point: &CompactionPoint, width: usize) -> PreviewLine {
    let mut text = String::from("── compaction ──");
    if let Some(tokens) = point.pre_tokens {
        let _ = write!(text, " {} tokens before it", thousands(tokens));
    }
    single(&text, Tone::Compaction, width)
}

/// One char after sanitizing, with its place in the original text.
#[derive(Clone, Copy)]
struct Item {
    ch: char,
    /// The byte offset of the char in the message text.
    offset: usize,
    /// The byte offset after the char in the message text.
    after: usize,
    columns: usize,
}

/// One wrapped line.
struct WrappedLine {
    items: Vec<Item>,
    /// The first byte offset that this line does not hold.
    end: usize,
}

struct Wrapped {
    lines: Vec<WrappedLine>,
}

/// Wrap the text of one message. The caller matches offsets against
/// `WrappedLine::end`.
fn wrap_message(text: &str, width: usize) -> Wrapped {
    let mut lines = Vec::new();
    let mut base = 0;
    for part in text.split('\n') {
        wrap_part(part, base, width, &mut lines);
        base += part.len() + 1;
    }
    Wrapped { lines }
}

/// Wrap one part of a message (one line of its text) into `out`.
fn wrap_part(part: &str, base: usize, width: usize, out: &mut Vec<WrappedLine>) {
    let mut line: Vec<Item> = Vec::new();
    let mut used = 0;
    let mut push_line = |items: Vec<Item>| {
        let end = items.last().map_or(base, |item| item.after);
        out.push(WrappedLine { items, end });
    };
    for item in sanitize(part, base, width) {
        while used + item.columns > width {
            if let Some(space) = line.iter().rposition(|held| held.ch == ' ') {
                let tail = line.split_off(space + 1);
                push_line(std::mem::replace(&mut line, tail));
            } else {
                push_line(std::mem::take(&mut line));
            }
            used = line.iter().map(|held| held.columns).sum();
        }
        used += item.columns;
        line.push(item);
    }
    let end = base + part.len() + 1;
    out.push(WrappedLine { items: line, end });
}

/// Replace the chars that a terminal must not get. The result keeps the
/// offsets of the original chars.
fn sanitize(part: &str, base: usize, width: usize) -> Vec<Item> {
    let mut items = Vec::new();
    for (position, c) in part.char_indices() {
        let offset = base + position;
        let after = offset + c.len_utf8();
        let (ch, count) = match c {
            '\r' => continue,
            '\t' => (' ', 4),
            c if c.is_control() => ('\u{FFFD}', 1),
            c => (c, 1),
        };
        let mut columns = char_width(ch);
        let ch = if columns > width { '\u{FFFD}' } else { ch };
        if ch == '\u{FFFD}' {
            columns = char_width(ch).min(width);
        }
        for _ in 0..count {
            items.push(Item {
                ch,
                offset,
                after,
                columns,
            });
        }
    }
    items
}

/// Turn one wrapped line into pieces. Items inside a range are highlights.
fn paint(line: &WrappedLine, ranges: &[std::ops::Range<usize>]) -> PreviewLine {
    let mut pieces: Vec<Piece> = Vec::new();
    for item in &line.items {
        let slot = ranges.partition_point(|range| range.end <= item.offset);
        let lit = ranges
            .get(slot)
            .is_some_and(|range| range.start <= item.offset);
        let tone = if lit { Tone::Highlight } else { Tone::Text };
        match pieces.last_mut() {
            Some(last) if last.tone == tone => last.text.push(item.ch),
            _ => pieces.push(Piece {
                text: item.ch.to_string(),
                tone,
            }),
        }
    }
    PreviewLine(pieces)
}
