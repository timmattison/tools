//! `cdva` — "count da various attributes".
//!
//! The library behind the `cdva` binary: a code counter that reports the test
//! code of a tree apart from its production code.
//!
//! [`Language`] names every language the tool counts, reads the language of a
//! file out of its path, and carries the comment and string syntax of that
//! language. [`classify`] reads a source under that syntax and labels every row
//! blank, comment, or code, and [`ends_unterminated`] asks that same scan
//! whether it ended inside a string or a block comment, which is how a wrong
//! row of the language table announces itself. [`PathRules`] marks a whole file
//! as test material from its path alone, which is the cheap half of the split,
//! and [`TreeRules`] parses what the path rule leaves and marks the rows of a
//! test node — or rather parses the few of those files whose raw bytes hold a
//! literal of the language, which is what [`TreeMode`] decides.
//!
//! [`walk`] finds the files, [`Counter`] reads one of them into a [`FileCount`]
//! of two buckets, [`resolve_test_modules`] marks the files that a
//! `#[cfg(test)] mod <name>;` declaration moved the test code into — the one
//! rule that reads across files, so it runs once every file is counted — and
//! [`Summary`] rolls those up by language. Together they are the whole run:
//! walk, count, resolve, add up. [`render_table`] prints that summary as a
//! table, and [`ReportOptions`] is the whole of what shapes it: one row for
//! each file rather than each language, the column the rows are ordered by, how
//! many of them are kept, and which [`Bucket`] the columns report.
//! [`render_json`] and [`render_csv`] print that same summary for a program to
//! read: the same rows in the same order, and every number the tool knows
//! about each of them rather than the ones a table has room for.
//!
//! The invariant that a reader of the report leans on lives in
//! [`FileCount::total`] — the two buckets always sum to the count the tool
//! would report with the split turned off.

pub mod counts;
pub mod file;
pub mod lang;
pub mod lines;
pub mod modpass;
pub mod pathrule;
pub mod report;
pub mod treerule;
pub mod walk;

pub use counts::{Row, Summary};
pub use file::{Counter, FileCount, ParseStatus, Rule, Span};
pub use lang::{AttributeChain, BlockSpec, CommentSyntax, Language, StringSpec};
pub use lines::{classify, count, ends_unterminated, Counts, LineIndex, LineKind};
pub use modpass::resolve_test_modules;
pub use pathrule::{PathRules, PathVerdict};
pub use report::{
    render_csv, render_explanation, render_failed_parses, render_json, render_table, Bucket,
    ReportOptions, SortColumn,
};
pub use treerule::{TreeMode, TreeOutcome, TreeRules};
pub use walk::{walk, WalkOptions};
