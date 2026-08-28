//! The line classifier: which rows of a file are blank, which are comments,
//! and which are code.
//!
//! One pass of a state machine over the bytes of the source labels every row.
//! The rule is the rule of `cloc`: a row with no character that is not white
//! space is blank, a row that holds a character of code is code, and every
//! other row is a comment. A row that holds both code and a comment is code.
//!
//! The scan reads bytes rather than characters, and that is safe because every
//! delimiter the language table holds is ASCII. A character of more than one
//! byte is built of bytes that are all 0x80 or above, so no such byte begins a
//! delimiter and no delimiter can match across the middle of one. Nothing here
//! ever indexes the source as a string, which is what `clippy::string_slice`
//! asks for and what keeps a file of Japanese from panicking the tool.

use crate::lang::{BlockSpec, CommentSyntax, Language, StringSpec};
use std::ops::{Add, AddAssign};

/// The kind of one line. A line holding both code and a comment is code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// The row holds no character that is not white space.
    Blank,
    /// The row holds a comment, and no code.
    Comment,
    /// The row holds code.
    Code,
}

/// The count of one bucket.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Counts {
    /// The rows that hold nothing but white space.
    pub blank: u64,
    /// The rows that hold a comment and no code.
    pub comment: u64,
    /// The rows that hold code.
    pub code: u64,
}

impl Counts {
    /// Every row of every kind.
    #[must_use]
    pub fn total(self) -> u64 {
        self.blank
            .saturating_add(self.comment)
            .saturating_add(self.code)
    }

    /// Adds one row of this kind.
    ///
    /// This is the one place a [`LineKind`] turns into a number, so the two
    /// buckets of a file and the count of the whole file are added up by the
    /// same code. A second copy of this match is how a bucket comes to count a
    /// comment row as code while the total counts it as a comment, and the two
    /// numbers then disagree with no line of either saying why.
    pub fn add_kind(&mut self, kind: LineKind) {
        let field = match kind {
            LineKind::Blank => &mut self.blank,
            LineKind::Comment => &mut self.comment,
            LineKind::Code => &mut self.code,
        };
        *field = field.saturating_add(1);
    }
}

impl Add for Counts {
    type Output = Counts;

    fn add(self, other: Counts) -> Counts {
        Counts {
            blank: self.blank.saturating_add(other.blank),
            comment: self.comment.saturating_add(other.comment),
            code: self.code.saturating_add(other.code),
        }
    }
}

impl AddAssign for Counts {
    fn add_assign(&mut self, other: Counts) {
        *self = *self + other;
    }
}

/// The byte offset at which each row starts, so a byte offset converts to a
/// row.
///
/// A byte offset never indexes the source directly. Tree-sitter reports a byte
/// offset, and a later slice turns one into a row through this index rather
/// than through `&source[..offset]`, which panics in the middle of a character
/// of more than one byte.
pub struct LineIndex {
    /// The byte offset of the first byte of each row, in order.
    starts: Vec<usize>,
}

impl LineIndex {
    /// Reads the row starts of a source.
    #[must_use]
    pub fn new(source: &str) -> Self {
        let mut starts = Vec::new();
        if let Some(last) = source.len().checked_sub(1) {
            starts.push(0);
            for (offset, byte) in source.bytes().enumerate() {
                // A newline that ends the source closes the last row rather
                // than opening an empty one after it.
                if byte == b'\n' && offset < last {
                    starts.push(offset + 1);
                }
            }
        }
        LineIndex { starts }
    }

    /// The 0-based row holding this byte offset. Saturates at the last row.
    #[must_use]
    pub fn row_of(&self, byte_offset: usize) -> u32 {
        let row = self
            .starts
            .partition_point(|&start| start <= byte_offset)
            .saturating_sub(1);
        u32::try_from(row).unwrap_or(u32::MAX)
    }

    /// The number of rows. A source ending in a newline does not gain an empty
    /// last row.
    #[must_use]
    pub fn row_count(&self) -> u32 {
        u32::try_from(self.starts.len()).unwrap_or(u32::MAX)
    }
}

/// Label every row of `source` under the syntax of `language`.
///
/// The returned vector has exactly [`LineIndex::row_count`] entries.
#[must_use]
pub fn classify(source: &str, language: Language) -> Vec<LineKind> {
    let index = LineIndex::new(source);
    let rows = index.starts.len();
    if rows == 0 {
        return Vec::new();
    }

    let mut scanner = Scanner {
        bytes: source.as_bytes(),
        syntax: language.comment_syntax(),
        code: vec![false; rows],
        comment: vec![false; rows],
        pos: 0,
        row: 0,
        row_start: 0,
        state: Scan::Normal,
    };
    scanner.run();
    scanner.finish()
}

/// Sum the labels of [`classify`].
#[must_use]
pub fn count(source: &str, language: Language) -> Counts {
    let mut counts = Counts::default();
    for kind in classify(source, language) {
        counts.add_kind(kind);
    }
    counts
}

/// Where the scanner stands between two bytes.
#[derive(Clone, Copy)]
enum Scan {
    /// Outside every comment and every string.
    Normal,
    /// Inside a block comment, `depth` openers deep. The depth only ever grows
    /// past one where the language nests such a comment.
    Block {
        depth: u32,
        spec: &'static BlockSpec,
    },
    /// Inside a string. `doc` says the string counts as a comment, which it
    /// does when it opened its row and its form allows it.
    Str {
        spec: &'static StringSpec,
        doc: bool,
    },
    /// Inside a raw string of the Rust shape, closed by a quote and this many
    /// hash marks.
    RawString { hashes: usize },
}

/// One pass over one source.
struct Scanner<'a> {
    /// The source, as bytes.
    bytes: &'a [u8],
    /// The syntax of the language of the source.
    syntax: &'static CommentSyntax,
    /// Whether each row holds a character of code.
    code: Vec<bool>,
    /// Whether each row holds a character of a comment.
    comment: Vec<bool>,
    /// The byte the scanner reads next.
    pos: usize,
    /// The row that byte is on.
    row: usize,
    /// The byte offset at which that row starts.
    row_start: usize,
    /// Where the scanner stands.
    state: Scan,
}

impl Scanner<'_> {
    /// Reads every byte of the source.
    fn run(&mut self) {
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            match self.state {
                Scan::Normal => self.step_normal(byte),
                Scan::Block { depth, spec } => self.step_block(byte, depth, spec),
                Scan::Str { spec, doc } => self.step_string(byte, spec, doc),
                Scan::RawString { hashes } => self.step_raw_string(byte, hashes),
            }
        }
    }

    /// Turns the two flags of each row into its label.
    fn finish(self) -> Vec<LineKind> {
        self.code
            .into_iter()
            .zip(self.comment)
            .map(|(code, comment)| {
                if code {
                    LineKind::Code
                } else if comment {
                    LineKind::Comment
                } else {
                    LineKind::Blank
                }
            })
            .collect()
    }

    /// Reads one byte outside every comment and every string.
    ///
    /// The three groups are tried in one order, and the order is what makes Lua
    /// `--[[` a block comment rather than a line comment, and Python `"""` a
    /// docstring rather than an empty string beside a quote: block openers,
    /// then line comment tokens, then string openers.
    fn step_normal(&mut self, byte: u8) {
        if byte == b'\n' {
            self.end_row();
            return;
        }

        let syntax = self.syntax;

        for spec in syntax.block {
            if spec.line_anchored && self.pos != self.row_start {
                continue;
            }
            if self.matches(spec.open) {
                self.state = Scan::Block { depth: 1, spec };
                self.mark_comment();
                self.pos += spec.open.len();
                return;
            }
        }

        for token in syntax.line {
            if self.matches(token) {
                self.mark_comment();
                self.skip_to_end_of_row();
                return;
            }
        }

        if syntax.raw_hash_strings {
            if let Some((hashes, length)) = self.raw_string_opener(byte) {
                self.state = Scan::RawString { hashes };
                self.mark_code();
                self.pos += length;
                return;
            }
        }

        for spec in syntax.strings {
            if self.matches(spec.open) {
                let doc = spec.doc_when_line_leading && self.row_is_untouched();
                self.state = Scan::Str { spec, doc };
                self.mark(doc);
                self.pos += spec.open.len();
                return;
            }
        }

        if !is_space(byte) {
            self.mark_code();
        }
        self.pos += 1;
    }

    /// Reads one byte inside a block comment.
    fn step_block(&mut self, byte: u8, depth: u32, spec: &'static BlockSpec) {
        if byte == b'\n' {
            self.end_row();
            return;
        }

        let anchored_here = !spec.line_anchored || self.pos == self.row_start;

        if anchored_here && self.matches(spec.close) {
            self.state = match depth.checked_sub(1) {
                Some(0) | None => Scan::Normal,
                Some(remaining) => Scan::Block {
                    depth: remaining,
                    spec,
                },
            };
            self.mark_comment();
            self.pos += spec.close.len();
            return;
        }

        if self.syntax.nested_block && anchored_here && self.matches(spec.open) {
            self.state = Scan::Block {
                depth: depth.saturating_add(1),
                spec,
            };
            self.mark_comment();
            self.pos += spec.open.len();
            return;
        }

        if !is_space(byte) {
            self.mark_comment();
        }
        self.pos += 1;
    }

    /// Reads one byte inside a string.
    fn step_string(&mut self, byte: u8, spec: &'static StringSpec, doc: bool) {
        if byte == b'\n' {
            // A string of one row ends where its row does, so an unterminated
            // quote never swallows the rest of the file.
            if !spec.multiline {
                self.state = Scan::Normal;
            }
            self.end_row();
            return;
        }

        if escape_byte(spec) == Some(byte) {
            self.mark(doc);
            self.pos += 1;
            match self.bytes.get(self.pos).copied() {
                // An escaped newline continues the string onto the next row,
                // which is what every language with an escape does.
                Some(b'\n') => self.end_row(),
                Some(escaped) => {
                    if !is_space(escaped) {
                        self.mark(doc);
                    }
                    self.pos += 1;
                }
                None => {}
            }
            return;
        }

        if self.matches(spec.close) {
            self.state = Scan::Normal;
            self.mark(doc);
            self.pos += spec.close.len();
            return;
        }

        if !is_space(byte) {
            self.mark(doc);
        }
        self.pos += 1;
    }

    /// Reads one byte inside a raw string of the Rust shape.
    fn step_raw_string(&mut self, byte: u8, hashes: usize) {
        if byte == b'\n' {
            self.end_row();
            return;
        }

        if byte == b'"' {
            let end = self.pos.saturating_add(1).saturating_add(hashes);
            let closes = self
                .bytes
                .get(self.pos + 1..end)
                .is_some_and(|tail| tail.iter().all(|&byte| byte == b'#'));
            if closes {
                self.state = Scan::Normal;
                self.mark_code();
                self.pos = end;
                return;
            }
        }

        if !is_space(byte) {
            self.mark_code();
        }
        self.pos += 1;
    }

    /// The hash count and the length of a raw string opener at the cursor,
    /// where one stands: `r"`, `r#"`, `r##"`, and so on.
    ///
    /// The quote is what tells a raw string from the raw identifier `r#foo`
    /// and from an `r` that is only a letter of a name.
    fn raw_string_opener(&self, byte: u8) -> Option<(usize, usize)> {
        if byte != b'r' {
            return None;
        }
        let mut offset = self.pos + 1;
        while self.bytes.get(offset) == Some(&b'#') {
            offset += 1;
        }
        if self.bytes.get(offset) != Some(&b'"') {
            return None;
        }
        let hashes = offset - self.pos - 1;
        Some((hashes, offset - self.pos + 1))
    }

    /// Whether the bytes at the cursor open with this delimiter.
    ///
    /// An empty delimiter matches nothing. A table row that held one would
    /// otherwise spin the scan forever, and a test asserts no row does.
    fn matches(&self, delimiter: &str) -> bool {
        !delimiter.is_empty()
            && self
                .bytes
                .get(self.pos..)
                .is_some_and(|tail| tail.starts_with(delimiter.as_bytes()))
    }

    /// Whether nothing on the current row has been marked yet, which is what
    /// "the first character of the row that is not white space" means here.
    fn row_is_untouched(&self) -> bool {
        !self.code.get(self.row).copied().unwrap_or(false)
            && !self.comment.get(self.row).copied().unwrap_or(false)
    }

    /// Marks the current row as holding code.
    fn mark_code(&mut self) {
        if let Some(flag) = self.code.get_mut(self.row) {
            *flag = true;
        }
    }

    /// Marks the current row as holding a comment.
    fn mark_comment(&mut self) {
        if let Some(flag) = self.comment.get_mut(self.row) {
            *flag = true;
        }
    }

    /// Marks the current row as holding a comment where `doc` says so, and as
    /// holding code otherwise.
    fn mark(&mut self, doc: bool) {
        if doc {
            self.mark_comment();
        } else {
            self.mark_code();
        }
    }

    /// Steps over the newline at the cursor and onto the next row.
    fn end_row(&mut self) {
        self.row += 1;
        self.pos += 1;
        self.row_start = self.pos;
    }

    /// Steps to the newline that ends the current row, or to the end of the
    /// source. The newline itself stays for the caller's next step.
    fn skip_to_end_of_row(&mut self) {
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            if byte == b'\n' {
                return;
            }
            self.pos += 1;
        }
    }
}

/// The escape of a string form, as one byte.
///
/// Every escape the table holds is ASCII. A character that is not fails this
/// conversion, and the form then reads as one with no escape at all rather than
/// as one whose escape matches the middle of a character.
fn escape_byte(spec: &StringSpec) -> Option<u8> {
    spec.escape
        .and_then(|escape| u8::try_from(u32::from(escape)).ok())
}

/// Whether a byte is white space that no bucket counts.
///
/// Only ASCII white space counts. Every byte of a character of more than one
/// byte is 0x80 or above, so such a character is content, which is what a
/// reader of the row would call it.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | 0x0b | 0x0c)
}
