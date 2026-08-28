//! One file: the counts of both buckets, and the spans that put rows there.
//!
//! [`Counter`] is the entrance. It reads one file, labels every row through the
//! line classifier, and splits those rows between the production bucket and the
//! test bucket. In this slice only the path rule marks anything, so a marked
//! file is test material from its first row to its last and an unmarked file is
//! production code. That is exactly what `--no-tree` will mean, and the tree
//! rule of a later slice narrows the marking without changing the shape of the
//! answer.
//!
//! # The invariant
//!
//! For every file, the production count plus the test count equals the count
//! the classifier reports on its own. [`FileCount::total`] is that sum, and the
//! split never adds a row or drops one, because the classifier decides the
//! *kind* of a row and the rules decide only its *bucket*. The two decisions
//! are independent, so the invariant holds by construction rather than by care.

use crate::lang::Language;
use crate::lines::{self, Counts, LineIndex};
use crate::pathrule::{PathRules, PathVerdict};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Which rule marked a span, so `--explain` can name it in a later slice.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Rule {
    /// The glob of the path rule that marked the whole file, written as the
    /// user wrote it rather than as the rule compiled it.
    PathGlob(String),
}

/// One marked region of one file, in 1-based inclusive rows.
///
/// A file of no rows carries no span. A span of `1..=0` would be the empty
/// region spelled as a region, and every reader of a row range has to special
/// case it; an absent span says the same thing and needs no special case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    /// The first row of the region, counting from one.
    pub first_row: u32,
    /// The last row of the region, which the region includes.
    pub last_row: u32,
    /// The rule that marked the region.
    pub rule: Rule,
}

/// Whether the file was parsed, and whether the parse held.
///
/// Tree-sitter recovers from a syntax error and still returns a tree, so a run
/// that throws no error proves nothing. A later slice looks for an ERROR node
/// and reports [`Failed`] when it finds one.
///
/// [`Failed`]: ParseStatus::Failed
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ParseStatus {
    /// No parser ever ran, because the path rule settled the file or because
    /// the tree rule is off.
    #[default]
    NotParsed,
    /// A parser ran and the tree holds no error.
    Clean,
    /// A parser ran and the tree holds an error, so the marking of this file is
    /// not to be trusted.
    Failed,
}

/// The result of reading one file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileCount {
    /// The file, as the walk found it.
    pub path: PathBuf,
    /// The language the file was counted under.
    pub language: Language,
    /// The rows of the file that are production code.
    pub production: Counts,
    /// The rows of the file that are test material.
    pub test: Counts,
    /// The regions that the rules marked, in the order the rules found them.
    pub spans: Vec<Span>,
    /// Whether a parser ran over this file, and how it went.
    pub parse_status: ParseStatus,
}

impl FileCount {
    /// The count with the split turned off.
    ///
    /// This is the invariant of the tool: the two buckets always sum to it.
    #[must_use]
    pub fn total(&self) -> Counts {
        self.production + self.test
    }

    /// Whether this file counts as a test file.
    ///
    /// A file counts as a test file when at least one of its rows is a test
    /// row. An empty file that a glob marked therefore is not one: it holds no
    /// test row, and a report that counted it would name a test file with
    /// nothing in it.
    #[must_use]
    pub fn is_test_file(&self) -> bool {
        self.test.total() > 0
    }
}

/// Counts one file at a time under a set of rules.
pub struct Counter {
    /// The globs that mark a whole file from its path alone.
    rules: PathRules,
}

impl Counter {
    /// A counter that reads these rules.
    #[must_use]
    pub const fn new(rules: PathRules) -> Self {
        Self { rules }
    }

    /// Count `source`, which was read from `path`.
    ///
    /// `relative` is the path as the globs of the path rule see it, relative to
    /// the root of the walk. The two paths are separate because the rules read
    /// a path that a reader of the command line would recognise, while the
    /// report prints the path that the walk found.
    ///
    /// Returns `None` for a file of no language the tool counts.
    #[must_use]
    pub fn count_source(&self, path: &Path, relative: &Path, source: &str) -> Option<FileCount> {
        let language = Language::from_path(path)?;
        let counts = lines::count(source, language);
        let rows = LineIndex::new(source).row_count();

        let (production, test, spans) = match self.rules.verdict(relative) {
            PathVerdict::Test(glob) => {
                // A file of no rows carries no span; see [`Span`].
                let spans = if rows == 0 {
                    Vec::new()
                } else {
                    vec![Span {
                        first_row: 1,
                        last_row: rows,
                        rule: Rule::PathGlob(glob),
                    }]
                };
                (Counts::default(), counts, spans)
            }
            PathVerdict::Production(_) | PathVerdict::Unmarked => {
                (counts, Counts::default(), Vec::new())
            }
        };

        Some(FileCount {
            path: path.to_path_buf(),
            language,
            production,
            test,
            spans,
            parse_status: ParseStatus::NotParsed,
        })
    }

    /// Read `path` and count it.
    ///
    /// Returns `None` for a file of no known language, and for a file that does
    /// not hold UTF-8 text. The language is read first, so a file the tool does
    /// not count is never opened.
    ///
    /// The read is a read of bytes and not of a string on purpose. The error of
    /// [`std::fs::read_to_string`] does not separate "this file is not text"
    /// from "this file cannot be read", and the two answers are not the same
    /// answer: a binary in the tree is a file to skip, and a permission denied
    /// is a fact the run must report.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read.
    pub fn count_path(&self, path: &Path, relative: &Path) -> Result<Option<FileCount>> {
        if Language::from_path(path).is_none() {
            return Ok(None);
        }

        let bytes =
            std::fs::read(path).with_context(|| format!("cannot read `{}`", path.display()))?;

        let Ok(source) = String::from_utf8(bytes) else {
            return Ok(None);
        };

        Ok(self.count_source(path, relative, &source))
    }
}
