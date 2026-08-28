//! The tree rule: which rows of a file hold test code, read from its syntax
//! tree.
//!
//! The path rule marks a whole file from its name. This rule marks a region of
//! a file that the path rule left [`Unmarked`], by parsing it and asking a
//! tree-sitter query which of its nodes are tests. [`TreeRules::outcome`] is
//! the whole interface: hand it a source and its language, and it hands back
//! the rows.
//!
//! [`Unmarked`]: crate::pathrule::PathVerdict::Unmarked

use crate::file::{ParseStatus, Span};
use crate::lang::Language;
use std::collections::BTreeSet;

/// The rows of a file that hold test code, and how the parse went.
pub struct TreeOutcome {
    /// The 1-based rows that hold test code.
    pub rows: BTreeSet<u32>,
    /// The regions the query matched, in the order it found them.
    pub spans: Vec<Span>,
    /// Whether the parse held.
    pub status: ParseStatus,
}

/// Parses a file and asks it which of its rows belong to a test.
pub struct TreeRules {}

impl TreeRules {
    /// A tree rule that reads the query of every language that has one.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    /// The test rows of `source`.
    ///
    /// Returns `None` when the language has no tree rule, which is the answer
    /// that leaves the whole file to the production bucket.
    #[must_use]
    pub fn outcome(&self, _source: &str, _language: Language) -> Option<TreeOutcome> {
        None
    }
}

impl Default for TreeRules {
    fn default() -> Self {
        Self::new()
    }
}
