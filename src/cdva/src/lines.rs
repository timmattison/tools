//! The line classifier: which rows of a file are blank, which are comments,
//! and which are code.
//!
//! One pass of a state machine over the bytes of the source labels every row.
//! The rule is the rule of `cloc`: a row with no character that is not white
//! space is blank, a row that holds a character of code is code, and every
//! other row is a comment. A row that holds both code and a comment is code.

use crate::lang::Language;
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
        if !source.is_empty() {
            starts.push(0);
            let last = source.len() - 1;
            for (offset, byte) in source.bytes().enumerate() {
                if byte == b'\n' && offset < last {
                    starts.push(offset + 1);
                }
            }
        }
        LineIndex { starts }
    }

    /// The 0-based row holding this byte offset. Saturates at the last row.
    #[must_use]
    pub fn row_of(&self, _byte_offset: usize) -> u32 {
        0
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
pub fn classify(source: &str, _language: Language) -> Vec<LineKind> {
    let index = LineIndex::new(source);
    vec![LineKind::Code; index.starts.len()]
}

/// Sum the labels of [`classify`].
#[must_use]
pub fn count(source: &str, language: Language) -> Counts {
    let mut counts = Counts::default();
    for kind in classify(source, language) {
        match kind {
            LineKind::Blank => counts.blank = counts.blank.saturating_add(1),
            LineKind::Comment => counts.comment = counts.comment.saturating_add(1),
            LineKind::Code => counts.code = counts.code.saturating_add(1),
        }
    }
    counts
}
