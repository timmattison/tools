//! The tree rule: which rows of a file hold test code, read from its syntax
//! tree.
//!
//! The path rule marks a whole file from its name. This rule marks a region of
//! a file that the path rule left [`Unmarked`], by parsing it and asking a
//! tree-sitter query which of its nodes are tests. [`TreeRules::outcome`] is
//! the whole interface: hand it a source and its language, and it hands back
//! the rows. Everything below — the grammar, the query, the capture names, the
//! chain of attributes, the recovery of the parser — stays behind that call,
//! and adding a language is a row of the table in [`crate::lang`] plus a
//! fixture.
//!
//! # The three capture names
//!
//! A capture whose name starts with `_` is a helper for a `#match?` predicate
//! and marks nothing. The three that mark are:
//!
//! - `@test` — the span of the captured node is test code.
//! - `@candidate` — the node is test code *only when* the chain of attributes
//!   before it matches, and the span then reaches back to the first attribute
//!   of that chain. Rust needs this, because there an attribute is a sibling of
//!   the item it decorates rather than a child of it.
//! - `@test_scope` — the outermost enclosing node of a kind the language lists
//!   is test code.
//!
//! Any other capture name is a mistake in the table, and it is refused loudly:
//! a capture the rule quietly ignored would mark nothing, and a language that
//! marks nothing reads exactly like a language with no test code in it.
//!
//! # The filter in front of the parser
//!
//! A parse costs far more than a scan of the rows, so the tool parses as few
//! files as it can. Three filters stand in front of the parser, in order of
//! what they cost: the path rule settles a whole file from its name and never
//! opens it; a literal search over the raw bytes then drops every file that can
//! hold no test at all, because a Rust file whose bytes hold neither `test` nor
//! `bench` can hold no test node; and what survives both is parsed.
//!
//! [`TreeMode`] says which of the two later filters run.
//! [`TreeMode::Auto`] is the default and the one described above.
//! [`TreeMode::Never`] parses nothing, which is `--no-tree` and the fast mode.
//! [`TreeMode::Always`] skips the literal search and parses every file of a
//! language that has a rule, which is `--tree` and the mode a test of the
//! filter reads.
//!
//! The needle set of a language must be a *superset* of everything its query
//! can match, and the two mistakes are not symmetrical. A needle that filters
//! nothing is merely slow. A needle that filters too much is a silent
//! undercount: the file is never parsed, its test rows are never found, and the
//! number that comes out reads exactly like a correct one. A test in
//! `tests/treerule.rs` holds `Auto` and `Always` to the same marking over the
//! fixture corpus and over this repository, which is what proves the sets
//! complete.
//!
//! # A parse that did not hold
//!
//! Tree-sitter recovers from a syntax error and still returns a tree, so a
//! parse that threw nothing proves nothing. Two shapes of defect come back:
//! an `ERROR` node, for input the parser could not fit, and a `MISSING` node,
//! for a token the parser inserted to carry on. Both were measured against
//! `tree-sitter-rust` 0.24, and `Node::has_error` on the root reports both —
//! a `let x = 1` with no semicolon yields a tree whose only defect is a
//! `MISSING ";"` and whose root still answers `true`. So the one call is
//! enough, and no walk of the tree is needed.
//!
//! A file whose parse did not hold counts entirely as production code. The
//! marking of such a file is not to be trusted, and a guessed test count is
//! worse than none: it reads exactly like a measured one.
//!
//! # A NUL byte in the source
//!
//! The lexer of a generated parser reads the value 0 as the end of the input,
//! because 0 is the value it gives a real end of input. A NUL byte inside a
//! literal is data that no language here objects to, and a grammar that met
//! one stopped there and marked the rest of the file an error. That is a
//! defect of the parser rather than of the file, and two files of a real
//! repository hit it.
//!
//! So the parser reads a copy in which every NUL byte is a space. A space is
//! one byte, as a NUL byte is, so every row and column of the tree still names
//! the row and column of the file, and no offset the query reports moves.
//!
//! The substitution reaches the parser and nothing else. The row
//! classification and the needle filter both read the file as it is, and a
//! file whose parse fails for any other reason fails as it did. A NUL byte
//! that sits between two tokens rather than inside a literal is a defect this
//! hides, and no compiler of these languages reads such a file either. This
//! tool counts rows, and it does not rule on whether a file builds.
//!
//! # The test code that is somewhere else
//!
//! A Rust file that declares `#[cfg(test)] mod tests;` holds none of the test
//! code it is talking about: the whole of the file it names is test code, and
//! the declaration is the only evidence of that anywhere. This rule reads one
//! file at a time and so cannot act on it, but it is the only thing that ever
//! reads the declaring file, so it collects the name into
//! [`TreeOutcome::test_mod_declarations`] and [`crate::modpass`] resolves it
//! once every file has been counted.
//!
//! [`Unmarked`]: crate::pathrule::PathVerdict::Unmarked

use crate::file::{ParseStatus, Rule, Span};
use crate::lang::{AttributeChain, Language};
use crate::lines::LineIndex;
use memchr::memmem::Finder;
use regex::Regex;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

/// The capture naming a node whose own span is test code.
const CAPTURE_TEST: &str = "test";

/// The capture naming a node that is test code when its attribute chain says
/// so.
const CAPTURE_CANDIDATE: &str = "candidate";

/// The capture naming a node whose outermost enclosing scope is test code.
const CAPTURE_TEST_SCOPE: &str = "test_scope";

/// The node kind of a Rust `mod` item, which is the one item that can move its
/// test code into another file.
const MOD_ITEM: &str = "mod_item";

/// The field of a `mod` item that holds the braces and everything in them.
const FIELD_BODY: &str = "body";

/// The field of a `mod` item that holds the name of the module.
const FIELD_NAME: &str = "name";

/// When the tree rule runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TreeMode {
    /// Parse a file only when a needle of its language appears in it.
    #[default]
    Auto,
    /// Never parse. The path rule alone decides, which is the fast mode.
    Never,
    /// Parse every file of a language that has a rule, skipping the needle
    /// filter. This is the slow and complete mode that a test of the filter
    /// uses.
    Always,
}

/// The rows of a file that hold test code, and how the parse went.
pub struct TreeOutcome {
    /// The 1-based rows that hold test code.
    pub rows: BTreeSet<u32>,
    /// The regions the query matched, in the order it found them. Two of them
    /// may cover the same row — a `#[test] fn` inside a `#[cfg(test)] mod` is
    /// two nodes over one region — which is why the rows are a set and not a
    /// sum of lengths.
    pub spans: Vec<Span>,
    /// Whether the parse held.
    pub status: ParseStatus,
    /// The names of `#[cfg(test)] mod <name>;` declarations, each of which
    /// moves the test code of this module into another file.
    pub test_mod_declarations: Vec<String>,
}

impl TreeOutcome {
    /// The outcome of a parse that did not hold.
    ///
    /// No row is a test row, so the file counts entirely as production code.
    fn failed() -> Self {
        Self {
            rows: BTreeSet::new(),
            spans: Vec::new(),
            status: ParseStatus::Failed,
            test_mod_declarations: Vec::new(),
        }
    }
}

/// Parses a file and asks it which of its rows belong to a test.
pub struct TreeRules {
    /// One slot per language of [`Language::all`], in that order, filled the
    /// first time a file of that language arrives.
    ///
    /// A `Query` and a `Regex` are both costly to build and both `Sync`, so
    /// each is built once and then read from every thread that counts a file.
    /// The slot is lazy rather than eager because a run over a tree of one
    /// language would otherwise pay to compile the queries of a dozen
    /// languages it never reads.
    compiled: Vec<OnceLock<Option<Compiled>>>,
}

impl TreeRules {
    /// A tree rule that reads the query of every language that has one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            compiled: std::iter::repeat_with(OnceLock::new)
                .take(Language::all().len())
                .collect(),
        }
    }

    /// The test rows of `source`.
    ///
    /// Returns `None` for every file this rule does not read, and the three
    /// ways a file reaches that answer cost three different amounts: the mode
    /// is [`TreeMode::Never`] and nothing is read at all; the language has no
    /// tree rule, which one comparison settles; or the mode is
    /// [`TreeMode::Auto`] and no needle of the language appears in the bytes of
    /// the file, which one pass over those bytes settles. All three leave the
    /// whole file to the production bucket without opening a parser, and the
    /// caller tells none of them apart — a file that was never parsed is a file
    /// that was never parsed.
    ///
    /// # Panics
    ///
    /// Panics when the query of the language table does not compile against
    /// the grammar of that table, when it captures a name that marks nothing,
    /// or when its attribute pattern is not a regular expression. Each of
    /// those is a fact of the table rather than of the file being counted, a
    /// test asserts all three for every language, and an answer of "no test
    /// rows" instead would silently miscount every file of that language.
    #[must_use]
    pub fn outcome(&self, source: &str, language: Language, mode: TreeMode) -> Option<TreeOutcome> {
        if mode == TreeMode::Never {
            return None;
        }
        let compiled = self.compiled(language)?;
        if mode == TreeMode::Auto && !compiled.may_hold_a_test(source.as_bytes()) {
            return None;
        }
        // After the filter, so a file that is never parsed never pays for the
        // copy. The needle holds no NUL byte, so the filter reads the same
        // answer out of either text.
        let text = without_nul(source);
        let text: &str = text.as_ref();

        // A fresh parser for every call. `tree_sitter::Parser` is `Send` but
        // not `Sync`, so one cannot be shared between the rayon threads that
        // read the files, and a single parser behind a lock would serialise the
        // one part of the run worth doing in parallel. Building one is cheap
        // beside parsing a file.
        let mut parser = Parser::new();
        if parser.set_language(&compiled.grammar).is_err() {
            return Some(TreeOutcome::failed());
        }
        let Some(tree) = parser.parse(text, None) else {
            return Some(TreeOutcome::failed());
        };
        if tree.root_node().has_error() {
            return Some(TreeOutcome::failed());
        }

        let index = LineIndex::new(text);
        let mut rows = BTreeSet::new();
        let mut spans = Vec::new();
        let mut test_mod_declarations = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&compiled.query, tree.root_node(), text.as_bytes());
        while let Some(matched) = matches.next() {
            for capture in matched.captures {
                let Some(marking) = compiled.marking(capture.index) else {
                    continue;
                };
                let Some(span) = compiled.span_of(marking, capture.node, text, &index) else {
                    continue;
                };
                if let Some(module) = declared_test_module(capture.node, text) {
                    test_mod_declarations.push(module);
                }
                rows.extend(span.first_row..=span.last_row);
                spans.push(span);
            }
        }

        Some(TreeOutcome {
            rows,
            spans,
            status: ParseStatus::Clean,
            test_mod_declarations,
        })
    }

    /// The compiled rule of a language, built on first use.
    fn compiled(&self, language: Language) -> Option<&Compiled> {
        let slot = self.compiled.get(index_of(language)?)?;
        slot.get_or_init(|| Compiled::new(language)).as_ref()
    }
}

impl Default for TreeRules {
    fn default() -> Self {
        Self::new()
    }
}

/// The source a parser reads: the file, with every NUL byte replaced by a
/// space.
///
/// A NUL byte is one byte and a space is one byte, so the copy holds every
/// other byte of the file at the offset it had. Thus a row, a column, and a
/// byte range of the tree all name the same place in the file the reader has
/// open.
///
/// The copy is made only for a file that holds such a byte, which is very few
/// of them. See the module documentation for why the substitution is made at
/// all.
fn without_nul(source: &str) -> Cow<'_, str> {
    if memchr::memchr(0, source.as_bytes()).is_none() {
        return Cow::Borrowed(source);
    }
    Cow::Owned(source.replace('\0', " "))
}

/// The slot a language's compiled rule lives in, which is its position in
/// [`Language::all`].
fn index_of(language: Language) -> Option<usize> {
    Language::all().iter().position(|&known| known == language)
}

/// What a capture marks.
#[derive(Clone, Copy)]
enum Marking {
    /// `@test`: the span of the captured node.
    Whole,
    /// `@candidate`: the node together with the chain of attributes before it,
    /// and only when one of those attributes says so.
    Candidate,
    /// `@test_scope`: the outermost enclosing node of a listed kind.
    Scope,
}

/// One language's tree rule, compiled.
struct Compiled {
    /// The grammar the parser is set to.
    grammar: tree_sitter::Language,
    /// The query, compiled against that grammar.
    query: Query,
    /// One searcher per needle of the language, built once and read from every
    /// thread. `Finder::new` builds a skip table over the needle, which is the
    /// work that makes the search fast and the work that must not be repeated
    /// per file.
    finders: Vec<Finder<'static>>,
    /// What each capture of the query marks, by capture index.
    markings: Vec<Option<Marking>>,
    /// The attribute chain and the compiled form of its pattern.
    chain: Option<(AttributeChain, Regex)>,
    /// The node kinds a `@test_scope` capture may climb to.
    scope_kinds: &'static [&'static str],
}

impl Compiled {
    /// Compiles the tree rule of a language, or `None` where it has none.
    ///
    /// # Panics
    ///
    /// Panics when the table's query does not compile, captures a name that
    /// marks nothing, or carries a pattern that is not a regular expression.
    /// See [`TreeRules::outcome`].
    fn new(language: Language) -> Option<Self> {
        let source = language.tree_query()?;
        let grammar = language.grammar()?;
        let query = Query::new(&grammar, source).unwrap_or_else(|error| {
            panic!(
                "the tree query of {} does not compile: {error}",
                language.name()
            )
        });
        let markings = query
            .capture_names()
            .iter()
            .map(|name| marking_of(name, language))
            .collect();
        let chain = language.attribute_chain().map(|chain| {
            let pattern = Regex::new(chain.pattern).unwrap_or_else(|error| {
                panic!(
                    "the attribute pattern of {} is not a regular expression: {error}",
                    language.name()
                )
            });
            (chain, pattern)
        });

        Some(Self {
            grammar,
            query,
            finders: language
                .needles()
                .iter()
                .map(|needle| Finder::new(needle.as_bytes()).into_owned())
                .collect(),
            markings,
            chain,
            scope_kinds: language.scope_kinds(),
        })
    }

    /// Whether `source` can hold a test of this language at all.
    ///
    /// This is the whole of the filter: a literal search over the raw bytes,
    /// which costs one pass and no allocation, in front of a parse that costs
    /// far more than that. A Rust file whose bytes hold neither `test` nor
    /// `bench` can hold no test node, whatever else is in it.
    ///
    /// The search is over bytes rather than over characters, which is right for
    /// two reasons. It is faster, and every needle is ASCII while UTF-8 never
    /// spells an ASCII byte inside a character of several bytes, so a byte
    /// found is a character found. Nothing here cuts the source at the offset
    /// it found, so a file of Japanese is read exactly as one of ASCII is.
    ///
    /// A language that declares no needle answers `true` for every file, so a
    /// row of the table that says nothing is parsed every time. That is the
    /// safe direction: the cost of saying nothing is a slow run, and the cost
    /// of saying too much is a file that is never read and never counted.
    fn may_hold_a_test(&self, source: &[u8]) -> bool {
        self.finders.is_empty()
            || self
                .finders
                .iter()
                .any(|finder| finder.find(source).is_some())
    }

    /// What the capture of this index marks, where it marks anything.
    fn marking(&self, index: u32) -> Option<Marking> {
        *self.markings.get(usize::try_from(index).ok()?)?
    }

    /// The span a capture marks, or `None` where this capture marks nothing
    /// after all.
    ///
    /// A `@candidate` whose attribute chain says nothing is the only capture
    /// that can decline here, and declining is the whole point of the name: a
    /// plain `fn` and a `#[test] fn` are the same node kind, and only the
    /// siblings before them tell the two apart.
    fn span_of(
        &self,
        marking: Marking,
        node: Node<'_>,
        source: &str,
        index: &LineIndex,
    ) -> Option<Span> {
        let (first_byte, end_byte, kind) = match marking {
            Marking::Whole => (node.start_byte(), node.end_byte(), node.kind()),
            Marking::Candidate => (
                self.chain_start(node, source)?,
                node.end_byte(),
                node.kind(),
            ),
            Marking::Scope => {
                let scope = outermost_scope(node, self.scope_kinds);
                (scope.start_byte(), scope.end_byte(), scope.kind())
            }
        };
        let (first_row, last_row) = rows_of(index, first_byte, end_byte);

        Some(Span {
            first_row,
            last_row,
            rule: Rule::TreeNode(kind.to_string()),
        })
    }

    /// The byte at which the chain of attributes before `node` starts, where
    /// one of those attributes says the node is test code.
    ///
    /// The whole contiguous chain is walked, and not the one adjacent sibling.
    /// A stack such as `#[rstest]` over `#[case(1)]` puts the attribute that
    /// decides two siblings back, so a walk of one sibling reads `#[case(1)]`,
    /// finds nothing, and drops that test — while still passing every fixture
    /// whose deciding attribute happens to sit next to the item.
    ///
    /// The text of an attribute is taken through `utf8_text`, which reads a
    /// byte range as a string. Nothing here indexes the source, so a file of
    /// Japanese or of emoji is read exactly as one of ASCII is.
    fn chain_start(&self, node: Node<'_>, source: &str) -> Option<usize> {
        let (chain, pattern) = self.chain.as_ref()?;
        let mut start = None;
        let mut decided = false;

        let mut sibling = node.prev_sibling();
        while let Some(attribute) = sibling {
            if attribute.kind() != chain.kind {
                break;
            }
            if let Ok(text) = attribute.utf8_text(source.as_bytes()) {
                decided |= pattern.is_match(text);
            }
            start = Some(attribute.start_byte());
            sibling = attribute.prev_sibling();
        }

        if decided {
            start
        } else {
            None
        }
    }
}

/// The module a `#[cfg(test)] mod <name>;` moves its test code into, where
/// `node` is such a declaration.
///
/// A node reaches here only once the rule has decided it is test code, so the
/// `#[cfg(test)]` has already been read off the chain of attributes before it.
/// What is left to tell apart is `mod tests;` from `mod tests { … }`, and the
/// two differ by a child rather than by a character: the braces and everything
/// in them are the `body` field of the node, so its *absence* is the question
/// asked here. Looking for a `;` in the text would answer the same for a module
/// whose body holds one, which is every module that holds a statement.
///
/// The name comes back through `utf8_text`, which reads a byte range as a
/// string, so a module named in Japanese is read exactly as one named in ASCII
/// is. Nothing here indexes the source.
///
/// Returns `None` for every node that is not such a declaration, which is every
/// node of every other language: `mod_item` is a kind of the Rust grammar
/// alone, and Rust is the one language of the table whose test code can live in
/// a file that says nothing about itself.
fn declared_test_module(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != MOD_ITEM || node.child_by_field_name(FIELD_BODY).is_some() {
        return None;
    }
    let name = node.child_by_field_name(FIELD_NAME)?;
    name.utf8_text(source.as_bytes()).ok().map(str::to_string)
}

/// What a capture of this name marks, or `None` where it marks nothing.
///
/// A name that starts with `_` is a helper a `#match?` predicate reads, and it
/// marks nothing on purpose. Any other name must be one of the three that
/// carry meaning; a fourth is refused rather than ignored, because a capture
/// the rule skipped would make the query mark less than its author wrote and
/// nothing in the output would say so.
fn marking_of(name: &str, language: Language) -> Option<Marking> {
    match name {
        CAPTURE_TEST => Some(Marking::Whole),
        CAPTURE_CANDIDATE => Some(Marking::Candidate),
        CAPTURE_TEST_SCOPE => Some(Marking::Scope),
        _ if name.starts_with('_') => None,
        _ => panic!(
            "the tree query of {} captures `@{name}`, which marks nothing",
            language.name()
        ),
    }
}

/// The outermost node of a listed kind at or above `node`.
///
/// Elixir is what needs this: `use ExUnit.Case` is a `call`, and so is the
/// `defmodule` that holds it, so climbing the `call` ancestors of the `use`
/// reaches the module it belongs to and leaves a neighbouring production
/// module alone. A node that sits under no ancestor of a listed kind marks its
/// own span, so a query naming a kind the tree never holds marks too little
/// rather than disappearing.
fn outermost_scope<'tree>(node: Node<'tree>, kinds: &[&str]) -> Node<'tree> {
    let mut outermost = node;
    let mut current = node;
    while let Some(parent) = current.parent() {
        if kinds.contains(&parent.kind()) {
            outermost = parent;
        }
        current = parent;
    }
    outermost
}

/// The 1-based inclusive rows that a byte range covers.
///
/// Tree-sitter counts rows from zero and everything else in this tool counts
/// them from one, so the conversion happens here and in no other place. The
/// end of the range is read one byte back, because a node that ends at the
/// first byte of the next row must not claim that row.
fn rows_of(index: &LineIndex, start_byte: usize, end_byte: usize) -> (u32, u32) {
    let first = index.row_of(start_byte);
    let last = index
        .row_of(end_byte.saturating_sub(1).max(start_byte))
        .max(first);
    (first.saturating_add(1), last.saturating_add(1))
}
