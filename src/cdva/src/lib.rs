//! `cdva` — "count da various attributes".
//!
//! The library behind the `cdva` binary: a code counter that reports the test
//! code of a tree apart from its production code.
//!
//! [`Language`] names every language the tool counts, reads the language of a
//! file out of its path, and carries the comment and string syntax of that
//! language. [`classify`] reads a source under that syntax and labels every row
//! blank, comment, or code. [`PathRules`] marks a whole file as test material
//! from its path alone, which is the cheap half of the split. A later slice
//! hangs the tree rule off the same table.
//!
//! [`walk`] finds the files, [`Counter`] reads one of them into a [`FileCount`]
//! of two buckets, and [`Summary`] rolls those up by language. Together they
//! are the whole run: walk, count, add up. The invariant that a reader of the
//! report leans on lives in [`FileCount::total`] — the two buckets always sum
//! to the count the tool would report with the split turned off.

pub mod counts;
pub mod file;
pub mod lang;
pub mod lines;
pub mod pathrule;
pub mod walk;

pub use counts::{Row, Summary};
pub use file::{Counter, FileCount, ParseStatus, Rule, Span};
pub use lang::{BlockSpec, CommentSyntax, Language, StringSpec};
pub use lines::{classify, count, Counts, LineIndex, LineKind};
pub use pathrule::{PathRules, PathVerdict};
pub use walk::{walk, WalkOptions};
