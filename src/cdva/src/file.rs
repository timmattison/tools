//! One file: the counts of both buckets, and the spans that put rows there.
//!
//! [`Counter`] is the entrance. It reads one file, labels every row through the
//! line classifier, and splits those rows between the production bucket and the
//! test bucket. The path rule marks a whole file from its name, and the tree
//! rule marks a region of one the path rule left unmarked. A counter built with
//! [`Counter::new`] alone reads the path rule and nothing else, which is
//! exactly what `--no-tree` will mean; [`Counter::with_tree_rules`] adds the
//! parse.
//!
//! # The order of the two rules
//!
//! The path rule runs first, and a file it settles is never parsed. A glob of
//! the user that holds a file out of the test bucket holds *all* of it out, so
//! a parse could only disagree with the user. A glob that marks a file marks it
//! whole, so a parse could only find rows that are already marked. Either way
//! the parse buys nothing, and skipping it is what makes a run over a tree of
//! test files cheap.
//!
//! # The invariant
//!
//! For every file, the production count plus the test count equals the count
//! the classifier reports on its own. [`FileCount::total`] is that sum, and the
//! split never adds a row or drops one, because the classifier decides the
//! *kind* of a row and the rules decide only its *bucket*. The two decisions
//! are independent, so the invariant holds by construction rather than by care.

use crate::lang::Language;
use crate::lines::{self, Counts, LineKind};
use crate::pathrule::{PathRules, PathVerdict};
use crate::treerule::{TreeMode, TreeRules};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Which rule marked a span, so `--explain` can name it in a later slice.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Rule {
    /// The glob of the path rule that marked the whole file, written as the
    /// user wrote it rather than as the rule compiled it.
    PathGlob(String),
    /// The tree rule matched a node of this kind.
    TreeNode(String),
    /// Another file declares this whole file as its test module, with a
    /// `#[cfg(test)] mod <name>;` that names it. The name is the module's, as
    /// the declaration spelled it, and not the file's.
    ModDeclaration(String),
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
/// that throws no error proves nothing. The tree rule asks the root node
/// whether the tree holds a defect, and reports [`Failed`] when it does.
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
    /// The names of the `#[cfg(test)] mod <name>;` declarations this file
    /// holds, each of which moves the test code of a module into another file.
    ///
    /// The names travel with the count so that [`resolve_test_modules`] can
    /// read them without parsing anything a second time.
    ///
    /// [`resolve_test_modules`]: crate::modpass::resolve_test_modules
    pub test_mod_declarations: Vec<String>,
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
    /// The parser that marks a region of a file the globs left unmarked, where
    /// the run asked for one.
    tree: Option<TreeRules>,
    /// When that parser runs.
    mode: TreeMode,
}

impl Counter {
    /// A counter that reads the path rule alone.
    ///
    /// No file is ever parsed, so a file the globs do not mark is production
    /// code from its first row to its last. This is what `--no-tree` selects,
    /// and a counter built with [`TreeMode::Never`] reads a file exactly as
    /// this one does.
    #[must_use]
    pub const fn new(rules: PathRules) -> Self {
        Self {
            rules,
            tree: None,
            mode: TreeMode::Never,
        }
    }

    /// Read the file with the tree rule as well as the path rule, in `mode`.
    ///
    /// The mode is an argument rather than a default because a caller that
    /// says nothing would get the filtered mode, and a test of the filter that
    /// silently ran filtered would pass by comparing a thing with itself.
    #[must_use]
    pub fn with_tree_rules(mut self, tree: TreeRules, mode: TreeMode) -> Self {
        self.tree = Some(tree);
        self.mode = mode;
        self
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
        let kinds = lines::classify(source, language);

        let (production, test, spans, parse_status, test_mod_declarations) =
            match self.rules.verdict(relative) {
                PathVerdict::Test(glob) => {
                    let (production, test) = split(&kinds, |_| true);
                    let spans = whole_file(&kinds, Rule::PathGlob(glob));
                    (production, test, spans, ParseStatus::NotParsed, Vec::new())
                }
                PathVerdict::Production(_) => {
                    let (production, test) = split(&kinds, |_| false);
                    (
                        production,
                        test,
                        Vec::new(),
                        ParseStatus::NotParsed,
                        Vec::new(),
                    )
                }
                PathVerdict::Unmarked => {
                    match self
                        .tree
                        .as_ref()
                        .and_then(|tree| tree.outcome(source, language, self.mode))
                    {
                        Some(outcome) => {
                            let (production, test) =
                                split(&kinds, |row| outcome.rows.contains(&row));
                            (
                                production,
                                test,
                                outcome.spans,
                                outcome.status,
                                outcome.test_mod_declarations,
                            )
                        }
                        None => {
                            let (production, test) = split(&kinds, |_| false);
                            (
                                production,
                                test,
                                Vec::new(),
                                ParseStatus::NotParsed,
                                Vec::new(),
                            )
                        }
                    }
                }
            };

        Some(FileCount {
            path: path.to_path_buf(),
            language,
            production,
            test,
            spans,
            parse_status,
            test_mod_declarations,
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

/// Splits the rows of a file between the two buckets.
///
/// `is_test` reads a 1-based row and says which bucket it belongs to. This is
/// the one place the split happens, and it is why the invariant of the tool
/// holds by construction rather than by care: the classifier already decided
/// the *kind* of every row, this decides only the *bucket*, and every row is
/// added to exactly one bucket under the kind it already has. Nothing here can
/// invent a row, drop one, or count one twice.
fn split(kinds: &[LineKind], is_test: impl Fn(u32) -> bool) -> (Counts, Counts) {
    let mut production = Counts::default();
    let mut test = Counts::default();
    for (offset, kind) in kinds.iter().enumerate() {
        let row = u32::try_from(offset.saturating_add(1)).unwrap_or(u32::MAX);
        if is_test(row) {
            test.add_kind(*kind);
        } else {
            production.add_kind(*kind);
        }
    }
    (production, test)
}

/// The one span that covers every row of a file, under this rule.
///
/// A file of no rows carries no span; see [`Span`].
fn whole_file(kinds: &[LineKind], rule: Rule) -> Vec<Span> {
    match u32::try_from(kinds.len()).unwrap_or(u32::MAX) {
        0 => Vec::new(),
        rows => vec![Span {
            first_row: 1,
            last_row: rows,
            rule,
        }],
    }
}
